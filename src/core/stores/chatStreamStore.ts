import { defineStore } from "pinia";
import { ref, computed, reactive, onScopeDispose } from "vue";
import { invoke, Channel } from "@tauri-apps/api/core";

import { useChatSessionStore } from "./chatSessionStore";
import { useAssistantStore } from "./assistant";
import { useAvatarStore } from "./avatar";
import { useTopicStore } from "./topicListManager";
import { useChatHistoryStore } from "./chatHistoryStore";
import type {
  ActiveGenerationDto,
  AuroraRecoverySnapshot,
  ChatMessage,
  ContentBlock,
  MarkdownNode,
  MessageShell,
  RecoveryResultDto,
  StreamBlock,
  StreamEventDto,
  TailFrame,
  TailRenderMode,
  TailTextOp,
} from "../types/chat";
import type {
  ConversationKey,
  ConversationOwnerType,
} from "./chatSessionStore";

interface StreamTerminalTombstone {
  terminalAt: number;
}

type TailContentOp = Extract<TailTextOp, { op: "append" | "replace" }>;

function buildTailBlockFromOp(
  op: TailContentOp,
  content: string,
  currentBlock?: StreamBlock,
): StreamBlock {
  const shared = {
    content,
    hash: op.hash,
    render_mode: op.mode,
  };

  if (op.blockType === "thought") {
    return {
      ...shared,
      type: "thought",
      theme: op.thoughtTheme
        ?? (currentBlock?.type === "thought" ? currentBlock.theme : "思维链"),
      is_complete: false,
    };
  }
  if (op.blockType === "html-preview") {
    return { ...shared, type: "html-preview" };
  }
  return { ...shared, type: "markdown" };
}

