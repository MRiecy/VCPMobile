import { defineStore } from "pinia";
import { ref, computed, nextTick } from "vue";
import { convertFileSrc, invoke, Channel } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { useStreamManagerStore } from "./streamManager";
import { useSettingsStore } from "./settings";
import { useAssistantStore } from "./assistant";
import { useTopicStore } from "./topicListManager";
import { useNotificationStore } from "./notification";
import { useDocumentProcessor } from "../composables/useDocumentProcessor";
import type { ContentBlock } from "../composables/useContentProcessor";

/**
 * Attachment 接口定义，严格对齐 Rust 端的 AttachmentSyncDTO / Attachment (仅保留核心字段)
 */
export interface Attachment {
  id?: string; // 纯前端 UI 稳定性标识 (Stable Key)
  type: string;
  name: string;
  size: number;
  progress?: number; // 0-100 的真实上传进度
  src: string; // 物理存储路径：真理之源。用于后续超栈文件追踪，或跨端同步时的原始路径参考
  resolvedSrc?: string; // Webview 可用的 asset:// 路径 (运行时动态生成，不进行持久化)
  hash?: string;
  status?: string;
  internalPath?: string; // 手机本地物理路径，仅供前端通过 convertFileSrc 转换为安全 URL
  extractedText?: string;
  imageFrames?: string[];
  thumbnailPath?: string;
  createdAt?: number;
}

/**
 * ChatMessage 接口定义，严格对齐 Rust 端的 MessageSyncDTO / ChatMessage
 */
export interface ChatMessage {
  id: string;
  role: string;
  name?: string;
  content?: string; // 原文，现在变为按需懒加载的可选字段
  blocks?: ContentBlock[]; // 预编译的 AST 数据块，前端直接渲染
  timestamp: number;

  isThinking?: boolean;
  agentId?: string;
  groupId?: string;
  isGroupMessage?: boolean;
  finishReason?: string;
  attachments?: Attachment[];

  // 以下为纯前端运行时 UI 状态 (Ephemeral)，绝不进行持久化
  displayedContent?: string; // 用于兼容旧版渲染器的全量文本
  stableContent?: string;    // Aurora: 稳定区 HTML/Markdown
  tailContent?: string;      // Aurora: 尾随区 Markdown (高频变动)
  processedContent?: string; // 缓存 Rust 返回的 AST 或文本，避免重复解析
}

/**
 * HistoryChunk 接口定义，用于 Channel 流式加载
 */
interface HistoryChunk {
  message: ChatMessage;
  index: number;
  is_last: boolean;
}

/**
 * TopicDelta 接口定义
 */
export interface TopicDelta {
  added: ChatMessage[];
  updated: ChatMessage[];
  deleted_ids: string[];
  sync_skipped?: boolean;
}

/**
 * TopicFingerprint 接口定义
 */
export interface TopicFingerprint {
  topic_id: string;
  mtime: number;
  size: number;
  msg_count: number;
}

/**
 * useChatManagerStore
 */
