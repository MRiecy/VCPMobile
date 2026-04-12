import { defineStore } from "pinia";
import { ref, computed, nextTick } from "vue";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { useStreamManagerStore } from "./streamManager";
import { useSettingsStore } from "./settings";
import { useAssistantStore } from "./assistant";
import { useTopicStore } from "./topicListManager";
import { syncService } from "../utils/syncService";
import { useDocumentProcessor } from "../composables/useDocumentProcessor";

/**
 * Attachment 接口定义，严格对齐 Rust 端的 AttachmentSyncDTO / Attachment (仅保留核心字段)
 */
export interface Attachment {
  id?: string; // 纯前端 UI 稳定性标识 (Stable Key)
  type: string;
  name: string;
  size: number;
  progress?: number; // 0-100 的真实上传进度
  src: string; // 仅用于前端 UI 临时渲染路径 (如 file:// 或 blob:)
  resolvedSrc?: string; // Webview 可用的 asset:// 路径
  hash?: string;
  status?: string;
  internalPath?: string;
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
  content: string;
  timestamp: number;

  isThinking?: boolean;
  agentId?: string;
  groupId?: string;
  isGroupMessage?: boolean;
  attachments?: Attachment[];

  // 以下为纯前端运行时 UI 状态 (Ephemeral)，绝不进行持久化
  displayedContent?: string; // 用于平滑流式显示的文本内容 (打字机效果暂存)
  processedContent?: string; // 缓存 Rust 返回的 AST 或文本，避免重复解析
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

  // 核心：记录每个会话（itemId + topicId）是否处于活动流状态
  // 格式: "itemId:topicId" -> Set<messageId>
  const sessionActiveStreams = ref<Map<string, Set<string>>>(new Map());

  // 兼容旧逻辑的计算属性
  const activeStreamingIds = computed(() => {
    if (!currentSelectedItem.value?.id || !currentTopicId.value)
      return new Set<string>();
    const key = `${currentSelectedItem.value.id}:${currentTopicId.value}`;
    return sessionActiveStreams.value.get(key) || new Set<string>();
  });

  const isGroupGenerating = computed(() => {
    if (
      !currentSelectedItem.value?.id ||
      !currentTopicId.value ||
      currentSelectedItem.value.type !== "group"
    )
      return false;
    const key = `${currentSelectedItem.value.id}:${currentTopicId.value}`;
    const streams = sessionActiveStreams.value.get(key);
    return streams ? streams.size > 0 : false;
  });

  // 辅助方法：管理会话流状态
  const addSessionStream = (
    ownerId: string,
    topicId: string,
    messageId: string,
  ) => {
    const key = `${ownerId}:${topicId}`;
    if (!sessionActiveStreams.value.has(key)) {
      sessionActiveStreams.value.set(key, new Set());
    }
    sessionActiveStreams.value.get(key)!.add(messageId);
  };

