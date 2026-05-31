import { defineStore } from "pinia";
import { ref, computed, reactive, onScopeDispose } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { releaseScreenKeep } from "../composables/useScreenKeeper";
import { useChatSessionStore } from "./chatSessionStore";
import { useAssistantStore } from "./assistant";
import { useAvatarStore } from "./avatar";
import { useTopicStore } from "./topicListManager";
import type { ChatMessage, MessageShell } from "../types/chat";

export const useChatStreamStore = defineStore("chatStream", () => {
  const streamingMessageId = ref<string | null>(null);

  // 核心：记录每个会话（itemId + topicId）是否处于活动流状态
  // 格式: "itemId:topicId" -> [messageId1, messageId2, ...]
  const sessionActiveStreams = ref<Record<string, string[]>>({});

  // 全局活跃流消息池：存储所有正在生成的响应对象 (messageId -> Reactive<ChatMessage>)
  // 无论是在前台还是后台，流式消息都从此池中获取，保证响应式链路不断裂
  const activeStreamMessages = reactive<Map<string, ChatMessage>>(new Map());

  const cleanupTimers = new Set<ReturnType<typeof setTimeout>>();

  // ===== rAF 30Hz 帧合并直推暂存池 =====
  // 记录每个消息最新的 Aurora 暂存数据，消灭定时器空转，硬件级防抖并实现30Hz降降基数
  const rAFPendingUpdates = new Map<string, {
    content: string | null;
    blocks: any[] | null;
    tailContent: string | null;
    tailBlock: any | null;
    animationFrameId: number | null;
    lastRenderTime: number;
  }>();
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
        }
      }
      rAFPendingUpdates.delete(messageId);
    }
  };

  const sessionStore = useChatSessionStore();
  const assistantStore = useAssistantStore();
  const avatarStore = useAvatarStore();
  const topicStore = useTopicStore();

  /**
   * 在前端本地计算 MessageShell（替代 Rust 的 precompute_shell）
   */
  function computeShell(msg: { role: string; agentId?: string; name?: string }): MessageShell {
    const empty = "";
    if (msg.role === "user") {
      const userColor = avatarStore.getDominantColor("user", "user_avatar") || "rgb(226,54,56)";
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

  // 兼容旧逻辑的计算属性
  const activeStreamingIds = computed(() => {
    if (!sessionStore.currentSelectedItem?.id || !sessionStore.currentTopicId)
      return new Set<string>();
    const key = `${sessionStore.currentSelectedItem.id}:${sessionStore.currentTopicId}`;
    return new Set(sessionActiveStreams.value[key] || []);
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
    const excess = activeStreamMessages.size - MAX_STREAM_MESSAGES;
    // 按插入顺序（Map 保持插入顺序）清理最旧的非活跃消息
    for (const [id] of activeStreamMessages) {
      if (excess <= 0) break;
      // 只删除已完成的流（不在当前活跃会话中）
      if (!activeStreamingIds.value.has(id)) {
        activeStreamMessages.delete(id);
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
    let didRemove = false;
    if (streams) {
      const index = streams.indexOf(messageId);
      if (index !== -1) {
        streams.splice(index, 1);
        didRemove = true;
      }
      if (streams.length === 0) {
        delete sessionActiveStreams.value[key];
      }
    }
    if (didRemove && Object.keys(sessionActiveStreams.value).length === 0) {
      releaseScreenKeep();
    }
    // 同时从全局池中移除 (延迟移除，确保 finalizeStream 能拿到对象)
    const cleanupTimer = setTimeout(() => {
        if (!activeStreamingIds.value.has(messageId)) {
            activeStreamMessages.delete(messageId);
            clearRAFUpdate(messageId, false); // 漏洞 2 修复：延迟清理时，强制安全注销 rAF 帧，杜绝句柄泄露
        }
    }, 1000);
    cleanupTimers.add(cleanupTimer);
  };

  /**
   * 处理流式事件的核心逻辑 (会话隔离调度器)
   */
  const processStreamEvent = async (event: any, callbacks?: {
    onMessageCreated?: (msg: ChatMessage, topicId: string) => void;
    onStreamFinished?: (messageId: string, topicId: string) => void;
  }) => {
    const actualMessageId = event.messageId || event.message_id || "";
    const { chunk, type, context } = event;
    const ctx = context || {};
    const topicId = ctx.topicId;
    const isGroup = !!ctx.isGroupMessage || !!ctx.groupId;
    const itemId = isGroup ? ctx.groupId : (ctx.agentId || ctx.ownerId);
 
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
        shell: computeShell({ role: "assistant", agentId: ctx.agentId, name: ctx.agentName }),
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

      // [关键修复] 异步持久化骨架消息到 SQLite 数据库
      // 使得用户即便中途切换会话，重新加载历史时也存在此消息占位，从而触发 Object Hydration 完美接续流式动画
      invoke("append_single_message", {
        ownerId: itemId,
        ownerType: isGroup ? "group" : "agent",
        topicId,
        message: {
          id: actualMessageId,
          role: "assistant",
          name: ctx.agentName || null,
          content: "",
          timestamp: msg!.timestamp,
          isThinking: msg!.isThinking,
          is_thinking: msg!.isThinking,
          agentId: ctx.agentId || null,
          groupId: ctx.groupId || null,
          topicId,
          isGroupMessage: isGroup,
        }
      }).catch(e => {
        console.error("[ChatStreamStore] Failed to persist initial thinking skeleton:", e);
      });
    }

    // 维护流状态
    if (type === "thinking") {
      msg!.isThinking = true;
      addSessionStream(itemId, topicId, actualMessageId);
      if (!streamingMessageId.value) {
        streamingMessageId.value = actualMessageId;
      }
    } else if (type === "data") {
      msg!.isThinking = false;
      addSessionStream(itemId, topicId, actualMessageId);

      let textChunk = "";
      if (typeof chunk === "string") {
        textChunk = chunk;
      } else if (chunk && chunk.choices && chunk.choices.length > 0) {
        const delta = chunk.choices[0].delta;
        if (delta && delta.content) textChunk = delta.content;
      }

      if (textChunk) {
        msg!.content = (msg!.content || "") + textChunk;
        msg!.tailContent = msg!.content;
      }
    } else if (type === "aurora") {
      const aurora = event.aurora;
      if (aurora) {
        // === 🚀 开启流式时空录制（开发模式生效，Release 构建时自动摇树切除） ===
        if (import.meta.env.DEV) {
          if (!(window as any).__VCP_STREAM_TRACES__) {
            (window as any).__VCP_STREAM_TRACES__ = [];
          }
          (window as any).__VCP_STREAM_TRACES__.push({
            timestamp: performance.now(),
            messageId: actualMessageId,
            auroraPayload: {
              stableChanged: aurora.stableChanged,
              stableBlocksCount: aurora.stableBlocks?.length || 0,
              stableBlocksHashes: aurora.stableBlocks?.map((b: any) => b.hash) || [],
              tailChanged: aurora.tailChanged,
              tailContent: aurora.tail || "",
              tailBlockType: aurora.tailBlock?.type || null
            },
            msgSnapshot: msg ? {
              content: msg.content,
              blocksCount: msg.blocks?.length || 0,
              tailContent: msg.tailContent,
            } : null
          });
        }
        // ==========================

        // 1. 初始化或获取该 messageId 的帧合并状态
        let update = rAFPendingUpdates.get(actualMessageId);
        if (!update) {
          update = {
            content: null,
            blocks: null,
            tailContent: null,
            tailBlock: null,
            animationFrameId: null,
            lastRenderTime: 0,
          };
          rAFPendingUpdates.set(actualMessageId, update);
        }

        // 2. 覆盖写入暂存数据（稀疏合并）
        if (typeof aurora.content === "string") {
          update.content = aurora.content;
        }
        if (aurora.stableChanged && aurora.stableBlocks) {
          update.blocks = aurora.stableBlocks;
        }
        if (aurora.tailChanged) {
          update.tailContent = aurora.tail || "";
          update.tailBlock = (aurora.tailBlock as any) || null;
        }

        // 3. 申请硬件级 rAF 自适应阻尼渲染（最大 30Hz）
        if (update.animationFrameId === null) {
          const runRenderLoop = () => {
            const up = rAFPendingUpdates.get(actualMessageId);
            if (!up) return;

            const now = performance.now();
            const elapsed = now - up.lastRenderTime;

            if (elapsed >= MIN_RENDER_INTERVAL_MS) {
              // 满足约 30Hz 间隔，触发 Vue 响应式写入进行重绘
              const m = activeStreamMessages.get(actualMessageId);
              if (m) {
                if (up.content !== null) m.content = up.content;
                if (up.blocks !== null) m.blocks = up.blocks;
                if (up.tailContent !== null) m.tailContent = up.tailContent;
                if (up.tailBlock !== undefined) m.tailBlock = up.tailBlock;
              }
              up.lastRenderTime = now;
              // 重置当前帧内的合并暂存状态
              up.content = null;
              up.blocks = null;
              up.tailContent = null;
              up.tailBlock = null;
              up.animationFrameId = null;
            } else {
              // 没到门槛，在下一屏幕物理刷新帧继续尝试
              up.animationFrameId = requestAnimationFrame(runRenderLoop);
            }
          };
          update.animationFrameId = requestAnimationFrame(runRenderLoop);
        }
      }
      msg!.isThinking = false;
      addSessionStream(itemId, topicId, actualMessageId);
    } else if (type === "end" || type === "error") {
      const errorMsg = event.error;
      const finishReason = event.finishReason;

      // 漏洞 1 & 2 & 3 修复：同步强制秒结，防止 tailContent 闪烁回滚丢失
      clearRAFUpdate(actualMessageId, true);

      if (finishReason) msg!.finishReason = finishReason;

      removeSessionStream(itemId, topicId, actualMessageId);
      if (streamingMessageId.value === actualMessageId) streamingMessageId.value = null;

      if (type === "error" && errorMsg && errorMsg !== "请求已中止") {
        const errorText = `\n\n> VCP流式错误: ${errorMsg}`;
        msg!.content += errorText;
        msg!.finishReason = "error";
      }

      if (msg) {
        msg!.isThinking = false;
        if (event.timestamp) {
          msg!.timestamp = event.timestamp;
        }
        try {
          // 如果后端已经带回了预渲染好的 blocks，直接使用，跳过冗余解析
          if (event.blocks) {
            msg.blocks = event.blocks as any;
          } else {
            const compiledBlocks = await invoke("process_message_content", {
              content: msg!.content || "",
            });
            msg.blocks = compiledBlocks as any;
          }
        } catch (e) {
          console.error("[ChatStreamStore] process_message_content failed:", e);
        } finally {
          // 漏洞 1 终极解决：在最终编译树成功上屏后，才同步清空临时 tail，实现绝对零闪烁和无缝平滑交接
          msg!.tailContent = "";
          msg!.tailBlock = undefined;

          // === 🚀 输出流式诊断提示与回放指南（开发模式生效，Release 构建时自动摇树切除） ===
          if (import.meta.env.DEV) {
            console.log(
              `%c[VCP Stream Debugger] 🎉 流式传输结束！当前录制帧数: ${(window as any).__VCP_STREAM_TRACES__?.length || 0}`,
              "color: #10b981; font-weight: bold; font-size: 13px;"
            );
            console.log(
              `%c👉 运行指令 A 可一键获取帧轨迹总览:\n   console.table(window.__VCP_STREAM_TRACES__.map((t, idx) => ({ '帧号': idx, '相对时间(ms)': Math.round(t.timestamp - window.__VCP_STREAM_TRACES__[0].timestamp), 'Stable变动': t.auroraPayload.stableChanged, 'Stable块数': t.auroraPayload.stableBlocksCount, 'Tail变动': t.auroraPayload.tailChanged, 'Tail内容': t.auroraPayload.tailContent.substring(0, 15) })))`,
              "color: #3b82f6;"
            );
            console.log(
              `%c👉 运行指令 B 进行任意相邻帧 Diff 比对（如 12 帧与 13 帧）:\n   const fA = window.__VCP_STREAM_TRACES__[12]; const fB = window.__VCP_STREAM_TRACES__[13]; console.log("=== 帧12 Tail ===", fA.auroraPayload.tailContent); console.log("=== 帧13 Stable Hashes ===", fB.auroraPayload.stableBlocksHashes); console.log("=== 帧13 Tail ===", fB.auroraPayload.tailContent);`,
              "color: #8b5cf6;"
            );
          }
        }
        
        if (callbacks?.onStreamFinished) {
          callbacks.onStreamFinished(actualMessageId, topicId);
        }
      }
    }
  };

  /**
   * 中止指定消息的生成
   */
  const stopMessage = async (messageId: string, onUpdateMessage?: (msgId: string) => Promise<void>) => {
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
      }

      // 漏洞 2 修复：手动点击中止流时，瞬间强行注销 rAF 帧，防止后台句柄悬空空转泄漏
      clearRAFUpdate(messageId, false);

      if (streamingMessageId.value === messageId) {
        streamingMessageId.value = null;
      }

      const ownerId = sessionStore.currentSelectedItem?.id;
      const topicId = sessionStore.currentTopicId;

      if (ownerId && topicId) {
        removeSessionStream(ownerId, topicId, messageId);
      }

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
    console.log(`[ChatStreamStore] Global Group Interruption for topic: ${topicId}`);
    try {
      await invoke("interruptGroupTurn", { topicId: topicId });
      
      const activeIds = Array.from(activeStreamingIds.value);
      if (activeIds.length > 0) {
        await Promise.all(activeIds.map(id => stopMessage(id)));
      }
    } catch (e) {
      console.error("[ChatStreamStore] Failed to stop group turn:", e);
    }
  };

  onScopeDispose(() => {
    cleanupTimers.forEach(clearTimeout);
    cleanupTimers.clear();
    rAFPendingUpdates.forEach((up) => {
      if (up.animationFrameId !== null) {
        cancelAnimationFrame(up.animationFrameId);
      }
    });
    rAFPendingUpdates.clear();
  });

  return {
    streamingMessageId,
    sessionActiveStreams,
    activeStreamMessages,
    activeStreamingIds,
    isGroupGenerating,
    computeShell,
    addSessionStream,
    removeSessionStream,
    processStreamEvent,
    stopMessage,
    stopGroupTurn,
  };
});