export const useChatManagerStore = defineStore("chatManager", () => {
  // --- 状态变量 (State) ---
  const currentChatHistory = ref<ChatMessage[]>([]);
  const currentSelectedItem = ref<any>(null);
  const currentTopicId = ref<string | null>(null);
  const loading = ref(false);
  const streamingMessageId = ref<string | null>(null);

  // 分页加载状态
  const historyOffset = ref(0);        // 当前已加载的消息总数（= 下次请求的 offset 起点）
  const hasMoreHistory = ref(true);    // 是否还有更多旧消息
  const isLoadingHistory = ref(false); // 防止并发重复触发

  // 核心：记录每个会话（itemId + topicId）是否处于活动流状态
  // 格式: "itemId:topicId" -> [messageId1, messageId2, ...]
  const sessionActiveStreams = ref<Record<string, string[]>>({});

  // 兼容旧逻辑的计算属性 (修正：返回 Set 以保持兼容性，但内部依赖数组触发更新)
  const activeStreamingIds = computed(() => {
    if (!currentSelectedItem.value?.id || !currentTopicId.value)
      return new Set<string>();
    const key = `${currentSelectedItem.value.id}:${currentTopicId.value}`;
    return new Set(sessionActiveStreams.value[key] || []);
  });

  const isGroupGenerating = computed(() => {
    if (
      !currentSelectedItem.value?.id ||
      !currentTopicId.value ||
      currentSelectedItem.value.type !== "group"
    )
      return false;
    const key = `${currentSelectedItem.value.id}:${currentTopicId.value}`;
    const streams = sessionActiveStreams.value[key];
    return streams ? streams.length > 0 : false;
  });

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
  };

  // 非当前视图流的轻量级内容缓存 (messageId -> { content: string, topicId: string, ownerId: string })
  const backgroundStreamingBuffers = ref<
    Map<string, { content: string; topicId: string; ownerId: string }>
  >(new Map());

  // 暂存的附件列表，准备随下一条消息发送
  const stagedAttachments = ref<Attachment[]>([]);

  const streamManager = useStreamManagerStore();
  const settingsStore = useSettingsStore();
  const assistantStore = useAssistantStore();
  const topicStore = useTopicStore();
  const notificationStore = useNotificationStore();

  // 用于拦截重新生成时的输入框补全
  const editMessageContent = ref("");
  // 用于标记当前是否正在“编辑重发”某条历史消息
  const editingOriginalMessageId = ref<string | null>(null);

  let listenersRegistered = false;

  /**
   * 尝试为话题生成 AI 总结标题 (对齐桌面端 attemptTopicSummarization)
   */
  const summarizeTopic = async () => {
    if (!currentTopicId.value || !currentSelectedItem.value?.id) return;

    const topicId = currentTopicId.value;
    const ownerId = currentSelectedItem.value.id;
    const ownerType = currentSelectedItem.value.type;

    // 只有“未命名”话题且消息数达到阈值才总结 (桌面端策略)
    const topic = topicStore.topics.find((t) => t.id === topicId);
    const isUnnamed =
      !topic ||
      topic.name.includes("新话题") ||
      topic.name.includes("topic_") ||
      topic.name.includes("group_topic_") ||
      topic.name === "主要群聊";
    const messageCount = currentChatHistory.value.filter(
      (m) => m.role !== "system",
    ).length;

    if (isUnnamed && messageCount >= 4) {
      console.log(`[ChatManager] Triggering AI summary for topic: ${topicId}`);
      try {
        const agentName =
          assistantStore.agents.find((a: any) => a.id === ownerId)?.name ||
          "AI";
        const newTitle = await invoke<string>("summarize_topic", {
          ownerId,
          ownerType,
          topicId,
          agentName,
        });

        if (newTitle) {
          console.log(`[ChatManager] AI Summarized Title: ${newTitle}`);
          await topicStore.updateTopicTitle(
            ownerId,
            ownerType,
            topicId,
            newTitle,
          );
        }
      } catch (e) {
        console.error("[ChatManager] AI Summary failed:", e);
      }
    }
  };

  /**
   * 处理消息中的本地资源路径 (仅附件)，使用 Tauri 原生 asset:// 协议绕过 WebView 限制
   */
  const resolveMessageAssets = (msg: ChatMessage) => {
    // 处理附件 (仅处理图片类型)
    if (msg.attachments && msg.attachments.length > 0) {
      msg.attachments.forEach((att) => {
        // Rust 后端返回的路径现在主要在 internalPath，如果不在，回退到 src
        const sourcePath = att.internalPath || att.src;
        if (
          att.type.startsWith("image/") &&
          sourcePath &&
          !sourcePath.startsWith("http") &&
          !sourcePath.startsWith("data:")
        ) {
          try {
            att.resolvedSrc = convertFileSrc(sourcePath);
          } catch (err) {
            console.warn(
              `[ChatManager] Failed to convert attachment image path ${att.name}:`,
              err,
            );
          }
        }
      });
    }
  };

  /**
   * 触发文件选择器并暂存附件 (使用标准 HTML Input 完美解决 Android content:// 协议名和类型丢失问题)
   */
  const handleAttachment = async () => {
    return new Promise<void>((resolve, reject) => {
      const input = document.createElement("input");
      input.type = "file";
      input.multiple = false;
      // 允许所有类型
      input.accept = "*/*";

      input.onchange = async (e: Event) => {
        try {
          const target = e.target as HTMLInputElement;
          if (!target.files || target.files.length === 0) {
            resolve();
            return;
          }

          const file = target.files[0];
          console.log(
            `[ChatManager] Selected file via HTML input: ${file.name}, type: ${file.type}, size: ${file.size}`,
          );

          // 1. 生成稳定 ID 并使用 unshift 插入首位 (实现“最新附件最先看到”)
          const stableId = `att_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`;
          const blobUrl = URL.createObjectURL(file);

          stagedAttachments.value.unshift({
            id: stableId,
            type: file.type || "application/octet-stream",
            src: blobUrl,
            name: file.name,
            size: file.size,
            status: "loading",
          });

          await nextTick();
          window.dispatchEvent(new Event("resize"));

          try {
            let finalData: any = null;

            // --- 分流策略：小文件 ( < 2MB ) 走 IPC，大文件走高速 TCP 链路 ---
            if (file.size < 2 * 1024 * 1024) {
              console.log(
                `[ChatManager] Small file detected (<2MB), using store_file IPC for ${file.name}`,
              );
              // 将 File 转换为 Uint8Array (Tauri v2 支持直接传递二进制，严禁使用 Array.from)
              const arrayBuffer = await file.arrayBuffer();
              const bytes = new Uint8Array(arrayBuffer);

              finalData = await invoke<any>("store_file", {
                originalName: file.name,
                fileBytes: bytes, // 直接传递，性能提升 100 倍
                mimeType: file.type || "application/octet-stream",
              });
            } else {
              console.log(
                `[ChatManager] Large file detected, opening High-Speed Link for ${file.name} (${file.size} bytes)`,
              );

              // 1. 准备链路 (Rust 开启临时本地 TCP 接收器)
              const endpoint = await invoke<any>("prepare_vcp_upload", {
                metadata: {
                  name: file.name,
                  mime: file.type || "application/octet-stream",
                  size: file.size,
                },
              });

              // 2. 内核级搬运 (利用流式上传)
              const xhr = new XMLHttpRequest();
              const uploadPromise = new Promise((res, rej) => {
                xhr.open("POST", endpoint.url, true);
                xhr.setRequestHeader(
                  "Content-Type",
                  "application/octet-stream",
                );
                xhr.setRequestHeader("X-Upload-Token", endpoint.token);

                let lastUpdate = 0;
                xhr.upload.onprogress = (event) => {
                  if (event.lengthComputable) {
                    const now = Date.now();
                    // 限制刷新频率为 ~30fps (每 33ms 刷新一次)，避免高频重绘导致卡顿
                    if (now - lastUpdate < 33) return;
                    lastUpdate = now;

                    const progress = Math.round(
                      (event.loaded / event.total) * 100,
                    );
                    const attIndex = stagedAttachments.value.findIndex(
                      (a) => a.id === stableId,
                    );
                    if (attIndex !== -1) {
                      stagedAttachments.value[attIndex].progress = progress;
                    }
                  }
                };

                xhr.onload = () => {
                  if (xhr.status >= 200 && xhr.status < 300) {
                    res(JSON.parse(xhr.responseText));
                  } else {
                    rej(new Error(`Upload failed with status ${xhr.status}`));
                  }
                };

                xhr.onerror = () => rej(new Error("XHR Network Error"));
                xhr.send(file);
              });

              finalData = await uploadPromise;
            }

            if (finalData) {
              const index = stagedAttachments.value.findIndex(
                (a) => a.id === stableId,
              );
              if (index !== -1) {
                stagedAttachments.value[index] = {
                  ...stagedAttachments.value[index],
                  type: finalData.type,
                  src: finalData.internalPath,
                  name: finalData.name,
                  size: finalData.size,
                  hash: finalData.hash,
                  status: "done",
                };
              }
            }
            resolve();
          } catch (err) {
            console.error("[ChatManager] High-speed upload failed:", err);
            const index = stagedAttachments.value.findIndex(
              (a) => a.id === stableId,
            );
            if (index !== -1) stagedAttachments.value.splice(index, 1);
            reject(err);
          } finally {
            URL.revokeObjectURL(blobUrl);
          }
          resolve();
        } catch (err) {
          console.error(
            "[ChatManager] Failed to pick or store attachment:",
            err,
          );
          reject(err);
        }
      };

      input.oncancel = () => {
        resolve();
      };

      input.click();
    });
  };

  /**
   * 加载聊天历史 (已优化：使用私有协议直连，搭载预编译 AST 且防 OOM)
   */
  const loadHistory = async (
    ownerId: string,
    ownerType: string,
    topicId: string,
    limit: number = 15,
    offset: number = 0,
  ) => {
    console.log(
      `[ChatManager] Streaming history for ${ownerId}, topic: ${topicId}`,
    );
    loading.value = true;
    isLoadingHistory.value = true;
    try {
      // [分页防护] 记录请求时的话题 ID
      const requestedTopicId = currentTopicId.value;

      const channel = new Channel<HistoryChunk>();
      const buffer: ChatMessage[] = [];
      let receivedCount = 0;
      let resolveComplete: (() => void) | null = null;
      const completePromise = new Promise<void>((resolve) => { resolveComplete = resolve; });

      channel.onmessage = (chunk) => {
        // [话题一致性防护] 若用户在此期间切换了话题，丢弃后续数据
        if (currentTopicId.value !== requestedTopicId && requestedTopicId !== null) {
          return;
        }

        if (offset === 0) {
          // 首次加载：逐条 push，最新消息尽快出现
          if (chunk.index === 0) {
            currentChatHistory.value = [];
            hasMoreHistory.value = true;
          }
          currentChatHistory.value.push(chunk.message);
          receivedCount++;
        } else {
          // 加载更多：先 buffer 避免 Vue 频繁重渲染
          buffer.push(chunk.message);
          receivedCount++;
        }

        if (chunk.is_last) {
          if (offset > 0) {
            // 一次性 prepend 旧消息
            currentChatHistory.value = [...buffer, ...currentChatHistory.value];
            historyOffset.value += buffer.length;
            if (buffer.length < limit) {
              hasMoreHistory.value = false;
            }
          } else {
            historyOffset.value = receivedCount;
            if (receivedCount < limit) {
              hasMoreHistory.value = false;
            }
          }
          resolveComplete?.();
        }
      };

      await invoke('load_chat_history_streamed', {
        ownerId,
        ownerType,
        topicId,
        limit,
        offset,
        onMessage: channel,
      });

      // [空列表兜底] 新建话题或空话题时 Rust 端不会发送任何 Channel 消息，
      // 导致 completePromise 永不 resolve。此处手动处理空列表情况。
      if (receivedCount === 0) {
        if (offset === 0) {
          currentChatHistory.value = [];
          hasMoreHistory.value = false;
          historyOffset.value = 0;
        }
        (resolveComplete as (() => void) | null)?.();
      }

      // 等待最后一条消息处理完毕
      await completePromise;

      // [话题一致性防护] 若用户在此期间切换了话题，丢弃旧结果
      if (currentTopicId.value !== requestedTopicId && requestedTopicId !== null) {
        console.warn(`[ChatManager] Topic changed during load, discarding ${receivedCount} messages.`);
        return;
      }

      if (offset === 0) {
        // 恢复后台缓存中的流式内容 (如果用户切回了正在流式的话题)
        backgroundStreamingBuffers.value.forEach((buf, msgId) => {
          if (buf.topicId === topicId) {
            const msg = currentChatHistory.value.find((m) => m.id === msgId);
            if (msg) {
              console.log(
                `[ChatManager] Restoring background buffer for ${msgId} into current view.`,
              );
              msg.content = (msg.content || "") + buf.content;
              msg.isThinking = false;
              // 同时也同步给 streamManager 确保平滑显示
              streamManager.appendChunk(msgId, "", (data) => {
                msg.stableContent = data.stable;
                msg.tailContent = data.tail;
                msg.displayedContent = data.stable + data.tail;
              });
            }
          }
        });

        // 清理不属于当前话题的后台缓存，防止无界增长
        for (const [msgId, buf] of backgroundStreamingBuffers.value.entries()) {
          if (buf.topicId !== topicId) {
            backgroundStreamingBuffers.value.delete(msgId);
          }
        }
      }

      currentTopicId.value = topicId;

      if (offset === 0) {
        // --- 核心优化：记录当前活跃话题 ID 到后端持久化 ---
        if (ownerType === 'agent') {
          invoke('update_agent_config', { 
            agentId: ownerId, 
            updates: { currentTopicId: topicId } 
          }).catch(e => console.warn('[ChatManager] Failed to persist currentTopicId:', e));
        } else if (ownerType === 'group') {
          invoke('update_group_config', { 
            groupId: ownerId, 
            updates: { currentTopicId: topicId } 
          }).catch(e => console.warn('[ChatManager] Failed to persist currentTopicId:', e));
        }
      }

      if (
        !currentSelectedItem.value ||
        currentSelectedItem.value.id !== ownerId
      ) {
        // 更新当前选中的项目详情 (确保头像和色调同步)
        const agent = assistantStore.agents.find(a => a.id === ownerId);
        const group = assistantStore.groups.find(g => g.id === ownerId);
        currentSelectedItem.value = agent ? { ...agent, type: 'agent' } : (group ? { ...group, type: 'group' } : { id: ownerId, type: ownerType });
      }

      // 异步解析本地资源路径
      const messagesToResolve = offset === 0 ? currentChatHistory.value : buffer;
      await Promise.all(
        messagesToResolve.map(async (msg) => {
          resolveMessageAssets(msg);
        }),
      );

      console.log(
        `[ChatManager] History streamed: ${receivedCount} messages (Pre-processed by Rust)`,
      );
    } catch (e) {
      console.error("[ChatManager] Failed to stream history:", e);
    } finally {
      loading.value = false;
      isLoadingHistory.value = false;
    }
  };

  /**
   * 首屏加载：直接加载最新 15 条（流式 Channel 已无首屏阻塞问题）
   */
  const loadHistoryPaginated = async (
    ownerId: string,
    ownerType: string,
    topicId: string,
  ) => {
    await loadHistory(ownerId, ownerType, topicId, 15, 0);
  };

  /**
   * 滚动触发：加载下一页旧消息
   */
  const loadMoreHistory = async () => {
    if (!hasMoreHistory.value || isLoadingHistory.value) return;
    if (!currentSelectedItem.value?.id || !currentTopicId.value) return;
    await loadHistory(
      currentSelectedItem.value.id,
      currentSelectedItem.value.type,
      currentTopicId.value,
      15,
      historyOffset.value,
    );
  };

  /**
   * 更新某条消息的内容（用于全屏编辑消息）
   */
  const updateMessageContent = async (
    messageId: string,
    newContent: string,
  ) => {
    const msg = currentChatHistory.value.find((m) => m.id === messageId);
    if (!msg) return;

    msg.content = newContent;
    // 重置预编译和显示缓存，强制触发 MessageRenderer 重新请求 Rust 解析
    msg.blocks = undefined;
    msg.processedContent = undefined;
    if (msg.displayedContent) {
      msg.displayedContent = "";
    }

    if (currentSelectedItem.value?.id && currentTopicId.value) {
      await invoke("patch_single_message", {
        ownerId: currentSelectedItem.value.id,
        ownerType: currentSelectedItem.value.type,
        topicId: currentTopicId.value,
        message: msg,
      });

      notificationStore.addNotification({
        type: "success",
        title: "消息编辑已保存",
        message: "变更已同步至底层数据库",
        toastOnly: true,
      });

    }
  };

  /**
   * 内部方法：触发 AI 生成逻辑 (对接 Rust 后端)
   * 支持单 Agent 和多 Agent 群组。
   * 它不负责创建用户消息，只负责根据已有的 userMsg 触发后续生成流。
   */
  const triggerGeneration = async (userMsg: ChatMessage) => {
    if (!currentSelectedItem.value || !currentTopicId.value) return;

    const agentId = currentSelectedItem.value.id;
    const now = Date.now();

    // 构造 AI 思考占位消息
    const thinkingId = `msg_${now}_assistant_${Math.random().toString(36).substring(2, 7)}`;
    const thinkingMsg: ChatMessage = {
      id: thinkingId,
      role: "assistant",
      name:
        currentSelectedItem.value.type === "agent"
          ? (assistantStore.agents.find((a) => a.id === agentId)?.name || "Assistant")
          : undefined,
      content: "",
      timestamp: now + 1, // 增加 1ms 偏移，确保在时间序列上绝对位于提问之后
      isThinking: false,
      isGroupMessage: currentSelectedItem.value.type === "group",
      groupId:
        currentSelectedItem.value.type === "group"
          ? currentSelectedItem.value.id
          : undefined,
      agentId:
        currentSelectedItem.value.type === "agent"
          ? currentSelectedItem.value.id
          : undefined,
    };

    currentChatHistory.value.push(thinkingMsg);
    streamingMessageId.value = thinkingId;

    if (currentSelectedItem.value?.id && currentTopicId.value) {
      addSessionStream(
        currentSelectedItem.value.id,
        currentTopicId.value,
        thinkingId,
      );
    }

    try {
      // 立即保存一次历史记录 (包含用户消息和思考态)
      // 注意：由于 Rust 使用 ON CONFLICT DO UPDATE，即使 userMsg 已经存在也没关系
      if (currentSelectedItem.value?.id && currentTopicId.value) {
        await invoke("append_single_message", {
          ownerId: currentSelectedItem.value.id,
          ownerType: currentSelectedItem.value.type,
          topicId: currentTopicId.value,
          message: userMsg,
        });
        await invoke("append_single_message", {
          ownerId: currentSelectedItem.value.id,
          ownerType: currentSelectedItem.value.type,
          topicId: currentTopicId.value,
          message: thinkingMsg,
        });

      }

      const settings = settingsStore.settings;
      if (!settings) {
        throw new Error("应用尚未完成初始化，缺少设置数据，无法发送消息");
      }

      const vcpUrl = settings.vcpServerUrl || "";
      const vcpApiKey = settings.vcpApiKey || "";

      // 创建流式 Channel，每个请求独立
      const streamChannel = new Channel<StreamEvent>();
      streamChannel.onmessage = handleStreamEvent;

      // --- 群组消息路由 ---
      if (currentSelectedItem.value?.type === "group") {
        const groupId = currentSelectedItem.value.id;

        const groupPayload = {
          groupId,
          topicId: currentTopicId.value,
          userMessage: userMsg,
          vcpUrl,
          vcpApiKey,
        };

        console.log("[ChatManager] Sending group payload:", groupPayload);
        await invoke("handle_group_chat_message", { payload: groupPayload, streamChannel });
        return;
      }

      // --- 普通单 Agent 消息逻辑 ---
      const agentPayload = {
        agentId,
        topicId: currentTopicId.value,
        userMessage: userMsg,
        vcpUrl,
        vcpApiKey,
        thinkingMessageId: thinkingId,
      };

      console.log("[ChatManager] Sending agent payload:", agentPayload);
      await invoke("handle_agent_chat_message", { payload: agentPayload, streamChannel });
    } catch (e) {
      console.error("[ChatManager] Failed to trigger generation:", e);

      const errorText = `\n\n> VCP错误: ${e instanceof Error ? e.message : String(e)}`;

      const msgIndex = currentChatHistory.value.findIndex(
        (m) => m.id === thinkingId,
      );
      if (msgIndex !== -1) {
        const msg = currentChatHistory.value[msgIndex];
        msg.isThinking = false;
        msg.content += errorText;
        if (msg.displayedContent !== undefined) {
          msg.displayedContent += errorText;
        }
        if (currentSelectedItem.value?.id && currentTopicId.value) {
          removeSessionStream(
            currentSelectedItem.value.id,
            currentTopicId.value,
            thinkingId,
          );
        }
      } else {
        currentChatHistory.value.push({
          id: `msg_${Date.now()}_system_error`,
          role: "system",
          content: errorText.trim(),
          timestamp: Date.now(),
        });
      }

      streamingMessageId.value = null;
      if (currentSelectedItem.value?.id && currentTopicId.value) {
        const msg = currentChatHistory.value.find((m) => m.id === thinkingId);
        if (msg) {
          await invoke("patch_single_message", {
            ownerId: currentSelectedItem.value.id,
            ownerType: currentSelectedItem.value.type,
            topicId: currentTopicId.value,
            message: msg,
          });
        }
      }
    }
  };

  /**
   * 发送消息
   */
  const sendMessage = async (content: string) => {
    if (
      !currentSelectedItem.value ||
      !currentTopicId.value ||
      (!content.trim() && stagedAttachments.value.length === 0)
    )
      return;

    // --- 拦截：编辑重发逻辑 ---
    if (editingOriginalMessageId.value) {
      const originalId = editingOriginalMessageId.value;
      editingOriginalMessageId.value = null; // 消费掉

      const targetIndex = currentChatHistory.value.findIndex(
        (m) => m.id === originalId,
      );
      if (targetIndex !== -1) {
        const targetMsg = currentChatHistory.value[targetIndex];
        targetMsg.content = content;
        // 重置缓存
        targetMsg.blocks = undefined;
        targetMsg.processedContent = undefined;

        // 1. 截断后端数据库 (删除该消息之后的所有内容)
        await invoke("truncate_history_after_timestamp", {
          ownerId: currentSelectedItem.value.id,
          ownerType: currentSelectedItem.value.type,
          topicId: currentTopicId.value,
          timestamp: targetMsg.timestamp, // 严格大于该时间戳的消息都将被删除
        });

        // 2. 截断前端 UI 历史
        currentChatHistory.value = currentChatHistory.value.slice(
          0,
          targetIndex + 1,
        );

        // 3. 重新发起生成
        await triggerGeneration(targetMsg);
        return;
      }
    }

    const currentStaged = [...stagedAttachments.value];
    // Clear staged area immediately for UI responsiveness
    stagedAttachments.value = [];

    // Document Processing JIT (Just-In-Time)
    if (currentStaged.length > 0) {
      const docProcessor = useDocumentProcessor();
      for (const att of currentStaged) {
        const ext = att.name.split(".").pop()?.toLowerCase();
        // Only process documents and PDFs as requested
        if (["txt", "md", "csv", "json", "docx", "pdf"].includes(ext || "")) {
          try {
            const result = await docProcessor.processAttachment(att);
            if (result) {
              if (result.extractedText)
                att.extractedText = result.extractedText;
              if (result.imageFrames) att.imageFrames = result.imageFrames;
            }
          } catch (e) {
            console.error(
              `[ChatManager] JIT document processing failed for ${att.name}:`,
              e,
            );
          }
        }
      }
    }

    // 构造用户消息
    const now = Date.now();
    const userMsg: ChatMessage = {
      id: `msg_${now}_user_${Math.random().toString(36).substring(2, 7)}`,
      role: "user",
      name: settingsStore.settings?.userName || "User",
      content,
      timestamp: now,
      attachments: currentStaged.length > 0 ? currentStaged : undefined,
    };

    currentChatHistory.value.push(userMsg);
    await triggerGeneration(userMsg);
  };

  /**
   * 删除指定消息及之后的所有消息 (通常用于重新生成或回退)
   * 如果 deleteAfter 为 true，则相当于时间回溯
   */
  const deleteMessage = async (
    messageId: string,
    deleteAfter: boolean = false,
  ) => {
    if (!currentSelectedItem.value || !currentTopicId.value) return;

    const ownerId = currentSelectedItem.value.id;
    const ownerType = currentSelectedItem.value.type;
    const topicId = currentTopicId.value;
    const targetIndex = currentChatHistory.value.findIndex(
      (m) => m.id === messageId,
    );
    if (targetIndex === -1) return;

    const targetMsg = currentChatHistory.value[targetIndex];

    if (deleteAfter) {
      // 物理截断：删除自身以及后面所有的
      await invoke("truncate_history_after_timestamp", {
        ownerId,
        ownerType,
        topicId,
        timestamp: targetMsg.timestamp - 1, // 包含自身
      });
      currentChatHistory.value.splice(targetIndex);
    } else {
      // 逻辑删除：仅删除自身
      await invoke("delete_messages", {
        ownerId,
        ownerType,
        topicId,
        msgIds: [messageId],
      });
      currentChatHistory.value.splice(targetIndex, 1);
    }

    notificationStore.addNotification({
      type: "success",
      title: "消息已删除",
      message: "该条消息已从本地库移除",
      toastOnly: true,
    });
  };

  /**
   * 强行中止整个群组的接力赛回合
   */
  const stopGroupTurn = async (topicId: string) => {
    console.log(`[ChatManager] Global Group Interruption for topic: ${topicId}`);
    try {
      // 1. 发射后端熔断信号，打断 for 循环接力
      await invoke("interruptGroupTurn", { topicId: topicId });
      
      // 2. 同时中止当前活跃的所有流消息（确保当前正在说话的 Agent 也立即停下）
      const activeIds = Array.from(activeStreamingIds.value);
      if (activeIds.length > 0) {
        await Promise.all(activeIds.map(id => stopMessage(id)));
      }
    } catch (e) {
      console.error("[ChatManager] Failed to stop group turn:", e);
    }
  };

  /**
   * 中止指定消息的生成
   */
  const stopMessage = async (messageId: string) => {
    console.log(
      `[ChatManager] Sending interrupt signal for message: ${messageId}`,
    );
    try {
      await invoke("interruptRequest", { messageId: messageId });
      // 本地伪造一个 end 事件，防止假死
      streamManager.finalizeStream(messageId);

      // 确保清理状态
      const msgIndex = currentChatHistory.value.findIndex(
        (m) => m.id === messageId,
      );
      let msg: ChatMessage | null = null;
      if (msgIndex !== -1) {
        msg = currentChatHistory.value[msgIndex];
        msg.isThinking = false;
        msg.finishReason = "cancelled_by_user";
      }

      const ownerId = currentSelectedItem.value?.id;
      const topicId = currentTopicId.value;

      if (ownerId && topicId) {
        removeSessionStream(ownerId, topicId, messageId);
      }

      if (streamingMessageId.value === messageId) {
        streamingMessageId.value = null;
      }
      // 增量保存当前中止后的内容
      if (msg && ownerId && topicId) {
        await invoke("append_single_message", {
          ownerId,
          ownerType: currentSelectedItem.value!.type,
          topicId,
          message: msg,
        });
      }
    } catch (e) {
      console.error(
        `[ChatManager] Failed to interrupt stream for ${messageId}:`,
        e,
      );
    }
  };
  
  /**
   * 重新生成消息
   * @param targetMessageId 用户想要重新生成的 AI 回复的 ID
   */
  const regenerateResponse = async (targetMessageId: string) => {
    // 1. 查找此消息在历史记录中的位置
    const targetIndex = currentChatHistory.value.findIndex(
      (m) => m.id === targetMessageId,
    );
    if (targetIndex === -1) return;

    const targetMsg = currentChatHistory.value[targetIndex];

    // 2. 截断后端数据库 (删除该 AI 消息及其之后的所有内容)
    if (currentSelectedItem.value?.id && currentTopicId.value) {
      await invoke("truncate_history_after_timestamp", {
        ownerId: currentSelectedItem.value.id,
        ownerType: currentSelectedItem.value.type,
        topicId: currentTopicId.value,
        timestamp: targetMsg.timestamp - 1, // 严格大于该时间戳的消息都将被删除，即包括目标消息本身
      });
    }

    // 3. 截断前端 UI 历史
    currentChatHistory.value = currentChatHistory.value.slice(0, targetIndex);

    // 4. 寻找距离目前最近的一条用户消息，作为触发生成的“引子”
    // 即使在群聊中存在连续 Agent 消息，后端也会从数据库加载完整的、已截断的历史上下文。
    let lastUserMsgIndex = currentChatHistory.value.length - 1;
    while (
      lastUserMsgIndex >= 0 &&
      currentChatHistory.value[lastUserMsgIndex].role !== "user"
    ) {
      lastUserMsgIndex--;
    }

    if (lastUserMsgIndex === -1) {
      console.warn("[ChatManager] Cannot regenerate: No user message found.");
      return;
    }

    const lastUserMsg = currentChatHistory.value[lastUserMsgIndex];

    // 5. 重新发起生成
    await triggerGeneration(lastUserMsg);
  };

  /**
   * 按需拉取单条消息的原始 Markdown 内容
   */
  const fetchRawContent = async (messageId: string): Promise<string> => {
    // 检查缓存中是否已有，或者是否正在流式传输
    const existingMsg = currentChatHistory.value.find((m) => m.id === messageId);
    if (existingMsg && existingMsg.content) {
      return existingMsg.content;
    }

    try {
      console.log(`[ChatManager] Lazy loading raw content for message: ${messageId}`);
      const content = await invoke<string>('fetch_raw_message_content', { messageId });
      if (existingMsg) {
        existingMsg.content = content;
      }
      return content;
    } catch (e) {
      console.error(`[ChatManager] Failed to fetch raw content for ${messageId}:`, e);
      return "";
    }
  };

  interface StreamEvent {
    type: string;
    chunk?: any;
    messageId?: string;
    message_id?: string;
    context?: any;
    finishReason?: string;
    error?: string;
  }

  const handleStreamEvent = (event: StreamEvent) => {
    const actualMessageId = event.messageId || event.message_id || "";
    const { chunk, type, context } = event;
      // 无论是否在当前视图，都尝试更新数据
      let msg = currentChatHistory.value.find((m) => m.id === actualMessageId);
      const ctx = context || {};
      const topicId = ctx.topicId || currentTopicId.value;
      const itemId =
        ctx.agentId || ctx.groupId || currentSelectedItem.value?.id;

      // [关键修复] 如果是群聊并行流，且消息尚未在 currentChatHistory 中（因为是 Rust 端刚发起的），
      // 我们需要根据 context 自动创建一个占位消息，以便立即展示流式内容。
      if (
        !msg &&
        context &&
        context.isGroupMessage &&
        context.groupId === currentSelectedItem.value?.id
      ) {
        console.log(
          `[ChatManager] Creating placeholder for group message: ${actualMessageId}`,
        );
        msg = {
          id: actualMessageId,
          role: "assistant",
          name: context.agentName,
          content: "",
          timestamp: Date.now(),
          isThinking: false,
          agentId: context.agentId,
          groupId: context.groupId,
          isGroupMessage: true,
        };
        currentChatHistory.value.push(msg as ChatMessage);
        // 保持排序
        currentChatHistory.value.sort((a, b) => a.timestamp - b.timestamp);
      }

      if (type === "data") {
        if (msg) {
          msg.isThinking = false;
        }

        // 确保在活动流集合中
        if (itemId && topicId) {
          addSessionStream(itemId, topicId, actualMessageId);
        }

        // 解析 chunk 提取文本内容
        let textChunk = "";
        if (typeof chunk === "string") {
          textChunk = chunk;
        } else if (chunk && chunk.choices && chunk.choices.length > 0) {
          const delta = chunk.choices[0].delta;
          if (delta && delta.content) {
            textChunk = delta.content;
          }
        }

        if (textChunk) {
          // 1. 更新当前视图 (如果匹配)
          if (msg) {
            msg.content = msg.content || "";
            msg.content += textChunk;
            // 使用 streamManager 平滑更新 Aurora 缓冲区

            streamManager.appendChunk(actualMessageId, textChunk, (data) => {
              const latestMsg = currentChatHistory.value.find(
                (m) => m.id === actualMessageId,
              );
              if (latestMsg) {
                latestMsg.stableContent = data.stable;
                latestMsg.tailContent = data.tail;
                // 保持兼容性：displayedContent 为两者合并，供旧版组件使用
                latestMsg.displayedContent = data.stable + data.tail;
              }
            });
          } else {
            // 2. 更新后台缓存 (如果不在当前视图)
            if (topicId && itemId) {
              const buffer = backgroundStreamingBuffers.value.get(
                actualMessageId,
              ) || { content: "", topicId, ownerId: itemId };
              buffer.content += textChunk;
              backgroundStreamingBuffers.value.set(actualMessageId, buffer);
            }

            // 同时也喂给 streamManager，防止切回来时 buffer 为空
            streamManager.appendChunk(actualMessageId, textChunk, (_data) => {});
          }
        }
      } else if (type === "end" || type === "error") {
        const errorMsg = event.error;
        const finishReason = event.finishReason;
        console.log(
          `[ChatManager] Stream ${type} for ${actualMessageId}${errorMsg ? ": " + errorMsg : ""}. Reason: ${finishReason}. Draining queue...`,
        );

        // 流式结束时，等待 streamManager 缓冲队列排空后再切换状态
        streamManager.finalizeStream(actualMessageId, async () => {
          const latestMsg = currentChatHistory.value.find(
            (m) => m.id === actualMessageId,
          );
          if (latestMsg) {
            // 确保最终内容一致
            latestMsg.displayedContent = latestMsg.content;
            if (finishReason) {
              latestMsg.finishReason = finishReason;
            }
          }

          // 清理活动流状态
          if (itemId && topicId) {
            removeSessionStream(itemId, topicId, actualMessageId);
          }
          if (streamingMessageId.value === actualMessageId) {
            streamingMessageId.value = null;
          }

          if (type === "error" && errorMsg && errorMsg !== "请求已中止") {
            const errorText = `\n\n> VCP流式错误: ${errorMsg}`;
            if (latestMsg) {
              latestMsg.content += errorText;
              latestMsg.displayedContent = latestMsg.content;
              latestMsg.finishReason = "error";
            } else {
              // 如果不在当前视图，暂时不处理系统错误消息的追加，
              // 因为我们没有全局的消息持久化更新接口（本阶段目标是打通多流基础）
            }
          }

          if (latestMsg) {
            // 使用 patch (墓碑更新) 代替 append
            // 因为在发送瞬间我们已经 append 过一个思考态占位符了，现在是更新它的内容
            if (currentSelectedItem.value?.id && currentTopicId.value) {
              await invoke("patch_single_message", {
                ownerId: currentSelectedItem.value.id,
                ownerType: currentSelectedItem.value.type,
                topicId: currentTopicId.value,
                message: latestMsg,
              });

              // [核心升级] 流式结束后，立即获取一次预编译的 AST 块并存在内存中
              // 这样在下次滚动或切换时，MessageRenderer 可以直接零耗时渲染
              try {
                const freshBlocks = await invoke<any[]>("process_message_content", { 
                  content: latestMsg.content 
                });
                latestMsg.blocks = freshBlocks;
                console.log(`[ChatManager] Fresh blocks pre-compiled for ${actualMessageId}`);
              } catch (err) {
                console.warn("[ChatManager] Post-stream compilation failed:", err);
              }
            }

            // 话题自动总结逻辑
            await summarizeTopic();
          } else {
            // 如果不在当前视图，从后台缓存中提取并尝试保存一次
            const buffer =
              backgroundStreamingBuffers.value.get(actualMessageId);
            if (buffer) {
              console.log(
                `[ChatManager] Finalizing background stream for ${actualMessageId}, triggering silent save.`,
              );
              // 这里我们不直接调用 saveHistory，因为 saveHistory 依赖 currentChatHistory。
              // 我们需要一个能保存特定话题历史的接口，或者依赖 Rust 端的断点存盘。
              // 鉴于 Rust 端 handle_group_chat_message 已经有断点存盘，单聊 sendToVCP 结束后也会返回 fullContent，
              // 前端这里的后台缓存主要用于“切回话题时立即显示最新进度”。
            }
          }

          // 彻底清理
          backgroundStreamingBuffers.value.delete(actualMessageId);
        });
      }
    }
  ;

  // --- 初始化与销毁 (Lifecycle) ---

  const ensureEventListenersRegistered = async () => {
    if (listenersRegistered) return;
    listenersRegistered = true;
    console.log("[ChatManager] Registering Tauri listeners");

    // 同步完成刷新已集中到 main.ts（window.location.reload），此处无需重复监听

    // 监听外部文件变更 (对应桌面端的 history-file-updated)
    listen("vcp-file-change", async (event: any) => {
      const paths = event.payload as string[];
      console.log("[ChatManager] File change detected by Rust Watcher:", paths);

      if (!currentTopicId.value || !currentSelectedItem.value?.id) return;

      // 检查变更的文件路径是否包含当前正在查看的 topicId
      const isCurrentTopicChanged = paths.some(
        (p) => p.includes(currentTopicId.value!) && p.endsWith("history.json"),
      );

      if (isCurrentTopicChanged) {
        console.log(
          `[ChatManager] Current topic ${currentTopicId.value} history changed externally. Syncing...`,
        );
        const ownerId = currentSelectedItem.value?.id;
        const ownerType = currentSelectedItem.value?.type;
        if (ownerId && ownerType) {
          await loadHistory(ownerId, ownerType, currentTopicId.value!);
        }
      }
    });

    listen("vcp-group-turn-finished", async (event: any) => {
      const { groupId, topic_id, topicId: legacyTopicId } = event.payload;
      const actualTopicId = topic_id || legacyTopicId;

      if (
        groupId === currentSelectedItem.value?.id &&
        actualTopicId === currentTopicId.value
      ) {
        console.log(`[ChatManager] Group turn finished for ${groupId}`);
        // 强制同步一次，确保所有并行 Agent 的最终结果都已落盘并同步到前端
        const ownerType = currentSelectedItem.value?.type;
        if (ownerType) {
          await loadHistory(groupId, ownerType, actualTopicId);
        }
      }
    });
  };

  /**
   * 选择一个助手或群组，并自动跳转到最近的话题
   */
  const selectTopicById = async (itemId: string, topicId: string) => {
    // 立即更新 currentTopicId，确保话题列表高亮实时响应
    currentTopicId.value = topicId;

    const ownerType = assistantStore.agents.some((a) => a.id === itemId)
      ? "agent"
      : "group";

    // [关键] 在 loadHistory 之前设置 currentSelectedItem，保持与原始逻辑一致的时序
    const agent = assistantStore.agents.find((a: any) => a.id === itemId);
    const group = assistantStore.groups.find((g) => g.id === itemId);
    if (agent) {
      currentSelectedItem.value = { ...agent, type: "agent" };
    } else if (group) {
      currentSelectedItem.value = { ...group, type: "group" };
    }

    await loadHistoryPaginated(itemId, ownerType, topicId);
  };

  const selectItem = async (item: any) => {
    if (!item) return;
    
    const ownerId = item.id;
    const ownerType = item.members ? 'group' : 'agent';
    
    // 如果已经选中了该项，且当前已有话题，则不重复加载（除非需要强制刷新）
    if (currentSelectedItem.value?.id === ownerId && currentTopicId.value) {
      return;
    }

    // 1. 获取目标项的最新配置（含 currentTopicId）
    let targetTopicId = item.currentTopicId;

    // 2. 如果没有记录的话题，或者记录的话题已失效，则尝试获取该 Owner 下最新的话题
    if (!targetTopicId) {
      try {
        const topics = await invoke<any[]>("get_topics", {
          ownerId,
          ownerType,
        });
        if (topics && topics.length > 0) {
          // 列表通常按 updated_at 倒序，取第一个
          targetTopicId = topics[0].id || topics[0].topic_id;
        }
      } catch (e) {
        console.error("[ChatManager] Failed to fetch fallback topics:", e);
      }
    }

    if (targetTopicId) {
      await selectTopicById(ownerId, targetTopicId);
    } else {
      // 没有任何话题的极端情况
      console.warn(`[ChatManager] No topics found for ${ownerId}`);
      currentSelectedItem.value = { ...item, type: ownerType };
      currentChatHistory.value = [];
      currentTopicId.value = null;
    }
  };

  /**
   * 异步将实时计算出的 AST 块持久化到数据库
   */
  const persistMessageBlocks = async (messageId: string, blocks: ContentBlock[]) => {
    const msg = currentChatHistory.value.find((m) => m.id === messageId);
    if (!msg || !currentSelectedItem.value?.id || !currentTopicId.value) return;

    // 仅当内存中尚无预编译块，或内容发生变化时才持久化，避免无效写入
    msg.blocks = blocks;
    
    try {
      await invoke("patch_single_message", {
        ownerId: currentSelectedItem.value.id,
        ownerType: currentSelectedItem.value.type,
        topicId: currentTopicId.value,
        message: msg,
      });
      console.log(`[ChatManager] Persisted pre-computed blocks for ${messageId}`);
    } catch (e) {
      console.warn(`[ChatManager] Failed to persist blocks for ${messageId}:`, e);
    }
  };

  return {
    ensureEventListenersRegistered,
    currentChatHistory,
    currentSelectedItem,
    currentTopicId,
    loading,
    streamingMessageId,
    stagedAttachments,
    editMessageContent,
    editingOriginalMessageId,
    sessionActiveStreams,
    loadHistory,
    loadHistoryPaginated,
    loadMoreHistory,
    historyOffset,
    hasMoreHistory,
    isLoadingHistory,
    selectItem,
    selectTopicById,
    fetchRawContent,
    sendMessage,
    handleAttachment,
    deleteMessage,
    stopMessage,
    stopGroupTurn,
    updateMessageContent,
    regenerateResponse,
    isGroupGenerating,
    activeStreamingIds,
    persistMessageBlocks,
  };
});
