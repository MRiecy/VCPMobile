import { defineStore } from "pinia";
import { ref, computed, reactive, onScopeDispose } from "vue";
import { invoke, Channel } from "@tauri-apps/api/core";

import { useChatSessionStore } from "./chatSessionStore";
import { useAssistantStore } from "./assistant";
import { useAvatarStore } from "./avatar";
import { useTopicStore } from "./topicListManager";
import { useChatHistoryStore } from "./chatHistoryStore";
import type { ChatMessage, MessageShell, TailFrame } from "../types/chat";
import type { ConversationKey } from "./chatSessionStore";

export const useChatStreamStore = defineStore("chatStream", () => {
  const streamingMessageId = ref<string | null>(null);

  // 核心：记录每个会话（itemId + topicId）是否处于活动流状态
  // 格式: "itemId:topicId" -> [messageId1, messageId2, ...]
  const sessionActiveStreams = ref<Record<string, string[]>>({});

  // 全局活跃流消息池：存储所有正在生成的响应对象 (messageId -> Reactive<ChatMessage>)
  // 无论是在前台还是后台，流式消息都从此池中获取，保证响应式链路不断裂
  const activeStreamMessages = reactive<Map<string, ChatMessage>>(new Map());
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

  function mergeTailFrame(
    existing: TailFrame | null,
    incoming: TailFrame,
    latestSnapshot?: any[],
    forceSnapshot = false,
  ): TailFrame {
    const incomingMutations = incoming.mutations || [];
    const snapshotFrame = (): TailFrame => ({
      ...incoming,
      reset: true,
      snapshot: latestSnapshot
        ? [...latestSnapshot]
        : incoming.snapshot
          ? [...incoming.snapshot]
          : undefined,
      mutations: [],
    });

    // 后台 WebView 的 rAF 可能长期停摆。此时只保留最新完整 AST 基线，
    // 不累计期间的每一条 diff；回到前台后单帧重建即可追上当前状态。
    if (forceSnapshot) {
      return snapshotFrame();
    }

    // 一个尚未刷入 DOM 的 reset 后续再收到增量时，直接把基线推进到最新完整节点。
    // 这样合并结果始终自洽，不需要保存 reset 之后的全部中间 diff。
    if (existing?.reset) {
      return snapshotFrame();
    }

    if (!existing || incoming.reset || incoming.epoch !== existing.epoch) {
      return {
        ...incoming,
        mutations: incoming.reset ? [] : [...incomingMutations],
        snapshot: incoming.snapshot ? [...incoming.snapshot] : undefined,
      };
    }

    const mutations = [
      ...(existing.reset ? [] : existing.mutations || []),
      ...incomingMutations,
    ];
    if (mutations.length > MAX_PENDING_TAIL_MUTATIONS) {
      return snapshotFrame();
    }

    return {
      ...incoming,
      reset: existing.reset || incoming.reset,
      snapshot: incoming.snapshot || existing.snapshot,
      mutations,
    };
  }

  const cleanupTimers = new Set<ReturnType<typeof setTimeout>>();

  // ===== rAF 30Hz 帧合并直推暂存池 =====
  // 记录每个消息最新的 Aurora 暂存数据，消灭定时器空转，硬件级防抖并实现30Hz降降基数
  const rAFPendingUpdates = new Map<
    string,
    {
      content: string | null;
      blocks: any[] | null;
      tailContent: string | null;
      tailBlock: any | null;
      tailFrame: TailFrame | null;
      tailSnapshot: any[] | null;
      animationFrameId: number | null;
      lastRenderTime: number;
    }
  >();
  const MIN_RENDER_INTERVAL_MS = 33.3; // 限制最大刷新频率为 30Hz

  /**
   * 物理防线：强行中止、强制同步刷新并安全清理指定消息的 rAF 帧状态，杜绝任何泄漏与闪烁
   */
  const clearRAFUpdate = (messageId: string, forceFlush = false) => {
    const up = rAFPendingUpdates.get(messageId);
    if (up) {
      if (up.animationFrameId !== null) {
        cancelAnimationFrame(up.animationFrameId);
        up.animationFrameId = null;
      }
      if (forceFlush) {
        const msg = activeStreamMessages.get(messageId);
        if (msg) {
          if (up.content !== null) msg.content = up.content;
          if (up.blocks !== null) msg.blocks = up.blocks;
          // 漏洞 1 修复：同步强刷收尾时，必须将暂存池中的 tail 字段强刷，绝不允许丢字闪烁
          if (up.tailContent !== null) msg.tailContent = up.tailContent;
          if (up.tailBlock !== undefined) msg.tailBlock = up.tailBlock;
          if (up.tailSnapshot !== null)
            msg.tailSnapshot = up.tailSnapshot as any;
          if (up.tailFrame !== null) msg.tailFrame = up.tailFrame;
        }
      }
      rAFPendingUpdates.delete(messageId);
    }
  };

  /**
   * 调度并申请 rAF 渲染，合并 data 和 aurora 的高频更新，在同一渲染帧内原子写入
   */
  const scheduleRAFUpdate = (messageId: string) => {
    const update = rAFPendingUpdates.get(messageId);
    if (!update || update.animationFrameId !== null) return;

    const runRenderLoop = () => {
      const up = rAFPendingUpdates.get(messageId);
      if (!up) return;

      const now = performance.now();
      const elapsed = now - up.lastRenderTime;

      if (elapsed >= MIN_RENDER_INTERVAL_MS) {
        // 满足 30Hz 时间间隔，以原子事务方式刷入 Vue 响应式数据
        const m = activeStreamMessages.get(messageId);
        if (m) {
          if (up.content !== null) m.content = up.content;
          if (up.blocks !== null) m.blocks = up.blocks;
          if (up.tailSnapshot !== null) m.tailSnapshot = up.tailSnapshot as any;
          if (up.tailFrame !== null) m.tailFrame = up.tailFrame;
          if (up.tailContent !== null) m.tailContent = up.tailContent;
          if (up.tailBlock !== undefined) m.tailBlock = up.tailBlock;
        }
        up.lastRenderTime = now;
        // 重置当前帧内的合并暂存状态
        up.content = null;
        up.blocks = null;
        up.tailContent = null;
        up.tailBlock = null;
        up.tailFrame = null;
        up.tailSnapshot = null;
        up.animationFrameId = null;
      } else {
        // 未到时间阀值，在下一物理帧继续尝试
        up.animationFrameId = requestAnimationFrame(runRenderLoop);
      }
    };

    update.animationFrameId = requestAnimationFrame(runRenderLoop);
  };

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
    const empty = "";
    if (msg.role === "user") {
      const userColor =
        avatarStore.getDominantColor("user", "user_avatar") || "rgb(226,54,56)";
      return {
        avatarColor: userColor,
        bubbleBorderColor: empty,
        bubbleBoxShadow: empty,
        displayName: msg.name || "User",
        isUser: true,
      };
    }
    const agent = msg.agentId
      ? assistantStore.agents.find((a) => a.id === msg.agentId)
      : undefined;
    return {
      avatarColor: agent?.avatarCalculatedColor || "",
      bubbleBorderColor: empty,
      bubbleBoxShadow: empty,
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

  const activeStreamIdSet = computed(() => {
    const ids = new Set<string>();
    for (const streams of Object.values(sessionActiveStreams.value)) {
      for (const id of streams) ids.add(id);
    }
    return ids;
  });

  function isMessageActive(messageId: string): boolean {
    return activeStreamIdSet.value.has(messageId);
  }

  function isMessageActiveInSession(
    ownerId: string,
    topicId: string,
    messageId: string,
  ): boolean {
    return (
      activeStreamSets.value[`${ownerId}:${topicId}`]?.has(messageId) ?? false
    );
  }

  // 兼容旧逻辑的计算属性
  const activeStreamingIds = computed(() => {
    if (!sessionStore.currentSelectedItem?.id || !sessionStore.currentTopicId)
      return new Set<string>();
    const key = `${sessionStore.currentSelectedItem.id}:${sessionStore.currentTopicId}`;
    return activeStreamSets.value[key] || new Set<string>();
  });

  const isGroupGenerating = computed(() => {
    if (
      !sessionStore.currentSelectedItem?.id ||
      !sessionStore.currentTopicId ||
      sessionStore.currentSelectedItem.type !== "group"
    )
      return false;
    const key = `${sessionStore.currentSelectedItem.id}:${sessionStore.currentTopicId}`;
    const streams = sessionActiveStreams.value[key];
    return streams ? streams.length > 0 : false;
  });

  // 全局流消息池上限，防止极端场景下 OOM
  const MAX_STREAM_MESSAGES = 100;

  const enforceStreamPoolLimit = () => {
    if (activeStreamMessages.size <= MAX_STREAM_MESSAGES) return;
    let remaining = activeStreamMessages.size - MAX_STREAM_MESSAGES;
    // 按插入顺序（Map 保持插入顺序）清理最旧的非活跃消息
    for (const [id] of activeStreamMessages) {
      if (remaining <= 0) break;
      // 只删除已完成的流（不在当前活跃会话中）
      if (!isMessageActive(id)) {
        activeStreamMessages.delete(id);
        remaining -= 1;
      }
    }
  };

  // 辅助方法：管理会话流状态
  const addSessionStream = (
    ownerId: string,
    topicId: string,
    messageId: string,
  ) => {
    const key = `${ownerId}:${topicId}`;
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
    topicId: string,
    messageId: string,
  ) => {
    const key = `${ownerId}:${topicId}`;
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
    // 同时从全局池中移除 (延迟移除，确保 finalizeStream 能拿到对象)
    const cleanupTimer = setTimeout(() => {
      cleanupTimers.delete(cleanupTimer);
      if (!activeStreamingIds.value.has(messageId)) {
        activeStreamMessages.delete(messageId);
        clearRAFUpdate(messageId, false); // 漏洞 2 修复：延迟清理时，强制安全注销 rAF 帧，杜绝句柄泄露
      }
    }, 1000);
    cleanupTimers.add(cleanupTimer);
  };

  const removeMessageFromAllSessions = (messageId: string) => {
    for (const [key, streams] of Object.entries(sessionActiveStreams.value)) {
      const remaining = streams.filter((id) => id !== messageId);
      if (remaining.length === streams.length) continue;
      if (remaining.length === 0) {
        delete sessionActiveStreams.value[key];
      } else {
        sessionActiveStreams.value[key] = remaining;
      }
    }

    const cleanupTimer = setTimeout(() => {
      cleanupTimers.delete(cleanupTimer);
      if (!isMessageActive(messageId)) {
        activeStreamMessages.delete(messageId);
        clearRAFUpdate(messageId, false);
      }
    }, 1000);
    cleanupTimers.add(cleanupTimer);
  };

  /**
   * 处理流式事件的核心逻辑 (会话隔离调度器)
   */
  const processStreamEvent = async (
    event: any,
    callbacks?: {
      onMessageCreated?: (msg: ChatMessage, topicId: string) => void;
      onStreamFinished?: (messageId: string, topicId: string) => void;
    },
  ) => {
    const actualMessageId = event.messageId || event.message_id || "";
    const { type, context } = event;
    const ctx = context || {};
    const topicId = ctx.topicId;
    const isGroup = !!ctx.isGroupMessage || !!ctx.groupId;
    const itemId = isGroup ? ctx.groupId : ctx.agentId || ctx.ownerId;

    if (!actualMessageId || !topicId || !itemId) return;

    let msg = activeStreamMessages.get(actualMessageId);
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
        groupId: ctx.groupId,
        isGroupMessage: !!ctx.isGroupMessage,
        shell: computeShell({
          role: "assistant",
          agentId: ctx.agentId,
          name: ctx.agentName,
        }),
      });
      activeStreamMessages.set(actualMessageId, msg!);

      topicStore.incrementTopicMsgCount(topicId);
      if (topicId !== sessionStore.currentTopicId) {
        topicStore.incrementTopicUnreadCount(topicId);
      }

      // 回调：通知 UI 列表插入新消息
      if (callbacks?.onMessageCreated) {
        callbacks.onMessageCreated(msg!, topicId);
      }
    }

    // 维护流状态
    if (type === "thinking") {
      msg!.isThinking = true;
      addSessionStream(itemId, topicId, actualMessageId);
      if (!streamingMessageId.value) {
        streamingMessageId.value = actualMessageId;
      }
    } else if (type === "aurora") {
      msg!.isThinking = false;
      addSessionStream(itemId, topicId, actualMessageId);

      const aurora = event.aurora;
      if (aurora) {
        recordStreamTrace({
          messageId: actualMessageId,
          auroraPayload: {
            stableChanged: aurora.stableChanged,
            stableBlocksCount: aurora.stableBlocks?.length || 0,
            stableBlocksHashes:
              aurora.stableBlocks?.map((b: any) => b.hash) || [],
            tailChanged: aurora.tailChanged,
            tailContent: aurora.tail || "",
            tailBlockType: aurora.tailBlock?.type || null,
            tailFrame: aurora.tailFrame
              ? {
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

        // 1. 初始化或获取该 messageId 的帧合并状态
        let update = rAFPendingUpdates.get(actualMessageId);
        if (!update) {
          update = {
            content: null,
            blocks: null,
            tailContent: null,
            tailBlock: null,
            tailFrame: null,
            tailSnapshot: null,
            animationFrameId: null,
            lastRenderTime: 0,
          };
          rAFPendingUpdates.set(actualMessageId, update);
        }

        // 2. 覆盖写入暂存数据（稀疏合并）
        if (typeof aurora.content === "string") {
          update.content = aurora.content;
        } else if (aurora.chunk) {
          const currentBase =
            update.content !== null ? update.content : msg!.content || "";
          update.content = currentBase + aurora.chunk;
        }
        if (aurora.stableChanged && aurora.stableBlocks) {
          update.blocks = aurora.stableBlocks;
        }
        if (aurora.tailFrame) {
          streamDebugLog(
            `[chatStreamStore] Received tailFrame seq=${aurora.tailFrame.frameSeq} mutations=${aurora.tailFrame.mutations?.length || 0} for ${actualMessageId}`,
          );
          const latestSnapshot =
            aurora.tailFrame.snapshot ||
            aurora.tailSnapshot ||
            aurora.tailBlock?.nodes;
          update.tailFrame = mergeTailFrame(
            update.tailFrame,
            aurora.tailFrame,
            latestSnapshot,
            typeof document !== "undefined" && document.hidden,
          );
          if (aurora.tailFrame.snapshot) {
            update.tailSnapshot = aurora.tailFrame.snapshot as any[];
          }
        }
        if (aurora.tailSnapshot) {
          update.tailSnapshot = aurora.tailSnapshot as any[];
        }
        if (aurora.tailChanged) {
          update.tailContent = aurora.tail || "";
          update.tailBlock = (aurora.tailBlock as any) || null;
        }

        // 3. 申请硬件级 rAF 渲染调度（合并原子提交）
        scheduleRAFUpdate(actualMessageId);
      }
    } else if (type === "end" || type === "error") {
      const errorMsg = event.error;
      const finishReason = event.finishReason;

      // 漏洞 1 & 2 & 3 修复：同步强制秒结，防止 tailContent 闪烁回滚丢失
      clearRAFUpdate(actualMessageId, true);

      if (finishReason) msg!.finishReason = finishReason;

      if (streamingMessageId.value === actualMessageId)
        streamingMessageId.value = null;

      if (type === "error" && errorMsg) {
        const errorText = `\n\n> VCP流式错误: ${errorMsg}`;
        if (msg) {
          const currentContent = msg.content || "";
          if (!currentContent.endsWith(errorText)) {
            msg.content = currentContent + errorText;
          }
          msg.finishReason = "error";
        }
      }

      if (msg) {
        msg!.isThinking = false;
        if (event.timestamp) {
          msg!.timestamp = event.timestamp;
        }

        // 定义闭环终结函数，保证在 blocks 赋值完成后才移除活动流状态
        const finalizeStream = () => {
          msg!.tailContent = "";
          msg!.tailBlock = undefined;
          removeSessionStream(itemId, topicId, actualMessageId);
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
            msg.blocks = event.blocks as any;
            finalizeStream();
          } else {
            invoke<any>("process_message_content", {
              content: msg!.content || "",
            })
              .then((compiledBlocks) => {
                msg.blocks = compiledBlocks as any;
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
        removeSessionStream(itemId, topicId, actualMessageId);
      }
    }
  };

  /**
   * 中止指定消息的生成
   */
  const stopMessage = async (
    messageId: string,
    onUpdateMessage?: (msgId: string) => Promise<void>,
  ) => {
    console.log(
      `[ChatStreamStore] Sending interrupt signal for message: ${messageId}`,
    );
    try {
      await invoke("interruptRequest", { messageId: messageId });

      // 本地模拟一个结束状态
      const msg = activeStreamMessages.get(messageId);
      if (msg) {
        msg.isThinking = false;
        msg.finishReason = "interrupted";
        const errorText = `\n\n> VCP流式错误: 请求已中止`;
        const currentContent = msg.content || "";
        if (!currentContent.endsWith(errorText)) {
          msg.content = currentContent + errorText;
        }
      }

      // 漏洞 2 修复：手动点击中止流时，瞬间强行注销 rAF 帧，防止后台句柄悬空空转泄漏
      clearRAFUpdate(messageId, false);

      if (streamingMessageId.value === messageId) {
        streamingMessageId.value = null;
      }

      // messageId 全局唯一；不要在 await 后读取可能已切换的当前会话。
      removeMessageFromAllSessions(messageId);

      if (onUpdateMessage) {
        await onUpdateMessage(messageId);
      }
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
  const stopGroupTurn = async (topicId: string) => {
    console.log(
      `[ChatStreamStore] Global Group Interruption for topic: ${topicId}`,
    );
    // 在首个 await 前冻结目标集合，避免切换会话后误停新话题的流。
    const activeIds = Array.from(activeStreamingIds.value);
    try {
      await invoke("interruptGroupTurn", { topicId: topicId });
      if (activeIds.length > 0) {
        await Promise.all(activeIds.map((id) => stopMessage(id)));
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
  });

  const isRecovering = ref(false);

  /**
   * 检查并恢复被异常打断的活跃生成（冷启动自对齐与流接续）
   */
  const checkAndRecoverInterruptedStreams = async () => {
    if (isRecovering.value) return;
    isRecovering.value = true;

    // 1. 本地扫表：无网状态下也能运行
    let activeGens: any[] = [];
    try {
      activeGens = await invoke<any[]>("get_active_generations");
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

      // 2. UI 预处理：在内存中将消息标记为 reconnecting，让用户在界面上看到“重连中”
      for (const gen of activeGens) {
        const { msgId, topicId, ownerId, ownerType, agentId, agentName } = gen;

        let msg = activeStreamMessages.get(msgId);
        if (!msg) {
          const historyStore = useChatHistoryStore();
          const existingMsg = historyStore.currentChatHistory.find(
            (x) => x.id === msgId,
          );

          if (existingMsg) {
            msg = existingMsg;
            msg.isReconnecting = true;
          } else {
            msg = reactive<ChatMessage>({
              id: msgId,
              role: "assistant",
              name: agentName,
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
            const currentKey = sessionStore.currentConversationKey;
            if (
              currentKey &&
              currentKey.topicId === topicId &&
              currentKey.ownerId === ownerId &&
              currentKey.ownerType === ownerType
            ) {
              historyStore.currentChatHistory.push(msg);
              historyStore.currentChatHistory.sort(
                (a, b) => a.timestamp - b.timestamp,
              );
            }
          }
          activeStreamMessages.set(msgId, msg);
        } else {
          msg.isReconnecting = true;
        }
        addSessionStream(ownerId, topicId, msgId);
      }

      // 3. 网络请求 Gate 门控
      if (typeof navigator !== "undefined" && !navigator.onLine) {
        console.log(
          "[ChatStreamStore] Network offline. Suspending active generations cloud sync.",
        );
        isRecovering.value = false;
        return;
      }

      for (const gen of activeGens) {
        const { msgId, topicId, ownerId, ownerType } = gen;
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

        if (recoveryMessageIds.has(msgId)) continue;
        recoveryMessageIds.add(msgId);

        const isWarm = warmMessageIds.has(msgId);
        let msg = activeStreamMessages.get(msgId);
        const originalContent = msg?.content || "";
        const originalBlocks = msg?.blocks ? [...msg.blocks] : [];

        if (!msg) {
          // 说明是冷启动后第一次加载或者流不在活跃池中
          const historyStore = useChatHistoryStore();
          const existing = historyStore.currentChatHistory.find(
            (x) => x.id === msgId,
          );
          if (existing) {
            msg = existing;
            activeStreamMessages.set(msgId, msg);
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

        const streamChannel = new Channel<any>();
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
                historyStore.currentChatHistory.sort(
                  (a, b) => a.timestamp - b.timestamp,
                );
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

        // 后端在一个 owner lease 内完成 query -> resume -> terminal commit。
        // 扫描门闩可以释放，但 recoveryMessageIds 会覆盖整个长连接生命周期。
        void invoke<any>("recover_active_generation", {
          msgId,
          streamChannel,
          isWarm,
        })
          .then((res) => {
            console.log(`[ChatStreamStore] Recovery status for ${msgId}:`, res);
            if (res.status === "already_running") {
              if (recoveryMessage) recoveryMessage.isReconnecting = false;
              return;
            }

            if (recoveryMessage) {
              recoveryMessage.isReconnecting = false;
              recoveryMessage.isThinking = false;
              if (typeof res.content === "string") {
                recoveryMessage.content = res.content;
                invoke<any>("process_message_content", {
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
            removeSessionStream(ownerId, topicId, msgId);

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
            removeSessionStream(ownerId, topicId, msgId);
          })
          .finally(() => {
            recoveryMessageIds.delete(msgId);
          });
      }
    } catch (e) {
      console.error("[ChatStreamStore] Cloud sync failed during recovery:", e);
    } finally {
      isRecovering.value = false;
    }
  };

  return {
    streamingMessageId,
    sessionActiveStreams,
    activeStreamMessages,
    activeStreamingIds,
    activeStreamIdSet,
    isMessageActive,
    isMessageActiveInSession,
    isGroupGenerating,
    computeShell,
    addSessionStream,
    removeSessionStream,
    processStreamEvent,
    stopMessage,
    stopGroupTurn,
    checkAndRecoverInterruptedStreams,
  };
});
