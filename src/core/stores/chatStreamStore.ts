import { defineStore } from "pinia";
import { ref, computed, reactive, onScopeDispose } from "vue";
import { invoke, type Channel } from "@tauri-apps/api/core";

import { useChatSessionStore } from "./chatSessionStore";
import { useAssistantStore } from "./assistant";
import { useAvatarStore } from "./avatar";
import { useTopicStore } from "./topicListManager";
import { useChatHistoryStore } from "./chatHistoryStore";
import { useNotificationStore } from "./notification";
import { useChatPreferencesStore } from "../../features/chat/preferences/store";
import { triggerMessageHaptic } from "../utils/nativeHaptics";
import type {
  ActiveGenerationDto,
  AuroraRecoverySnapshot,
  ChatMessage,
  ContentBlock,
  MarkdownNode,
  MessageShell,
  InterruptRequestDto,
  ReconnectChatStreamsDto,
  StreamBlock,
  StreamEventDto,
  TailFrame,
  TailRenderMode,
  TailTextOp,
} from "../types/chat";
import type { ConversationOwnerType } from "./chatSessionStore";

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

// 快照提供完整正文基线，不能让先前未消费的 append/replace 继续代表它。
function buildTailSnapshotOp(block: StreamBlock | null, mode: TailRenderMode): TailTextOp {
  if (!block) return { op: "clear" };
  return {
    op: "replace",
    content: block.content ?? "",
    hash: block.hash,
    mode,
    blockType: block.type === "thought" || block.type === "html-preview" ? block.type : "markdown",
    thoughtTheme: block.type === "thought" ? block.theme : undefined,
  };
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

  // 核心：记录每个完整会话是否处于活动流状态。
  const sessionActiveStreams = ref<Record<string, string[]>>({});

  // 全局活跃流消息池：按完整消息身份存储正在生成的响应对象。
  // 无论是在前台还是后台，流式消息都从此池中获取，保证响应式链路不断裂
  const activeStreamMessages = reactive<Map<string, ChatMessage>>(new Map());
  const streamTerminalTombstones = new Map<
    string,
    StreamTerminalTombstone
  >();
  const streamConsumerRevisions = new Map<string, number>();
  const conversationOperations = reactive(new Map<string, number>());
  const activeGroupTurns = reactive(new Map<string, string>());
  const groupConsumerRevisions = new Map<string, number>();
  const activeRequests = reactive(new Map<string, Set<string>>());
  const requestConsumerRevisions = new Map<string, number>();
  const MAX_REQUEST_ACTIVITY_REVISIONS = 1024;
  const activityInventoryKnown = ref(false);
  let reconnectChannel: Channel<StreamEventDto> | null = null;
  const connectionId = crypto.randomUUID();
  let connectionEpoch = 0;
  let reconnectPromise: Promise<void> | null = null;
  let receiptEventConversations: Set<string> | null = null;
  let receiptEventMessages: Set<string> | null = null;
  let receiptRequestKeys: Set<string> | null = null;
  let disposed = false;

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
  interface PendingRenderUpdate {
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
  const rAFPendingUpdates = new Map<string, PendingRenderUpdate>();

  // rAF 与强制 flush 提交同一组字段；null 表示本批未更新，tailBlockChanged 区分清空。
  const commitPendingRender = (message: ChatMessage, update: PendingRenderUpdate) => {
    if (update.content !== null) message.content = update.content;
    if (update.blocks !== null) message.blocks = update.blocks;
    if (update.tailContent !== null) message.tailContent = update.tailContent;
    if (update.tailBlockChanged) message.tailBlock = update.tailBlock ?? undefined;
    if (update.tailOp !== null) message.tailOp = update.tailOp;
    if (update.tailSnapshot !== null) message.tailSnapshot = update.tailSnapshot;
    if (update.tailFrame !== null) message.tailFrame = update.tailFrame;
  };

  // 只清除待提交投影；流身份、连续性游标和恢复原因由各自的接管路径处理。
  const clearPendingRender = (update: PendingRenderUpdate) => {
    update.content = null;
    update.blocks = null;
    update.tailContent = null;
    update.tailBlock = null;
    update.tailBlockChanged = false;
    update.tailOp = null;
    update.tailFrame = null;
    update.tailSnapshot = null;
  };

  // Snapshot 完整覆盖渲染基线；完整 blocks 不带 tail，因而同时释放旧尾部。
  const replaceRenderProjection = (
    message: ChatMessage,
    blocks: ContentBlock[],
    tail?: { block: StreamBlock | null; mode: TailRenderMode; snapshot: MarkdownNode[]; frame: TailFrame },
  ) => {
    message.blocks = blocks;
    message.tailContent = tail?.block?.content ?? "";
    message.tailBlock = tail?.block ?? undefined;
    message.tailOp = tail ? buildTailSnapshotOp(tail.block, tail.mode) : undefined;
    message.tailSnapshot = tail?.snapshot;
    message.tailFrame = tail?.frame;
  };

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
        if (msg) commitPendingRender(msg, up);
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
      if (m) commitPendingRender(m, up);

      // 重置下一次绘制前的合并暂存状态。
      clearPendingRender(up);
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
        if (disposed || hasStreamTerminalTombstone(messageKey)) return false;
        const message = activeStreamMessages.get(messageKey);
        const pending = rAFPendingUpdates.get(messageKey);
        if (!message || !pending) return false;

        const source = pending.content ?? message.content ?? "";
        const consumerRevision = streamConsumerRevisions.get(messageKey);
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

        if (disposed || hasStreamTerminalTombstone(messageKey)) return false;
        const latestMessage = activeStreamMessages.get(messageKey);
        const latestPending = rAFPendingUpdates.get(messageKey);
        if (!latestMessage || !latestPending) return false;
        const latestSource = latestPending.content
          ?? latestMessage.content
          ?? "";
        const latestCursor = latestPending.tailCursor
          ?? cursorFromFrame(latestMessage.tailFrame);
        if (streamConsumerRevisions.get(messageKey) !== consumerRevision
          || latestSource !== source || !cursorsEqual(latestCursor, cursor)) {
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
        replaceRenderProjection(latestMessage, snapshot.stableBlocks, {
          block: tailBlock,
          mode,
          snapshot: snapshot.tailSnapshot,
          frame: resetFrame,
        });

        clearPendingRender(latestPending);
        latestPending.needsSnapshotReason = null;
        return true;
      }
    })().finally(() => {
      if (auroraSnapshotJobs.get(messageKey) === job) auroraSnapshotJobs.delete(messageKey);
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
  const chatPreferences = useChatPreferencesStore();

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

  // 覆盖请求持久化/上下文准备以及首个 thinking 事件前的窗口。
  function beginConversationOperation(
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
  ): () => void {
    const key = conversationMapKey(ownerId, ownerType, topicId);
    conversationOperations.set(key, (conversationOperations.get(key) ?? 0) + 1);
    let released = false;
    return () => {
      if (released) return;
      released = true;
      const remaining = (conversationOperations.get(key) ?? 0) - 1;
      if (remaining > 0) conversationOperations.set(key, remaining);
      else conversationOperations.delete(key);
    };
  }

  function isConversationBusy(
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
  ): boolean {
    const key = conversationMapKey(ownerId, ownerType, topicId);
    return !activityInventoryKnown.value
      || conversationOperations.has(key)
      || activeGroupTurns.has(key)
      || activeRequests.has(key)
      || Boolean(sessionActiveStreams.value[key]?.length);
  }

  function isConversationPreparing(
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
  ): boolean {
    const key = conversationMapKey(ownerId, ownerType, topicId);
    return conversationOperations.has(key) || activeRequests.has(key);
  }

  function isOwnerBusy(ownerId: string, ownerType: ConversationOwnerType): boolean {
    if (!activityInventoryKnown.value) return true;
    const keys = new Set([
      ...conversationOperations.keys(),
      ...activeGroupTurns.keys(),
      ...activeRequests.keys(),
      ...Object.keys(sessionActiveStreams.value),
    ]);
    for (const key of keys) {
      const [type, id] = JSON.parse(key) as [ConversationOwnerType, string, string];
      if (type === ownerType && id === ownerId) return true;
    }
    return false;
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
    return activeGroupTurns.has(key) || Boolean(sessionActiveStreams.value[key]?.length);
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
        streamConsumerRevisions.delete(messageKey);
        clearRAFUpdate(messageKey, false);
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
    discardImmediately = false,
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
    if (discardImmediately) {
      activeStreamMessages.delete(messageKey);
      streamConsumerRevisions.delete(messageKey);
      clearRAFUpdate(messageKey, false);
      return;
    }

    // 同时从全局池中移除 (延迟移除，确保 finalizeStream 能拿到对象)
    const cleanupTimer = setTimeout(() => {
      cleanupTimers.delete(cleanupTimer);
      if (!activeStreamKeySet.value.has(messageKey)) {
        activeStreamMessages.delete(messageKey);
        streamConsumerRevisions.delete(messageKey);
        clearRAFUpdate(messageKey, false); // 漏洞 2 修复：延迟清理时，强制安全注销 rAF 帧，杜绝句柄泄露
      }
    }, 1000);
    cleanupTimers.add(cleanupTimer);
  };

  // 只在后端删除已提交后调用；同一消息的迟到事件、快照和恢复回调全部失效。
  const forgetDeletedMessage = (
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
    messageId: string,
  ) => {
    const messageKey = streamMessageMapKey(ownerId, ownerType, topicId, messageId);
    recordStreamTerminalTombstone(messageKey);
    if (streamingMessageKey.value === messageKey) streamingMessageKey.value = null;
    removeSessionStream(ownerId, ownerType, topicId, messageId, true);
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
    if (disposed) return;
    const actualMessageId = event.messageId || "";
    const { type, context } = event;
    const ctx = context || {};
    const topicId = ctx.topicId;
    const ownerType = ctx.ownerType;
    const ownerId = ctx.ownerId;

    if (
      !topicId ||
      !ownerId ||
      (ownerType !== "agent" && ownerType !== "group")
    ) return;
    if (!Number.isSafeInteger(event.consumerRevision) || event.consumerRevision <= 0) return;
    const conversationKey = conversationMapKey(ownerId, ownerType, topicId);
    if (type === "request_activity") {
      if (typeof ctx.requestId !== "string") return;
      const requestKey = streamMessageMapKeyFromConversation(conversationKey, ctx.requestId);
      const previousRevision = requestConsumerRevisions.get(requestKey) ?? 0;
      if (event.consumerRevision < previousRevision) return;
      requestConsumerRevisions.set(requestKey, event.consumerRevision);
      receiptRequestKeys?.add(requestKey);
      const activity = event.chunk as { active?: boolean } | null;
      if (activity?.active === true) {
        if (!activeRequests.has(conversationKey)) activeRequests.set(conversationKey, new Set());
        activeRequests.get(conversationKey)!.add(ctx.requestId);
      } else if (activity?.active === false) {
        const requests = activeRequests.get(conversationKey);
        requests?.delete(ctx.requestId);
        if (requests?.size === 0) activeRequests.delete(conversationKey);
      }
      // 仅保留近期已结束 attempt 的交付序号；正在执行的 owner 不被淘汰。
      if (requestConsumerRevisions.size > MAX_REQUEST_ACTIVITY_REVISIONS) {
        for (const key of requestConsumerRevisions.keys()) {
          if (requestConsumerRevisions.size <= MAX_REQUEST_ACTIVITY_REVISIONS) break;
          const [scope, requestId] = JSON.parse(key) as [string, string];
          if (!activeRequests.get(scope)?.has(requestId)) requestConsumerRevisions.delete(key);
        }
      }
      return;
    }
    if (type === "group_turn") {
      if (ownerType !== "group" || typeof ctx.turnId !== "string") return;
      const previousRevision = groupConsumerRevisions.get(conversationKey) ?? 0;
      if (event.consumerRevision < previousRevision) return;
      receiptEventConversations?.add(conversationKey);
      groupConsumerRevisions.set(conversationKey, event.consumerRevision);
      const activity = event.chunk as { active?: boolean } | null;
      if (activity?.active === true) activeGroupTurns.set(conversationKey, ctx.turnId);
      else if (activity?.active === false && activeGroupTurns.get(conversationKey) === ctx.turnId) {
        activeGroupTurns.delete(conversationKey);
      }
      return;
    }
    if (!actualMessageId) return;
    const messageKey = streamMessageMapKey(
      ownerId,
      ownerType,
      topicId,
      actualMessageId,
    );

    if (hasStreamTerminalTombstone(messageKey)) return;
    const currentRevision = streamConsumerRevisions.get(messageKey) ?? 0;
    if (event.consumerRevision < currentRevision) return;
    receiptEventMessages?.add(messageKey);
    if (event.consumerRevision > currentRevision) {
      clearRAFUpdate(messageKey, false);
    }
    streamConsumerRevisions.set(messageKey, event.consumerRevision);
    if (type === "end") {
      recordStreamTerminalTombstone(messageKey);
    }

    let msg = activeStreamMessages.get(messageKey);
    if (!msg) {
      const history = useChatHistoryStore();
      const loaded = history.loadedConversationKey;
      if (loaded && loaded.ownerId === ownerId && loaded.ownerType === ownerType
        && loaded.topicId === topicId && sessionStore.isConversationCurrent(loaded)) {
        msg = history.currentChatHistory.find(message => message.id === actualMessageId);
        if (msg) activeStreamMessages.set(messageKey, msg);
      }
    }
    const consumerRevision = streamConsumerRevisions.get(messageKey);
    const ownsConsumer = () => !disposed
      && streamConsumerRevisions.get(messageKey) === consumerRevision;
    const isNewStream = !msg;

    if (isNewStream) {
      msg = reactive<ChatMessage>({
        id: actualMessageId,
        role: "assistant",
        name: ctx.agentName,
        content: "",
        timestamp: event.timestamp ?? Date.now(),
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
      const isRecoveryStatus = type === "reconnecting" || type === "connected" || type === "stopping";
      if (!isRecoveryStatus) topicStore.incrementTopicMsgCount(ownerId, ownerType, topicId);
      const currentKey = sessionStore.currentConversationKey;
      if (!isRecoveryStatus && (
        !currentKey ||
        currentKey.ownerId !== ownerId ||
        currentKey.ownerType !== ownerType ||
        currentKey.topicId !== topicId
      )) {
        void topicStore
          .incrementTopicUnreadCount(ownerId, ownerType, topicId)
          .catch(() => {});
      }
      if (isRecoveryStatus && ownerType === "agent" && (
        currentKey?.ownerId !== ownerId || currentKey.ownerType !== ownerType || currentKey.topicId !== topicId
      )) {
        void topicStore.setTopicUnread(ownerId, ownerType, topicId, true).catch(() => {});
      }

      // 回调：通知 UI 列表插入新消息
      if (callbacks?.onMessageCreated) {
        callbacks.onMessageCreated(msg!, topicId);
      }
    }

    // 维护流状态
    if (type === "thinking") {
      msg!.isThinking = true;
      msg!.isReconnecting = false;
      msg!.streamRecoveryState = undefined;
      addSessionStream(ownerId, ownerType, topicId, actualMessageId);
      if (!streamingMessageKey.value) {
        streamingMessageKey.value = messageKey;
      }
    } else if (type === "reconnecting" || type === "connected" || type === "stopping") {
      msg!.isThinking = false;
      msg!.isReconnecting = type === "reconnecting";
      msg!.streamRecoveryState = type === "reconnecting" ? "reconnecting" : undefined;
      // 停止意图属于请求 owner，接管成功不会把它重新解释成继续生成。
      if (type === "stopping") {
        msg!.isStopping = true;
        clearRAFUpdate(messageKey, true);
        if (typeof event.content === "string" && event.content !== msg!.content) {
          const content = event.content;
          msg!.content = content;
          void invoke<ContentBlock[]>("process_message_content", { content })
            .then(blocks => {
              if (!ownsConsumer() || hasStreamTerminalTombstone(messageKey)
                || !msg!.isStopping || msg!.content !== content || rAFPendingUpdates.has(messageKey)) return;
              replaceRenderProjection(msg!, blocks);
            })
            .catch(error => console.error("[ChatStreamStore] Failed to render stopping partial:", error));
        }
      }
      if (msg!.finishReason === "error") msg!.finishReason = undefined;
      addSessionStream(ownerId, ownerType, topicId, actualMessageId);
    } else if (type === "aurora") {
      msg!.isThinking = false;
      msg!.isReconnecting = false;
      msg!.streamRecoveryState = undefined;
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
            clearPendingRender(update);
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
          update.tailOp = buildTailSnapshotOp(update.tailBlock, snapshotMode);
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
      // error 表示尚未 durable（终态提交失败或 Helper 接管中断）。保留 partial 和 active owner，
      // 不写终态 tombstone；后续 recovery 的权威 end 仍必须能够进入。
      clearRAFUpdate(messageKey, true);
      const contentChanged = typeof event.content === "string" && event.content !== msg!.content;
      if (typeof event.content === "string") msg!.content = event.content;
      msg!.isThinking = false;
      msg!.isReconnecting = false;
      const wasWaiting = msg!.streamRecoveryState === "waiting_retry";
      msg!.streamRecoveryState = "waiting_retry";
      addSessionStream(ownerId, ownerType, topicId, actualMessageId);
      if (contentChanged) {
        const content = msg!.content || "";
        void invoke<ContentBlock[]>("process_message_content", { content })
          .then(blocks => {
            if (!ownsConsumer() || hasStreamTerminalTombstone(messageKey)
              || msg!.streamRecoveryState !== "waiting_retry" || msg!.content !== content) return;
            replaceRenderProjection(msg!, blocks);
          })
          .catch(error => console.error("[ChatStreamStore] Failed to render recoverable partial:", error));
      }
      if (!wasWaiting && !msg!.isStopping) {
        useNotificationStore().addNotification({
          id: `stream-recovery:${messageKey}`,
          type: "error",
          title: "流式接续失败",
          message: event.error || "连接暂不可用，等待重试",
          toastOnly: true,
        });
      }
    } else if (type === "end") {
      const finishReason = event.finishReason;
      // Only a live response completing normally is a new completion cue. Durable
      // history replays and user cancellation still follow the same rendering owner.
      const playCompletionFeedback = isMessageActive(ownerId, ownerType, topicId, actualMessageId)
        && ctx.replayed !== true && !event.error
        && (finishReason === 'completed' || finishReason === 'stop' || finishReason === 'length');
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
        msg!.streamRecoveryState = undefined;
        msg!.isStopping = false;
        if (event.timestamp) {
          msg!.timestamp = event.timestamp;
        }

        // 定义闭环终结函数，保证在 blocks 赋值完成后才移除活动流状态
        const finalizeStream = (blocks?: ContentBlock[]) => {
          if (!ownsConsumer()) return;
          // 解析失败时保留已有展示；只有完整结果到达才释放旧尾部。
          if (blocks) replaceRenderProjection(msg!, blocks);
          removeSessionStream(ownerId, ownerType, topicId, actualMessageId);
          if (callbacks?.onStreamFinished) {
            callbacks.onStreamFinished(actualMessageId, topicId);
          }
          if (playCompletionFeedback && chatPreferences.messageHapticsEnabled) triggerMessageHaptic('complete');
        };

        try {
          // 如果后端已经带回了预渲染好的 blocks，直接使用，跳过冗余解析
          if (event.blocks) {
            finalizeStream(event.blocks);
          } else {
            invoke<ContentBlock[]>("process_message_content", {
              content: msg!.content || "",
            })
              .then((compiledBlocks) => {
                finalizeStream(compiledBlocks);
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
    const message = getActiveStreamMessage(ownerId, ownerType, topicId, messageId);
    const messageKey = streamMessageMapKey(ownerId, ownerType, topicId, messageId);
    if (message) message.isStopping = true;
    try {
      const receipt = await invoke<InterruptRequestDto>("interruptRequest", {
        ownerId,
        ownerType,
        topicId,
        messageId,
      });
      if (disposed) return;
      if (receipt?.status === "completed" && !hasStreamTerminalTombstone(messageKey)
        && isMessageActive(ownerId, ownerType, topicId, messageId)) {
        // 收据只证明后端已提交；缺失的正文仍经统一连接/历史对账取得。
        await checkAndRecoverInterruptedStreams();
      }
    } catch (e) {
      console.error(
        `[ChatStreamStore] Failed to interrupt stream for ${messageId}:`,
        e,
      );
      if (!disposed && isMessageActive(ownerId, ownerType, topicId, messageId)) {
        useNotificationStore().addNotification({
          id: `stream-stop:${messageKey}`,
          type: "warning",
          title: "停止待确认",
          message: e instanceof Error ? e.message : String(e),
          toastOnly: true,
        });
      }
    }
  };

  /**
   * 强行中止整个群组的接力赛回合
   */
  const stopGroupTurn = async (ownerId: string, topicId: string) => {
    const conversationKey = conversationMapKey(ownerId, "group", topicId);
    for (const id of sessionActiveStreams.value[conversationKey] ?? []) {
      const message = getActiveStreamMessage(ownerId, "group", topicId, id);
      if (message) message.isStopping = true;
    }
    try {
      await invoke("interruptGroupTurn", {
        ownerId,
        ownerType: "group",
        topicId,
      });
    } catch (e) {
      console.error("[ChatStreamStore] Failed to stop group turn:", e);
      if (!disposed && (activeGroupTurns.has(conversationKey) || sessionActiveStreams.value[conversationKey]?.length)) {
        useNotificationStore().addNotification({
          id: `stream-stop-group:${conversationKey}`,
          type: "warning",
          title: "停止待确认",
          message: e instanceof Error ? e.message : String(e),
          toastOnly: true,
        });
      }
    }
  };

  onScopeDispose(() => {
    disposed = true;
    connectionEpoch += 1;
    reconnectChannel = null;
    receiptEventConversations = null;
    receiptEventMessages = null;
    receiptRequestKeys = null;
    conversationOperations.clear();
    activeGroupTurns.clear();
    groupConsumerRevisions.clear();
    activeRequests.clear();
    requestConsumerRevisions.clear();
    cleanupTimers.forEach(clearTimeout);
    cleanupTimers.clear();
    rAFPendingUpdates.forEach((up) => {
      if (up.animationFrameId !== null) {
        cancelAnimationFrame(up.animationFrameId);
      }
    });
    rAFPendingUpdates.clear();
    streamTerminalTombstones.clear();
    streamConsumerRevisions.clear();
  });

  const connectionError = ref<string | null>(null);

  const createOutputChannel = (): Channel<StreamEventDto> => {
    const epoch = ++connectionEpoch;
    const channel = useChatHistoryStore().makeStreamChannel(null);
    const handleEvent = channel.onmessage;
    channel.onmessage = event => {
      if (!disposed && epoch === connectionEpoch) handleEvent(event);
    };
    reconnectChannel = channel;
    return channel;
  };

  /**
   * 同一 WebView 只持有一个输出连接。Rust 安排接管；每条生成的结果只从 Channel 归约。
   */
  const checkAndRecoverInterruptedStreams = (): Promise<void> => {
    if (disposed) return Promise.resolve();
    if (reconnectPromise) return reconnectPromise;
    // Tauri 每次反序列化 Channel 都新建原生序号，不能重复传递同一个 JS Channel。
    // 注册后只传连接身份，让 Rust 复用最初注册的 Channel 实例及其有序序号。
    let registrationChannel = reconnectChannel ? undefined : createOutputChannel();
    let epoch = connectionEpoch;
    const previousActiveKeys = new Set(activeStreamKeySet.value);
    const eventConversations = new Set<string>();
    const eventMessages = new Set<string>();
    const requestEvents = new Set<string>();
    receiptEventConversations = eventConversations;
    receiptEventMessages = eventMessages;
    receiptRequestKeys = requestEvents;

    reconnectPromise = (async () => {
      try {
        let receipt: ReconnectChatStreamsDto;
        try {
          receipt = await invoke<ReconnectChatStreamsDto>("reconnect_chat_streams", {
            connectionId,
            ...(registrationChannel ? { channel: registrationChannel } : {}),
          });
        } catch (error) {
          if (disposed || epoch !== connectionEpoch) return;
          if (String(error) !== "connection_not_registered" || registrationChannel) throw error;
          registrationChannel = createOutputChannel();
          epoch = connectionEpoch;
          receipt = await invoke<ReconnectChatStreamsDto>("reconnect_chat_streams", {
            connectionId, channel: registrationChannel,
          });
        }
        if (disposed || epoch !== connectionEpoch) return;
        if (!receipt || !Array.isArray(receipt.generations) || !Array.isArray(receipt.groupTurns)
          || !Array.isArray(receipt.activeRequests)) {
          throw new Error("聊天连接未返回有效的活动状态");
        }
        activityInventoryKnown.value = true;
        connectionError.value = null;
        const pendingKeys = new Set<string>();
        for (const generation of receipt.generations) {
          if (generation.ownerType !== "agent" && generation.ownerType !== "group") continue;
          const { ownerId, ownerType, topicId, msgId } = generation;
          const messageKey = streamMessageMapKey(ownerId, ownerType, topicId, msgId);
          pendingKeys.add(messageKey);
          // 事件可先于收据抵达；收据从不覆盖新快照、待重试状态或持久终态。
          if (hasStreamTerminalTombstone(messageKey)) continue;
          hydratePendingReceipt(generation, ownerType, eventMessages.has(messageKey));
        }

        const liveTurns = new Map(receipt.groupTurns.map(turn => [
          conversationMapKey(turn.ownerId, "group", turn.topicId), turn.turnId,
        ]));
        for (const key of activeGroupTurns.keys()) {
          if (!eventConversations.has(key) && !liveTurns.has(key)) activeGroupTurns.delete(key);
        }
        for (const [key, turnId] of liveTurns) {
          if (!eventConversations.has(key)) activeGroupTurns.set(key, turnId);
        }

        const liveRequestKeys = new Set<string>();
        for (const request of receipt.activeRequests) {
          const key = conversationMapKey(request.ownerId, request.ownerType, request.topicId);
          const requestKey = streamMessageMapKeyFromConversation(key, request.requestId);
          liveRequestKeys.add(requestKey);
          if (!requestEvents.has(requestKey)) {
            if (!activeRequests.has(key)) activeRequests.set(key, new Set());
            activeRequests.get(key)!.add(request.requestId);
          }
        }
        for (const [key, requests] of activeRequests) {
          for (const requestId of requests) {
            const requestKey = streamMessageMapKeyFromConversation(key, requestId);
            if (!liveRequestKeys.has(requestKey) && !requestEvents.has(requestKey)) requests.delete(requestId);
          }
          if (requests.size === 0) activeRequests.delete(key);
        }

        // end 可能在 WebView 离线期间已经提交。只清理本次连接前已有且不在收据中的状态，
        // 不把收据缺席解释为业务失败；当前界面的权威正文由历史重新取得。
        let reloadCurrentHistory = false;
        for (const messageKey of previousActiveKeys) {
          if (pendingKeys.has(messageKey) || eventMessages.has(messageKey)
            || hasStreamTerminalTombstone(messageKey)) continue;
          const [conversationKey, messageId] = JSON.parse(messageKey) as [string, string];
          const [ownerType, ownerId, topicId] = JSON.parse(conversationKey) as [ConversationOwnerType, string, string];
          const message = activeStreamMessages.get(messageKey);
          if (message) {
            clearRAFUpdate(messageKey, true);
            message.isThinking = false;
            message.isReconnecting = false;
            message.streamRecoveryState = undefined;
            message.isStopping = false;
          }
          // 清退旧交付身份，避免迟到帧重新插入已不 pending 的气泡；不推断业务 finishReason。
          recordStreamTerminalTombstone(messageKey);
          removeSessionStream(ownerId, ownerType, topicId, messageId, true);
          const current = sessionStore.currentConversationKey;
          if (current?.ownerId === ownerId && current.ownerType === ownerType
            && current.topicId === topicId) reloadCurrentHistory = true;
        }
        const current = sessionStore.currentConversationKey;
        if (reloadCurrentHistory && current) {
          await useChatHistoryStore().loadHistory(current.ownerId, current.ownerType, current.topicId);
        }
      } catch (error) {
        if (disposed || epoch !== connectionEpoch) return;
        connectionError.value = error instanceof Error ? error.message : String(error);
        useNotificationStore().addNotification({
          id: "chat-output-connection",
          type: "error",
          title: "聊天连接失败",
          message: `${connectionError.value}。返回前台或网络恢复后将重新连接。`,
          toastOnly: true,
        });
      } finally {
        if (receiptEventConversations === eventConversations) receiptEventConversations = null;
        if (receiptEventMessages === eventMessages) receiptEventMessages = null;
        if (receiptRequestKeys === requestEvents) receiptRequestKeys = null;
        reconnectPromise = null;
      }
    })();
    return reconnectPromise;
  };

  function hydratePendingReceipt(
    generation: ActiveGenerationDto,
    ownerType: ConversationOwnerType,
    preserveEventState: boolean,
  ): void {
    const { ownerId, topicId, msgId } = generation;
    const messageKey = streamMessageMapKey(ownerId, ownerType, topicId, msgId);
    const history = useChatHistoryStore();
    const loaded = history.loadedConversationKey;
    const isLoaded = loaded?.ownerId === ownerId && loaded.ownerType === ownerType
      && loaded.topicId === topicId && sessionStore.isConversationCurrent(loaded);
    let message = activeStreamMessages.get(messageKey);
    if (!message && isLoaded) message = history.currentChatHistory.find(item => item.id === msgId);
    if (!message) {
      message = reactive<ChatMessage>({
        id: msgId,
        role: "assistant",
        name: generation.agentName ?? undefined,
        content: "",
        timestamp: generation.createdAt,
        agentId: generation.agentId ?? (ownerType === "agent" ? ownerId : undefined),
        groupId: ownerType === "group" ? ownerId : undefined,
        isGroupMessage: ownerType === "group",
      });
    }
    // live owner 的 connected 可先于元数据抵达；只补展示身份，不覆盖它已交付的正文/状态。
    if (!message.name && generation.agentName) message.name = generation.agentName;
    if (!message.agentId && generation.agentId) message.agentId = generation.agentId;
    if (generation.createdAt > 0) message.timestamp = generation.createdAt;
    message.shell = computeShell({ role: message.role, agentId: message.agentId, name: message.name ?? undefined });
    activeStreamMessages.set(messageKey, message);
    if (isLoaded && !history.currentChatHistory.some(item => item.id === msgId)) {
      history.currentChatHistory.push(message);
      history.currentChatHistory.sort((left, right) =>
        left.timestamp - right.timestamp || left.id.localeCompare(right.id));
    }
    // 只有没有事件状态的冷占位才表示等待接管；已有消息状态由生产者继续控制。
    if (!preserveEventState && !streamConsumerRevisions.has(messageKey)) {
      message.isReconnecting = !generation.stopRequested;
      message.streamRecoveryState = generation.stopRequested ? undefined : "reconnecting";
    }
    if (!preserveEventState && generation.stopRequested) message.isStopping = true;
    addSessionStream(ownerId, ownerType, topicId, msgId);
  }

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
    beginConversationOperation,
    isConversationBusy,
    isConversationPreparing,
    isOwnerBusy,
    activityInventoryKnown,
    connectionError,
    computeShell,
    addSessionStream,
    removeSessionStream,
    forgetDeletedMessage,
    requestAuroraSnapshot,
    processStreamEvent,
    stopMessage,
    stopGroupTurn,
    checkAndRecoverInterruptedStreams,
  };
});