export const useChatStreamStore = defineStore("chatStream", () => {
  const streamingMessageKey = ref<string | null>(null);

  const conversationMapKey = (
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
  ) => JSON.stringify([ownerType, ownerId, topicId]);

  const streamMessageMapKeyFromConversation = (
    conversationKey: string,
    messageId: string,
  ) => JSON.stringify([conversationKey, messageId]);

  const streamMessageMapKey = (
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
    messageId: string,
  ) => streamMessageMapKeyFromConversation(
    conversationMapKey(ownerId, ownerType, topicId),
    messageId,
  );

  const compareMessageOrder = (a: ChatMessage, b: ChatMessage) => {
    if (a.timestamp !== b.timestamp) return a.timestamp - b.timestamp;
    return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
  };

  // 核心：记录每个完整会话是否处于活动流状态。
  const sessionActiveStreams = ref<Record<string, string[]>>({});

  // 全局活跃流消息池：按完整消息身份存储正在生成的响应对象。
  // 无论是在前台还是后台，流式消息都从此池中获取，保证响应式链路不断裂
  const activeStreamMessages = reactive<Map<string, ChatMessage>>(new Map());
  const streamTerminalTombstones = new Map<
    string,
    StreamTerminalTombstone
  >();
  // 覆盖完整 recovery command 生命周期，避免扫描门闩释放后重复 claim 同一消息。
  const recoveryMessageIds = new Set<string>();

  function isStreamDebugEnabled(): boolean {
    return Boolean(import.meta.env.DEV && (window as any).__VCP_STREAM_DEBUG__);
  }

  function recordStreamTrace(data: any): void {
    if (!isStreamDebugEnabled()) return;
    if (!(window as any).__VCP_STREAM_TRACES__) {
      (window as any).__VCP_STREAM_TRACES__ = [];
    }
    (window as any).__VCP_STREAM_TRACES__.push({
      timestamp: performance.now(),
      ...data,
    });
  }

  function streamDebugLog(...args: unknown[]): void {
    if (isStreamDebugEnabled()) {
      console.warn(...args);
    }
  }

  const MAX_PENDING_TAIL_MUTATIONS = 512;

  interface TailFrameCursor {
    streamId: number;
    epoch: number;
    revision: number;
    frameSeq: number;
  }

  interface TailFrameMergeResult {
    accepted: boolean;
    frame?: TailFrame;
    cursor?: TailFrameCursor;
    needsSnapshot?: string;
  }

  function mergeTailFrame(
    existing: TailFrame | null,
    cursor: TailFrameCursor | null,
    incoming: TailFrame,
    latestSnapshot?: MarkdownNode[],
    forceSnapshot = false,
  ): TailFrameMergeResult {
    const incomingMutations = incoming.mutations || [];
    const incomingCursor: TailFrameCursor = {
      streamId: incoming.streamId,
      epoch: incoming.epoch,
      revision: incoming.revision,
      frameSeq: incoming.frameSeq,
    };
    let snapshotReason = forceSnapshot ? "hidden" : null;
    if (cursor) {
      if (incoming.streamId < cursor.streamId) {
        return { accepted: false };
      }
      if (incoming.streamId > cursor.streamId) {
        snapshotReason = "stream_changed";
      } else if (incoming.epoch < cursor.epoch) {
        return { accepted: false };
      } else if (incoming.epoch > cursor.epoch) {
        snapshotReason = "epoch_changed";
      } else if (incoming.frameSeq <= cursor.frameSeq) {
        return { accepted: false };
      } else if (incoming.frameSeq > cursor.frameSeq + 1) {
        snapshotReason = "frame_gap";
      }
    } else if (incoming.frameSeq > 1) {
      snapshotReason = "late_first_frame";
    }

    const incomingSnapshot = latestSnapshot ?? incoming.snapshot;
    if (incoming.reset === true || snapshotReason) {
      if (incomingSnapshot !== undefined) {
        return {
          accepted: true,
          frame: {
            ...incoming,
            reset: true,
            snapshot: [...incomingSnapshot],
            mutations: [...incomingMutations],
          },
          cursor: incomingCursor,
        };
      }
      return {
        accepted: true,
        cursor: incomingCursor,
        needsSnapshot: snapshotReason || "reset_without_snapshot",
      };
    }

    // reset 尚未刷入 DOM 时保留其基线，并在同一批次继续累积后续补丁。
    if (existing?.reset) {
      const mutations = [
        ...(existing.mutations || []),
        ...incomingMutations,
      ];
      if (mutations.length > MAX_PENDING_TAIL_MUTATIONS) {
        return {
          accepted: true,
          cursor: incomingCursor,
          needsSnapshot: "mutation_overflow_after_reset",
        };
      }
      return {
        accepted: true,
        frame: {
          ...incoming,
          reset: true,
          snapshot: existing.snapshot,
          mutations,
        },
        cursor: incomingCursor,
      };
    }

    if (!existing) {
      return {
        accepted: true,
        frame: {
          ...incoming,
          mutations: [...incomingMutations],
          snapshot: incoming.snapshot ? [...incoming.snapshot] : undefined,
        },
        cursor: incomingCursor,
      };
    }

    const mutations = [
      ...(existing.reset ? [] : existing.mutations || []),
      ...incomingMutations,
    ];
    if (mutations.length > MAX_PENDING_TAIL_MUTATIONS) {
      return {
        accepted: true,
        cursor: incomingCursor,
        needsSnapshot: "mutation_overflow",
      };
    }

    return {
      accepted: true,
      frame: {
        ...incoming,
        reset: false,
        snapshot: incoming.snapshot || existing.snapshot,
        mutations,
      },
      cursor: incomingCursor,
    };
  }

  const cleanupTimers = new Set<ReturnType<typeof setTimeout>>();
  const STREAM_TERMINAL_TOMBSTONE_TTL_MS = 24 * 60 * 60 * 1000;
  const MAX_STREAM_TERMINAL_TOMBSTONES = 1000;

  const pruneStreamTerminalTombstones = (now = Date.now()) => {
    for (const [messageKey, tombstone] of streamTerminalTombstones) {
      if (now - tombstone.terminalAt <= STREAM_TERMINAL_TOMBSTONE_TTL_MS) {
        continue;
      }
      streamTerminalTombstones.delete(messageKey);
    }

    while (streamTerminalTombstones.size > MAX_STREAM_TERMINAL_TOMBSTONES) {
      const oldestMessageId = streamTerminalTombstones.keys().next().value;
      if (typeof oldestMessageId !== "string") break;
      streamTerminalTombstones.delete(oldestMessageId);
    }
  };

  const hasStreamTerminalTombstone = (messageKey: string): boolean => {
    const tombstone = streamTerminalTombstones.get(messageKey);
    if (!tombstone) return false;
    if (Date.now() - tombstone.terminalAt > STREAM_TERMINAL_TOMBSTONE_TTL_MS) {
      streamTerminalTombstones.delete(messageKey);
      return false;
    }
    return true;
  };

  const recordStreamTerminalTombstone = (messageKey: string) => {
    streamTerminalTombstones.delete(messageKey);
    streamTerminalTombstones.set(messageKey, {
      terminalAt: Date.now(),
    });
    pruneStreamTerminalTombstones();
  };

  // ===== rAF 对齐绘制的原子提交暂存池 =====
  // Rust vcp_client 的 33ms 门禁是 Aurora 唯一的频率上限。前端不能再设第二个
  // 33ms 时间门禁：后端时钟与屏幕 VSync 不同步，叠加会平白增加一帧延迟。
  // 此处 rAF 只合并下一次绘制前到达的事件，并将同一 Aurora 帧的字段原子写入 Vue。
  const rAFPendingUpdates = new Map<
    string,
    {
      content: string | null;
      blocks: ContentBlock[] | null;
      tailContent: string | null;
      tailBlock: StreamBlock | null;
      tailBlockChanged: boolean;
      tailOp: TailTextOp | null;
      tailFrame: TailFrame | null;
      tailSnapshot: MarkdownNode[] | null;
      streamId: number | null;
      tailCursor: TailFrameCursor | null;
      needsSnapshotReason: string | null;
      animationFrameId: number | null;
    }
  >();

  /**
   * 物理防线：强行中止、强制同步刷新并安全清理指定消息的 rAF 帧状态，杜绝任何泄漏与闪烁
   */
  const clearRAFUpdate = (messageKey: string, forceFlush = false) => {
    const up = rAFPendingUpdates.get(messageKey);
    if (up) {
      if (up.animationFrameId !== null) {
        cancelAnimationFrame(up.animationFrameId);
        up.animationFrameId = null;
      }
      if (forceFlush) {
        const msg = activeStreamMessages.get(messageKey);
        if (msg) {
          if (up.content !== null) msg.content = up.content;
          if (up.blocks !== null) msg.blocks = up.blocks;
          // 漏洞 1 修复：同步强刷收尾时，必须将暂存池中的 tail 字段强刷，绝不允许丢字闪烁
          if (up.tailContent !== null) msg.tailContent = up.tailContent;
          if (up.tailBlockChanged) {
            msg.tailBlock = up.tailBlock ?? undefined;
          }
          if (up.tailOp !== null) msg.tailOp = up.tailOp;
          if (up.tailSnapshot !== null)
            msg.tailSnapshot = up.tailSnapshot;
          if (up.tailFrame !== null) msg.tailFrame = up.tailFrame;
        }
      }
      rAFPendingUpdates.delete(messageKey);
    }
  };

  /**
   * 调度并申请 rAF 渲染，合并 data 和 aurora 的高频更新，在同一渲染帧内原子写入
   */
  const scheduleRAFUpdate = (messageKey: string) => {
    const update = rAFPendingUpdates.get(messageKey);
    if (!update || update.animationFrameId !== null) return;

    update.animationFrameId = requestAnimationFrame(() => {
      const up = rAFPendingUpdates.get(messageKey);
      if (!up) return;

      const m = activeStreamMessages.get(messageKey);
      if (m) {
        if (up.content !== null) m.content = up.content;
        if (up.blocks !== null) m.blocks = up.blocks;
        if (up.tailSnapshot !== null) m.tailSnapshot = up.tailSnapshot;
        if (up.tailFrame !== null) m.tailFrame = up.tailFrame;
        if (up.tailContent !== null) m.tailContent = up.tailContent;
        if (up.tailBlockChanged) {
          m.tailBlock = up.tailBlock ?? undefined;
        }
        if (up.tailOp !== null) m.tailOp = up.tailOp;
      }

      // 重置下一次绘制前的合并暂存状态。
      up.content = null;
      up.blocks = null;
      up.tailContent = null;
      up.tailBlock = null;
      up.tailBlockChanged = false;
      up.tailOp = null;
      up.tailFrame = null;
      up.tailSnapshot = null;
      up.animationFrameId = null;
    });
  };

  const auroraSnapshotJobs = new Map<string, Promise<boolean>>();

  const cursorFromFrame = (frame?: TailFrame): TailFrameCursor | null => frame
    ? {
        streamId: frame.streamId,
        epoch: frame.epoch,
        revision: frame.revision,
        frameSeq: frame.frameSeq,
      }
    : null;

  const cursorsEqual = (
    left: TailFrameCursor | null,
    right: TailFrameCursor | null,
  ) => !!left
    && !!right
    && left.streamId === right.streamId
    && left.epoch === right.epoch
    && left.revision === right.revision
    && left.frameSeq === right.frameSeq;

  const requestAuroraSnapshotByKey = (
    messageKey: string,
    reason: string,
  ): Promise<boolean> => {
    const existing = auroraSnapshotJobs.get(messageKey);
    if (existing) return existing;

    const job = (async () => {
      // 同一消息最多保留一个在途请求；若流在编译期间前进，就直接追赶最新 source/cursor。
      for (;;) {
        if (hasStreamTerminalTombstone(messageKey)) return false;
        const message = activeStreamMessages.get(messageKey);
        const pending = rAFPendingUpdates.get(messageKey);
        if (!message || !pending) return false;

        const source = pending.content ?? message.content ?? "";
        const cursor = pending.tailCursor ?? cursorFromFrame(message.tailFrame);
        if (!cursor) return false;

        if (import.meta.env.DEV && isStreamDebugEnabled()) {
          streamDebugLog(
            `[chatStreamStore] Requesting Aurora snapshot for ${message.id}: ${reason}`,
          );
        }

        let snapshot: AuroraRecoverySnapshot;
        try {
          snapshot = await invoke<AuroraRecoverySnapshot>(
            "rebuild_aurora_snapshot",
            { content: source },
          );
        } catch (error) {
          console.error(
            `[chatStreamStore] Failed to rebuild Aurora snapshot for ${message.id}:`,
            error,
          );
          return false;
        }

        if (hasStreamTerminalTombstone(messageKey)) return false;
        const latestMessage = activeStreamMessages.get(messageKey);
        const latestPending = rAFPendingUpdates.get(messageKey);
        if (!latestMessage || !latestPending) return false;
        const latestSource = latestPending.content
          ?? latestMessage.content
          ?? "";
        const latestCursor = latestPending.tailCursor
          ?? cursorFromFrame(latestMessage.tailFrame);
        if (latestSource !== source || !cursorsEqual(latestCursor, cursor)) {
          continue;
        }

        const mode: TailRenderMode = snapshot.tailMode
          ?? (snapshot.tailSnapshot.length > 0 ? "ast" : "plain");
        const tailBlock = snapshot.tailBlock
          ? { ...snapshot.tailBlock, render_mode: mode }
          : null;
        const resetFrame: TailFrame = {
          streamId: cursor.streamId,
          epoch: cursor.epoch,
          revision: cursor.revision,
          frameSeq: cursor.frameSeq,
          reset: true,
          snapshot: snapshot.tailSnapshot,
          mutations: [],
        };

        // Snapshot 是一次性原子恢复点：直接提交到响应式消息，避免等待下一次 rAF
        // 期间组件再次挂载并重复请求同一快照。
        if (latestPending.animationFrameId !== null) {
          cancelAnimationFrame(latestPending.animationFrameId);
          latestPending.animationFrameId = null;
        }
        latestMessage.content = latestSource;
        latestMessage.blocks = snapshot.stableBlocks;
        latestMessage.tailContent = snapshot.tailBlock?.content || "";
        latestMessage.tailBlock = tailBlock ?? undefined;
        latestMessage.tailSnapshot = snapshot.tailSnapshot;
        latestMessage.tailFrame = resetFrame;

        latestPending.content = null;
        latestPending.blocks = null;
        latestPending.tailContent = null;
        latestPending.tailBlock = null;
        latestPending.tailBlockChanged = false;
        latestPending.tailSnapshot = null;
        latestPending.tailFrame = null;
        latestPending.needsSnapshotReason = null;
        return true;
      }
    })().finally(() => {
      auroraSnapshotJobs.delete(messageKey);
    });

    auroraSnapshotJobs.set(messageKey, job);
    return job;
  };

  const requestAuroraSnapshot = (
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
    messageId: string,
    reason = "renderer",
  ) => requestAuroraSnapshotByKey(
    streamMessageMapKey(ownerId, ownerType, topicId, messageId),
    reason,
  );

  const handleAuroraVisibilityChange = () => {
    if (typeof document === "undefined" || document.hidden) return;
    for (const [messageKey, pending] of rAFPendingUpdates) {
      if (pending.needsSnapshotReason) {
        void requestAuroraSnapshotByKey(
          messageKey,
          pending.needsSnapshotReason,
        );
      }
    }
  };
  if (typeof document !== "undefined") {
    document.addEventListener("visibilitychange", handleAuroraVisibilityChange);
    onScopeDispose(() => {
      document.removeEventListener("visibilitychange", handleAuroraVisibilityChange);
    });
  }

  const sessionStore = useChatSessionStore();
  const assistantStore = useAssistantStore();
  const avatarStore = useAvatarStore();
  const topicStore = useTopicStore();

  /**
   * 在前端本地计算 MessageShell（替代 Rust 的 precompute_shell）
   */
  function computeShell(msg: {
    role: string;
    agentId?: string;
    name?: string;
  }): MessageShell {
    if (msg.role === "user") {
      const userColor =
        avatarStore.getDominantColor("user", "user_avatar") || "rgb(226,54,56)";
      return {
        avatarColor: userColor,
        displayName: msg.name || "User",
        isUser: true,
      };
    }
    const agent = msg.agentId
      ? assistantStore.agents.find((a) => a.id === msg.agentId)
      : undefined;
    return {
      avatarColor: agent?.avatarCalculatedColor || "",
      displayName: msg.name || agent?.name || "AI",
      isUser: false,
    };
  }

  const activeStreamSets = computed(() => {
    const sets: Record<string, Set<string>> = {};
    for (const [key, streams] of Object.entries(sessionActiveStreams.value)) {
      sets[key] = new Set(streams);
    }
    return sets;
  });

  const activeStreamKeySet = computed(() => {
    const keys = new Set<string>();
    for (const [conversationKey, streams] of Object.entries(
      sessionActiveStreams.value,
    )) {
      for (const id of streams) {
        keys.add(streamMessageMapKeyFromConversation(conversationKey, id));
      }
    }
    return keys;
  });

  function isMessageActive(
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
    messageId: string,
  ): boolean {
    return activeStreamKeySet.value.has(
      streamMessageMapKey(ownerId, ownerType, topicId, messageId),
    );
  }

  function isMessageActiveInSession(
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
    messageId: string,
  ): boolean {
    return (
      activeStreamSets.value[conversationMapKey(ownerId, ownerType, topicId)]?.has(
        messageId,
      ) ?? false
    );
  }

  function getActiveStreamMessage(
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
    messageId: string,
  ): ChatMessage | undefined {
    return activeStreamMessages.get(
      streamMessageMapKey(ownerId, ownerType, topicId, messageId),
    );
  }

  // 兼容旧逻辑的计算属性
  const activeStreamingIds = computed(() => {
    if (!sessionStore.currentSelectedItem?.id || !sessionStore.currentTopicId)
      return new Set<string>();
    const key = conversationMapKey(
      sessionStore.currentSelectedItem.id,
      sessionStore.currentSelectedItem.type,
      sessionStore.currentTopicId,
    );
    return activeStreamSets.value[key] || new Set<string>();
  });

  const isGroupGenerating = computed(() => {
    if (
      !sessionStore.currentSelectedItem?.id ||
      !sessionStore.currentTopicId ||
      sessionStore.currentSelectedItem.type !== "group"
    )
      return false;
    const key = conversationMapKey(
      sessionStore.currentSelectedItem.id,
      "group",
      sessionStore.currentTopicId,
    );
    const streams = sessionActiveStreams.value[key];
    return streams ? streams.length > 0 : false;
  });

  // 全局流消息池上限，防止极端场景下 OOM
  const MAX_STREAM_MESSAGES = 100;

  const enforceStreamPoolLimit = () => {
    if (activeStreamMessages.size <= MAX_STREAM_MESSAGES) return;
    let remaining = activeStreamMessages.size - MAX_STREAM_MESSAGES;
    // 按插入顺序（Map 保持插入顺序）清理最旧的非活跃消息
    for (const [messageKey] of activeStreamMessages) {
      if (remaining <= 0) break;
      // 只删除已完成的流（不在当前活跃会话中）
      if (!activeStreamKeySet.value.has(messageKey)) {
        activeStreamMessages.delete(messageKey);
        remaining -= 1;
      }
    }
  };

  // 辅助方法：管理会话流状态
  const addSessionStream = (
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
    messageId: string,
  ) => {
    const key = conversationMapKey(ownerId, ownerType, topicId);
    if (!sessionActiveStreams.value[key]) {
      sessionActiveStreams.value[key] = [];
    }
    if (!sessionActiveStreams.value[key].includes(messageId)) {
      sessionActiveStreams.value[key].push(messageId);
    }
    // 新增流时检查并执行上限保护
    enforceStreamPoolLimit();
  };

  const removeSessionStream = (
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
    messageId: string,
    retainMessageUntilTerminal = false,
  ) => {
    const key = conversationMapKey(ownerId, ownerType, topicId);
    const messageKey = streamMessageMapKey(
      ownerId,
      ownerType,
      topicId,
      messageId,
    );
    const streams = sessionActiveStreams.value[key];
    if (streams) {
      const index = streams.indexOf(messageId);
      if (index !== -1) {
        streams.splice(index, 1);
      }
      if (streams.length === 0) {
        delete sessionActiveStreams.value[key];
      }
    }
    // 手动停止只撤销活跃 UI，原消息对象必须等权威 end/error 复用并收口。
    // 若终态永久缺失，既有流消息池上限会回收这个非活跃对象。
    if (retainMessageUntilTerminal) return;

    // 同时从全局池中移除 (延迟移除，确保 finalizeStream 能拿到对象)
    const cleanupTimer = setTimeout(() => {
      cleanupTimers.delete(cleanupTimer);
      if (!activeStreamKeySet.value.has(messageKey)) {
        activeStreamMessages.delete(messageKey);
        clearRAFUpdate(messageKey, false); // 漏洞 2 修复：延迟清理时，强制安全注销 rAF 帧，杜绝句柄泄露
      }
    }, 1000);
    cleanupTimers.add(cleanupTimer);
  };

  /**
   * 处理流式事件的核心逻辑 (会话隔离调度器)
   */
  const processStreamEvent = async (
    event: StreamEventDto,
    callbacks?: {
      onMessageCreated?: (msg: ChatMessage, topicId: string) => void;
      onStreamFinished?: (messageId: string, topicId: string) => void;
    },
  ) => {
    const actualMessageId = event.messageId || "";
    const { type, context } = event;
    const ctx = context || {};
    const topicId = ctx.topicId;
    const ownerType = ctx.ownerType;
    const ownerId = ctx.ownerId;

    if (
      !actualMessageId ||
      !topicId ||
      !ownerId ||
      (ownerType !== "agent" && ownerType !== "group")
    ) return;
    const messageKey = streamMessageMapKey(
      ownerId,
      ownerType,
      topicId,
      actualMessageId,
    );

    if (hasStreamTerminalTombstone(messageKey)) return;
    if (type === "end") {
      recordStreamTerminalTombstone(messageKey);
    }

    let msg = activeStreamMessages.get(messageKey);
    const isNewStream = !msg;

    if (isNewStream) {
      msg = reactive<ChatMessage>({
        id: actualMessageId,
        role: "assistant",
        name: ctx.agentName,
        content: "",
        timestamp: Date.now(),
        isThinking: type === "thinking",
        agentId: ctx.agentId,
        groupId: ownerType === "group" ? ownerId : undefined,
        isGroupMessage: ownerType === "group",
        shell: computeShell({
          role: "assistant",
          agentId: ctx.agentId,
          name: ctx.agentName,
        }),
      });
      activeStreamMessages.set(messageKey, msg!);
    }

    if (isNewStream) {
      topicStore.incrementTopicMsgCount(ownerId, ownerType, topicId);
      const currentKey = sessionStore.currentConversationKey;
      if (
        !currentKey ||
        currentKey.ownerId !== ownerId ||
        currentKey.ownerType !== ownerType ||
        currentKey.topicId !== topicId
      ) {
        void topicStore
          .incrementTopicUnreadCount(ownerId, ownerType, topicId)
          .catch(() => {});
      }

      // 回调：通知 UI 列表插入新消息
      if (callbacks?.onMessageCreated) {
        callbacks.onMessageCreated(msg!, topicId);
      }
    }

    // 维护流状态
    if (type === "thinking") {
      msg!.isThinking = true;
      addSessionStream(ownerId, ownerType, topicId, actualMessageId);
      if (!streamingMessageKey.value) {
        streamingMessageKey.value = messageKey;
      }
    } else if (type === "aurora") {
      msg!.isThinking = false;
      addSessionStream(ownerId, ownerType, topicId, actualMessageId);

      const aurora = event.aurora;
      if (aurora) {
        if (import.meta.env.DEV && isStreamDebugEnabled()) {
          recordStreamTrace({
            messageId: actualMessageId,
            auroraPayload: {
              kind: aurora.kind,
              streamId: aurora.streamId,
              stableBlocksCount: aurora.stableBlocks?.length || 0,
              stableAppendCount: aurora.stableAppend?.blocks.length || 0,
              stableBlocksHashes:
                aurora.stableBlocks?.map((b) => b.hash) || [],
              tailOp: aurora.tailOp?.op || null,
              tailContent: aurora.tailBlock?.content || "",
              tailBlockType: aurora.tailBlock?.type || null,
              tailFrame: aurora.tailFrame
                ? {
                    streamId: aurora.tailFrame.streamId,
                    epoch: aurora.tailFrame.epoch,
                    revision: aurora.tailFrame.revision,
                    frameSeq: aurora.tailFrame.frameSeq,
                    reset: aurora.tailFrame.reset,
                    mutationsCount: aurora.tailFrame.mutations?.length || 0,
                    hasSnapshot: !!aurora.tailFrame.snapshot,
                  }
                : null,
            },
            msgSnapshot: msg
              ? {
                  contentLength: msg.content?.length || 0,
                  blocksCount: msg.blocks?.length || 0,
                  tailContentLength: msg.tailContent?.length || 0,
                }
              : null,
          });
        }

        // 1. 初始化或获取该 messageId 的帧合并状态
        let update = rAFPendingUpdates.get(messageKey);
        if (!update) {
          update = {
            content: null,
            blocks: null,
            tailContent: null,
            tailBlock: null,
            tailBlockChanged: false,
            tailOp: null,
            tailFrame: null,
            tailSnapshot: null,
            streamId: null,
            tailCursor: null,
            needsSnapshotReason: null,
            animationFrameId: null,
          };
          rAFPendingUpdates.set(messageKey, update);
        }

        // 2. 先认领 Aurora 流身份与帧序列，再合并同一事件的 chunk/blocks/tail。
        // 重复或迟到事件必须整体丢弃，否则即使忽略 AST frame，chunk 仍会被重复追加。
        const eventStreamId = aurora.streamId ?? aurora.tailFrame?.streamId;
        let streamChanged = false;
        if (eventStreamId !== undefined) {
          if (!Number.isSafeInteger(eventStreamId) || eventStreamId <= 0) return;
          if (
            aurora.tailFrame &&
            aurora.tailFrame.streamId !== eventStreamId
          ) return;
          if (update.streamId !== null && eventStreamId < update.streamId) return;
          if (update.streamId === null || eventStreamId > update.streamId) {
            streamChanged = update.streamId !== null;
            update.content = null;
            update.blocks = null;
            update.tailContent = null;
            update.tailBlock = null;
            update.tailBlockChanged = false;
            update.tailOp = null;
            update.tailFrame = null;
            update.tailSnapshot = null;
            update.streamId = eventStreamId;
            update.tailCursor = null;
            update.needsSnapshotReason = null;
          }
        }

        let mergedTailFrame: TailFrame | null = null;
        if (aurora.tailFrame) {
          if (eventStreamId === undefined) return;
          if (import.meta.env.DEV && isStreamDebugEnabled()) {
            streamDebugLog(
              `[chatStreamStore] Received tailFrame stream=${aurora.tailFrame.streamId} seq=${aurora.tailFrame.frameSeq} mutations=${aurora.tailFrame.mutations?.length || 0} for ${actualMessageId}`,
            );
          }
          const latestSnapshot =
            aurora.tailFrame.snapshot ??
            aurora.tailBlock?.nodes;
          const merged = mergeTailFrame(
            update.tailFrame,
            update.tailCursor,
            aurora.tailFrame,
            latestSnapshot,
            streamChanged
              || !!update.needsSnapshotReason
              || (typeof document !== "undefined" && document.hidden),
          );
          if (!merged.accepted || !merged.cursor) return;
          update.tailCursor = merged.cursor;
          if (merged.needsSnapshot) {
            update.tailFrame = null;
            update.needsSnapshotReason = merged.needsSnapshot;
          } else if (merged.frame) {
            mergedTailFrame = merged.frame;
            update.needsSnapshotReason = null;
          }
        }

        // 3. 覆盖 Snapshot，或按基线身份应用 Delta。
        if (typeof aurora.content === "string") {
          update.content = aurora.content;
        } else if (aurora.chunk) {
          const currentBase =
            update.content !== null ? update.content : msg!.content || "";
          update.content = currentBase + aurora.chunk;
        }

        if (aurora.kind === "snapshot") {
          if (aurora.stableBlocks) {
            update.blocks = aurora.stableBlocks;
          }
          const snapshotMode: TailRenderMode = aurora.tailMode
            ?? (aurora.tailBlock?.nodes ? "ast" : "plain");
          update.tailContent = aurora.tailBlock?.content || "";
          update.tailBlock = aurora.tailBlock
            ? { ...aurora.tailBlock, render_mode: snapshotMode }
            : null;
          update.tailBlockChanged = true;
        } else if (aurora.kind === "delta") {
          if (aurora.stableAppend) {
            const stableBase = update.blocks ?? msg!.blocks ?? [];
            if (stableBase.length === aurora.stableAppend.baseCount) {
              update.blocks = [
                ...stableBase,
                ...aurora.stableAppend.blocks,
              ];
            } else {
              update.needsSnapshotReason = "stable_base_mismatch";
            }
          }

          if (aurora.tailOp) {
            if (update.tailOp && aurora.tailOp.op === "append") {
              if (update.tailOp.op === "append") {
                update.tailOp = {
                  ...aurora.tailOp,
                  content: update.tailOp.content + aurora.tailOp.content,
                  previousHash: update.tailOp.previousHash,
                };
              } else if (update.tailOp.op === "replace") {
                update.tailOp = {
                  ...update.tailOp,
                  content: update.tailOp.content + aurora.tailOp.content,
                  hash: aurora.tailOp.hash,
                };
              } else {
                update.tailOp = aurora.tailOp;
              }
            } else {
              update.tailOp = aurora.tailOp;
            }

            const currentBlock = update.tailBlock ?? msg!.tailBlock;
            const currentContent = update.tailContent !== null
              ? update.tailContent
              : msg!.tailContent || "";
            if (aurora.tailOp.op === "clear") {
              update.tailContent = "";
              update.tailBlock = null;
              update.tailBlockChanged = true;
              update.tailSnapshot = [];
            } else if (aurora.tailOp.op === "replace") {
              update.tailContent = aurora.tailOp.content;
              update.tailBlock = buildTailBlockFromOp(
                aurora.tailOp,
                aurora.tailOp.content,
                currentBlock,
              );
              update.tailBlockChanged = true;
            } else {
              const currentHash = currentBlock?.hash
                ? String(currentBlock.hash)
                : undefined;
              if (currentHash !== aurora.tailOp.previousHash) {
                update.needsSnapshotReason = "tail_base_mismatch";
              } else {
                const nextContent = currentContent + aurora.tailOp.content;
                update.tailContent = nextContent;
                update.tailBlock = buildTailBlockFromOp(
                  aurora.tailOp,
                  nextContent,
                  currentBlock,
                );
                update.tailBlockChanged = true;
              }
            }
          }
        }

        if (mergedTailFrame) {
          update.tailFrame = mergedTailFrame;
          if (mergedTailFrame.snapshot !== undefined) {
            update.tailSnapshot = mergedTailFrame.snapshot;
          }
        }

        // 4. 申请硬件级 rAF 渲染调度（合并原子提交）
        scheduleRAFUpdate(messageKey);
        if (
          update.needsSnapshotReason
          && (typeof document === "undefined" || !document.hidden)
        ) {
          void requestAuroraSnapshotByKey(
            messageKey,
            update.needsSnapshotReason,
          );
        }
      }
    } else if (type === "error") {
      // error 只表示 durable finalizer 未能提交。保留当前 partial 和 active owner，
      // 不写终态 tombstone；后续 recovery 的权威 end 仍必须能够进入。
      clearRAFUpdate(messageKey, true);
      if (typeof event.content === "string") msg!.content = event.content;
      msg!.isThinking = false;
      msg!.isReconnecting = true;
    } else if (type === "end") {
      const finishReason = event.finishReason;
      if (typeof event.topicUpdatedAt === "number") {
        topicStore.setTopicUpdatedAt(
          ownerId,
          ownerType,
          topicId,
          event.topicUpdatedAt,
        );
      }

      // durable end 原子覆盖权威正文与渲染结果，再撤销活动流状态。
      clearRAFUpdate(messageKey, true);
      if (typeof event.content === "string") msg!.content = event.content;

      if (finishReason) msg!.finishReason = finishReason;

      if (streamingMessageKey.value === messageKey)
        streamingMessageKey.value = null;

      if (msg) {
        msg!.isThinking = false;
        msg!.isReconnecting = false;
        if (event.timestamp) {
          msg!.timestamp = event.timestamp;
        }

        // 定义闭环终结函数，保证在 blocks 赋值完成后才移除活动流状态
        const finalizeStream = () => {
          msg!.tailContent = "";
          msg!.tailBlock = undefined;
          msg!.tailOp = undefined;
          removeSessionStream(ownerId, ownerType, topicId, actualMessageId);
          if (callbacks?.onStreamFinished) {
            callbacks.onStreamFinished(actualMessageId, topicId);
          }
          if (typeof navigator !== "undefined" && navigator.vibrate) {
            navigator.vibrate(40);
          }
        };

        try {
          // 如果后端已经带回了预渲染好的 blocks，直接使用，跳过冗余解析
          if (event.blocks) {
            msg.blocks = event.blocks;
            finalizeStream();
          } else {
            invoke<ContentBlock[]>("process_message_content", {
              content: msg!.content || "",
            })
              .then((compiledBlocks) => {
                msg.blocks = compiledBlocks;
                finalizeStream();
              })
              .catch((e) => {
                console.error(
                  "[ChatStreamStore] process_message_content failed:",
                  e,
                );
                finalizeStream();
              });
          }
        } catch (e) {
          console.error("[ChatStreamStore] process_message_content failed:", e);
          finalizeStream();
        }
      } else {
        removeSessionStream(ownerId, ownerType, topicId, actualMessageId);
      }
    }
  };

  /**
   * 中止指定消息的生成
   */
  const stopMessage = async (
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
    messageId: string,
  ) => {
    console.log(
      `[ChatStreamStore] Sending interrupt signal for message: ${messageId}`,
    );
    try {
      await invoke("interruptRequest", {
        ownerId,
        ownerType,
        topicId,
        messageId,
      });
    } catch (e) {
      console.error(
        `[ChatStreamStore] Failed to interrupt stream for ${messageId}:`,
        e,
      );
    }
  };

  /**
   * 强行中止整个群组的接力赛回合
   */
  const stopGroupTurn = async (ownerId: string, topicId: string) => {
    console.log(
      `[ChatStreamStore] Global Group Interruption for topic: ${topicId}`,
    );
    // 在首个 await 前冻结目标集合，避免切换会话后误停新话题的流。
    const activeIds = Array.from(activeStreamingIds.value);
    try {
      await invoke("interruptGroupTurn", {
        ownerId,
        ownerType: "group",
        topicId,
      });
      if (activeIds.length > 0) {
        await Promise.all(
          activeIds.map((id) => stopMessage(ownerId, "group", topicId, id)),
        );
      }
    } catch (e) {
      console.error("[ChatStreamStore] Failed to stop group turn:", e);
    }
  };

  onScopeDispose(() => {
    recoveryMessageIds.clear();
    cleanupTimers.forEach(clearTimeout);
    cleanupTimers.clear();
    rAFPendingUpdates.forEach((up) => {
      if (up.animationFrameId !== null) {
        cancelAnimationFrame(up.animationFrameId);
      }
    });
    rAFPendingUpdates.clear();
    streamTerminalTombstones.clear();
  });

  const isRecovering = ref(false);

  /**
   * 检查并恢复被异常打断的活跃生成（冷启动自对齐与流接续）
   */
  const checkAndRecoverInterruptedStreams = async () => {
    if (isRecovering.value) return;
    isRecovering.value = true;

    // 1. 本地扫表：无网状态下也能运行
    let activeGens: ActiveGenerationDto[] = [];
    try {
      activeGens = await invoke<ActiveGenerationDto[]>("get_active_generations");
    } catch (e) {
      console.error("[ChatStreamStore] Failed to get active generations:", e);
      isRecovering.value = false;
      return;
    }

    if (!activeGens || activeGens.length === 0) {
      isRecovering.value = false;
      return;
    }

    try {
      console.log(
        `[ChatStreamStore] Found ${activeGens.length} interrupted active generations:`,
        activeGens,
      );
      // 必须在注入恢复占位对象前冻结；否则冷启动消息会被恒定误判为 warm。
      const warmMessageIds = new Set(activeStreamMessages.keys());
      const recoveryUnreadUpdates: Promise<void>[] = [];

      // 2. UI 预处理：在内存中将消息标记为 reconnecting，让用户在界面上看到“重连中”
      for (const gen of activeGens) {
        const { msgId, topicId, ownerId, ownerType, agentId, agentName } = gen;
        if (ownerType !== "agent" && ownerType !== "group") continue;
        const messageKey = streamMessageMapKey(
          ownerId,
          ownerType,
          topicId,
          msgId,
        );

        let msg = activeStreamMessages.get(messageKey);
        if (!msg) {
          const historyStore = useChatHistoryStore();
          const currentKey = sessionStore.currentConversationKey;
          const isCurrentConversation = Boolean(
            currentKey &&
              currentKey.topicId === topicId &&
              currentKey.ownerId === ownerId &&
              currentKey.ownerType === ownerType,
          );
          const existingMsg = isCurrentConversation
            ? historyStore.currentChatHistory.find((x) => x.id === msgId)
            : undefined;

          if (existingMsg) {
            msg = existingMsg;
            msg.isReconnecting = true;
          } else {
            msg = reactive<ChatMessage>({
              id: msgId,
              role: "assistant",
              name: agentName ?? undefined,
              content: "",
              timestamp: gen.createdAt || Date.now(),
              isThinking: false,
              isReconnecting: true,
              agentId: agentId || (ownerType === "agent" ? ownerId : undefined),
              groupId: ownerType === "group" ? ownerId : undefined,
              isGroupMessage: ownerType === "group",
              shell: computeShell({
                role: "assistant",
                agentId: agentId || (ownerType === "agent" ? ownerId : undefined),
              }),
            });

            // 如果是当前展示的话题，且历史中没有，立即推入历史中展示
            if (isCurrentConversation) {
              historyStore.currentChatHistory.push(msg);
              historyStore.currentChatHistory.sort(compareMessageOrder);
            }
          }
          activeStreamMessages.set(messageKey, msg);
        } else {
          msg.isReconnecting = true;
        }
        addSessionStream(ownerId, ownerType, topicId, msgId);
      }

      for (const gen of activeGens) {
        const { msgId, topicId, ownerId, ownerType } = gen;
        if (ownerType !== "agent" && ownerType !== "group") continue;
        const messageKey = streamMessageMapKey(
          ownerId,
          ownerType,
          topicId,
          msgId,
        );
        const currentKey = sessionStore.currentConversationKey;
        const recoveryKey: ConversationKey | null =
          currentKey &&
          currentKey.topicId === topicId &&
          currentKey.ownerId === ownerId &&
          currentKey.ownerType === ownerType
            ? {
                ownerId: currentKey.ownerId,
                ownerType: currentKey.ownerType,
                topicId: currentKey.topicId,
                epoch: currentKey.epoch,
              }
            : null;

        if (recoveryMessageIds.has(messageKey)) continue;
        recoveryMessageIds.add(messageKey);

        const isWarm = warmMessageIds.has(messageKey);
        if (!isWarm && ownerType === "agent" && !recoveryKey) {
          recoveryUnreadUpdates.push(
            topicStore.setTopicUnread(ownerId, ownerType, topicId, true),
          );
        }
        let msg = activeStreamMessages.get(messageKey);
        const originalContent = msg?.content || "";
        const originalBlocks = msg?.blocks ? [...msg.blocks] : [];

        if (!msg) {
          // 说明是冷启动后第一次加载或者流不在活跃池中
          const historyStore = useChatHistoryStore();
          const existing = recoveryKey
            ? historyStore.currentChatHistory.find((x) => x.id === msgId)
            : undefined;
          if (existing) {
            msg = existing;
            activeStreamMessages.set(messageKey, msg);
          }
        }

        const recoveryMessage = msg;
        if (recoveryMessage) {
          recoveryMessage.isThinking = false;
          if (!isWarm) {
            // 冷接续没有旧 AST 基线，必须从 helper 的事件 0 完整回放。
            recoveryMessage.content = "";
            recoveryMessage.blocks = [];
          }
        }

        const streamChannel = new Channel<StreamEventDto>();
        streamChannel.onmessage = (event) =>
          processStreamEvent(event, {
            onMessageCreated: (m, tid) => {
              const historyStore = useChatHistoryStore();
              if (
                recoveryKey &&
                tid === recoveryKey.topicId &&
                sessionStore.isConversationCurrent(recoveryKey) &&
                !historyStore.currentChatHistory.some((x) => x.id === m.id)
              ) {
                historyStore.currentChatHistory.push(m);
                historyStore.currentChatHistory.sort(compareMessageOrder);
              }
            },
            onStreamFinished: (_mid, tid) => {
              if (
                recoveryKey &&
                tid === recoveryKey.topicId &&
                sessionStore.isConversationCurrent(recoveryKey)
              ) {
                const historyStore = useChatHistoryStore();
                historyStore.summarizeTopic();
              }
            },
          });

        // 后端在一个 owner lease 内先判定 durable 状态，再决定是否需要网络接续。
        // navigator.onLine 不能前置否决：Finalizing/Interrupted 等状态必须可离线收口。
        // 扫描门闩可以释放，但 recoveryMessageIds 会覆盖整个长连接生命周期。
        void invoke<RecoveryResultDto>("recover_active_generation", {
          ownerId,
          ownerType,
          topicId,
          msgId,
          streamChannel,
          isWarm,
        })
          .then((res) => {
            console.log(
              `[ChatStreamStore] Recovery status for ${msgId}: ${String(res?.status ?? "unknown")}`,
            );
            if (res.status === "already_running") {
              if (recoveryMessage) recoveryMessage.isReconnecting = false;
              return;
            }

            if (recoveryMessage) {
              recoveryMessage.isReconnecting = false;
              recoveryMessage.isThinking = false;
              if (typeof res.content === "string") {
                recoveryMessage.content = res.content;
                invoke<ContentBlock[]>("process_message_content", {
                  content: recoveryMessage.content,
                })
                  .then((compiledBlocks) => {
                    recoveryMessage.blocks = compiledBlocks;
                  })
                  .catch((e) => {
                    console.error(
                      "[ChatStreamStore] Failed to compile recovered message:",
                      e,
                    );
                  });
              }
              if (res.status === "failed" || res.status === "not_found") {
                recoveryMessage.finishReason = "error";
              }
            }
            removeSessionStream(ownerId, ownerType, topicId, msgId);

            if (
              recoveryKey &&
              sessionStore.isConversationCurrent(recoveryKey)
            ) {
              const historyStore = useChatHistoryStore();
              if (
                !historyStore.currentChatHistory.some((x) => x.id === msgId)
              ) {
                void historyStore.loadHistory(ownerId, ownerType, topicId);
              }
            }
          })
          .catch((err) => {
            console.error(
              `[ChatStreamStore] Failed to recover stream for ${msgId}:`,
              err,
            );
            if (recoveryMessage) {
              recoveryMessage.isReconnecting = false;
              recoveryMessage.finishReason = "error";
              recoveryMessage.content =
                originalContent + "\n\n> VCP流式错误: 接续失败";
              recoveryMessage.blocks = originalBlocks;
            }
            removeSessionStream(ownerId, ownerType, topicId, msgId);
          })
          .finally(() => {
            recoveryMessageIds.delete(messageKey);
          });
      }

      if (recoveryUnreadUpdates.length > 0) {
        await Promise.allSettled(recoveryUnreadUpdates);
      }

      const selectedOwner = sessionStore.currentSelectedItem;
      if (
        selectedOwner &&
        (selectedOwner.type === "agent" || selectedOwner.type === "group") &&
        activeGens.some(
          (generation) =>
            generation.ownerId === selectedOwner.id &&
            generation.ownerType === selectedOwner.type,
        )
      ) {
        await topicStore
          .loadTopicList(selectedOwner.id, selectedOwner.type)
          .catch((error) => {
            console.error(
              "[ChatStreamStore] Failed to reconcile topic list after recovery:",
              error,
            );
          });
      }
    } catch (e) {
      console.error("[ChatStreamStore] Cloud sync failed during recovery:", e);
    } finally {
      isRecovering.value = false;
    }
  };

  return {
    streamingMessageKey,
    sessionActiveStreams,
    activeStreamMessages,
    activeStreamingIds,
    activeStreamKeySet,
    isMessageActive,
    isMessageActiveInSession,
    getActiveStreamMessage,
    isGroupGenerating,
    computeShell,
    addSessionStream,
    removeSessionStream,
    requestAuroraSnapshot,
    processStreamEvent,
    stopMessage,
    stopGroupTurn,
    checkAndRecoverInterruptedStreams,
  };
});