  const removeSessionStream = (
    ownerId: string,
    topicId: string,
    messageId: string,
  ) => {
    const key = `${ownerId}:${topicId}`;
    const streams = sessionActiveStreams.value.get(key);
    if (streams) {
      streams.delete(messageId);
      if (streams.size === 0) {
        sessionActiveStreams.value.delete(key);
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

  // 用于拦截重新生成时的输入框补全
  const editMessageContent = ref("");

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
   * 加载聊天历史 (已优化：数据在 Rust 端完成预处理)
   */
  const loadHistory = async (
    ownerId: string,
    ownerType: string,
    topicId: string,
    limit: number = 50,
    offset: number = 0,
  ) => {
    console.log(
      `[ChatManager] Loading history for ${ownerId}, topic: ${topicId}`,
    );
    loading.value = true;
    try {
      const history = await invoke<ChatMessage[]>("load_chat_history", {
        ownerId,
        ownerType,
        topicId,
        limit,
        offset,
      });

      if (offset === 0) {
        currentChatHistory.value = history;

        // 恢复后台缓存中的流式内容 (如果用户切回了正在流式的话题)
        backgroundStreamingBuffers.value.forEach((buffer, msgId) => {
          if (buffer.topicId === topicId) {
            const msg = currentChatHistory.value.find((m) => m.id === msgId);
            if (msg) {
              console.log(
                `[ChatManager] Restoring background buffer for ${msgId} into current view.`,
              );
              msg.content += buffer.content;
              msg.isThinking = false;
              // 同时也同步给 streamManager 确保平滑显示
              streamManager.appendChunk(msgId, "", (text) => {
                msg.displayedContent = text;
              });
            }
          }
        });
      } else {
        currentChatHistory.value = [...history, ...currentChatHistory.value];
      }

      currentTopicId.value = topicId;

      if (
        !currentSelectedItem.value ||
        currentSelectedItem.value.id !== ownerId
      ) {
        currentSelectedItem.value = { id: ownerId, type: ownerType };
      }

      // 异步解析本地资源路径
      await Promise.all(
        history.map(async (msg) => {
          resolveMessageAssets(msg);
        }),
      );

      console.log(
        `[ChatManager] History loaded: ${history.length} messages (Pre-processed by Rust)`,
      );
    } catch (e) {
      console.error("[ChatManager] Failed to load history:", e);
    } finally {
      loading.value = false;
    }
  };

  /**
   * 保存聊天历史 (已弃用，迁移到 SQLite 自动保存)
   */
  const saveHistory = async () => {
    console.log("[ChatManager] saveHistory is obsolete. SQLite auto-saves.");
  };

  /**
   * 增量同步聊天历史 (优化: 直接重载全量历史, Rust SQLite 查询极快)
   */
  const syncHistoryWithDelta = async () => {
    if (!currentSelectedItem.value || !currentTopicId.value) return;

    try {
      const history = await invoke<ChatMessage[]>("load_chat_history", {
        ownerId: currentSelectedItem.value.id,
        ownerType: currentSelectedItem.value.type,
        topicId: currentTopicId.value,
      });

      // 合并流状态和最新数据
      for (const newMsg of history) {
        const existing = currentChatHistory.value.find(
          (m) => m.id === newMsg.id,
        );
        if (existing) {
          if (existing.id !== streamingMessageId.value) {
            Object.assign(existing, newMsg);
          }
        } else {
          currentChatHistory.value.push(newMsg);
        }
      }

      // 处理被删除的数据
      currentChatHistory.value = currentChatHistory.value.filter(
        (m) =>
          history.some((hm) => hm.id === m.id) ||
          m.id === streamingMessageId.value,
      );

      currentChatHistory.value.sort((a, b) => a.timestamp - b.timestamp);

      await Promise.all(currentChatHistory.value.map(resolveMessageAssets));
    } catch (e) {
      console.error("[ChatManager] Failed to sync delta:", e);
    }
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

    // 触发同步到桌面端
    syncService.pushTopicToDesktop(ownerId, topicId, currentChatHistory.value);
  };

  /**
   * 强行中止正在生成的流式请求
   */
  /**
   * 中止指定消息的生成
   */
  const stopMessage = async (messageId: string) => {
    console.log(
      `[ChatManager] Sending interrupt signal for message: ${messageId}`,
    );
    try {
      await invoke("interruptRequest", { message_id: messageId });
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
        // 触发同步到桌面端
        syncService.pushTopicToDesktop(
          ownerId,
          topicId,
          currentChatHistory.value,
        );
      }
    } catch (e) {
      console.error(
        `[ChatManager] Failed to interrupt stream for ${messageId}:`,
        e,
      );
    }
  };

  /**
   * 强行中止正在生成的流式请求 (兼容旧接口)
   */
  const stopGenerating = async () => {
    if (activeStreamingIds.value.size > 0) {
      // 中止所有当前活跃的流
      const ids = Array.from(activeStreamingIds.value);
      await Promise.all(ids.map((id) => stopMessage(id as string)));
    }
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
    // 清理可能存在的显示缓存，确保触发重新渲染
    if (msg.displayedContent) {
      msg.displayedContent = "";
    }
    msg.processedContent = undefined;
    if (currentSelectedItem.value?.id && currentTopicId.value) {
      await invoke("patch_single_message", {
        ownerId: currentSelectedItem.value.id,
        ownerType: currentSelectedItem.value.type,
        topicId: currentTopicId.value,
        message: msg,
      });
      // 触发同步到桌面端
      syncService.pushTopicToDesktop(
        currentSelectedItem.value.id,
        currentTopicId.value,
        currentChatHistory.value,
      );
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

    const agentId = currentSelectedItem.value.id;
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
      content,
      timestamp: now,
      attachments: currentStaged.length > 0 ? currentStaged : undefined,
    };

    currentChatHistory.value.push(userMsg);

    // 构造 AI 思考占位消息
    const thinkingId = `msg_${now}_assistant_${Math.random().toString(36).substring(2, 7)}`;
    const thinkingMsg: ChatMessage = {
      id: thinkingId,
      role: "assistant",
      content: "",
      timestamp: now,
      isThinking: true,
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

        // 触发同步到桌面端
        syncService.pushTopicToDesktop(
          currentSelectedItem.value.id,
          currentTopicId.value,
          currentChatHistory.value,
        );
      }

      const settings = settingsStore.settings;
      if (!settings) {
        throw new Error("应用尚未完成初始化，缺少设置数据，无法发送消息");
      }

      const vcpUrl = settings.vcpServerUrl || "";
      const vcpApiKey = settings.vcpApiKey || "";

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
        // 直接调用 Rust 端群组调度器，不再设置前端硬超时
        await invoke("handle_group_chat_message", { payload: groupPayload });

        // 注意：这里不再立即移除 thinkingId，由后续的 vcp-stream type='end' 或 type='error' 来清理
        // 或者由下一次 loadHistory/syncHistory 全量覆盖
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
      await invoke("handle_agent_chat_message", { payload: agentPayload });
    } catch (e) {
      console.error("[ChatManager] Failed to send message:", e);

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
        // Fallback if message was somehow lost
        currentChatHistory.value.push({
          id: `msg_${Date.now()}_system_error`,
          role: "system",
          content: errorText.trim(),
          timestamp: Date.now(),
        });
      }

      streamingMessageId.value = null;
      if (currentSelectedItem.value?.id && currentTopicId.value) {
        // 查找当前的思考占位消息
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
   * 重新生成消息
   * @param targetMessageId 用户想要重新生成的 AI 回复的 ID
   */
  const regenerateResponse = async (targetMessageId: string) => {
    // 1. 查找此 AI 消息前的一条 用户消息
    const targetIndex = currentChatHistory.value.findIndex(
      (m) => m.id === targetMessageId,
    );
    if (targetIndex === -1) return;

    // 我们采取“时间回退”策略，将聊天截断到这条 AI 消息之前，然后触发上一次的 prompt 再次请求
    // 注意：桌面端通常需要回溯找到最近的一条 user 消息来作为输入，但在我们的架构下，
    // 我们直接切片历史记录并发起空的 content 请求即可，因为 VCP 会自动拾取最新的完整 messages 数组

    await deleteMessage(targetMessageId, true);

    // 再次触发发送，留空内容即可，VCP 后端会用最后一句话作为基准续写
    await sendMessage("");
  };

  // --- 初始化与销毁 (Lifecycle) ---

  const ensureEventListenersRegistered = async () => {
    if (listenersRegistered) return;
    listenersRegistered = true;
    console.log("[ChatManager] Registering Tauri listeners");

    // 监听 AI 流式输出事件
    listen("vcp-stream", (event: any) => {
      // 适配 Rust 端默认序列化使用下划线命名法 (message_id)
      const {
        message_id,
        messageId: legacyMessageId,
        chunk,
        type,
        context,
      } = event.payload;
      const actualMessageId = message_id || legacyMessageId;
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
          isThinking: true,
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
            msg.content += textChunk;
            // 使用 streamManager 平滑更新 displayedContent
            streamManager.appendChunk(actualMessageId, textChunk, (text) => {
              const latestMsg = currentChatHistory.value.find(
                (m) => m.id === actualMessageId,
              );
              if (latestMsg) {
                latestMsg.displayedContent = text;
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
            streamManager.appendChunk(actualMessageId, textChunk, () => {});
          }
        }
      } else if (type === "end" || type === "error") {
        const errorMsg = event.payload.error;
        console.log(
          `[ChatManager] Stream ${type} for ${actualMessageId}${errorMsg ? ": " + errorMsg : ""}. Draining queue...`,
        );

        if (msg) {
          msg.isThinking = false;
        }

        // 流式结束时，等待 streamManager 缓冲队列排空后再切换状态
        streamManager.finalizeStream(actualMessageId, async () => {
          const latestMsg = currentChatHistory.value.find(
            (m) => m.id === actualMessageId,
          );
          if (latestMsg) {
            // 确保最终内容一致
            latestMsg.displayedContent = latestMsg.content;
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
    });

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
        await syncHistoryWithDelta();
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
        await syncHistoryWithDelta();
      }
    });
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
    sessionActiveStreams,
    loadHistory,
    saveHistory,
    syncHistoryWithDelta,
    sendMessage,
    handleAttachment,
    deleteMessage,
    stopMessage,
    stopGenerating,
    updateMessageContent,
    regenerateResponse,
    isGroupGenerating,
    activeStreamingIds,
  };
});
