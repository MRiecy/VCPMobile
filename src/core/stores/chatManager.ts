import { defineStore } from 'pinia';
import { ref, computed } from 'vue';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

import { useStreamManagerStore } from './streamManager';
import { useSettingsStore } from './settings';
import { useAssistantStore } from './assistant';
import { useModelStore } from './modelStore';
import { useTopicStore } from './topicListManager';

/**
 * Attachment 接口定义
 */
export interface Attachment {
  type: string;
  src: string;
  resolvedSrc?: string; // 已在 Rust 端预转换的 asset:// 路径
  name: string;
  size: number;
  hash?: string;
  extractedText?: string;
}

/**
 * ChatMessage 接口定义，与 Rust 端 ChatMessage 结构保持对齐
 */
export interface ChatMessage {
  id: string;
  role: string;
  name?: string;
  content: string;
  displayedContent?: string; // 用于平滑流式显示的文本内容
  processedContent?: string; // Rust 正则清洗后的成品内容
  timestamp: number;
  isThinking?: boolean;
  avatarUrl?: string;
  avatarColor?: string; // 兼容旧版历史记录中的气泡颜色
  resolvedAvatarUrl?: string; // 已在 Rust 端预转换的 asset:// 路径
  attachments?: Attachment[];
  extra?: Record<string, any>;
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
export const useChatManagerStore = defineStore('chatManager', () => {
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
    if (!currentSelectedItem.value?.id || !currentTopicId.value) return new Set<string>();
    const key = `${currentSelectedItem.value.id}:${currentTopicId.value}`;
    return sessionActiveStreams.value.get(key) || new Set<string>();
  });

  const isGroupGenerating = computed(() => {
    if (!currentSelectedItem.value?.id || !currentTopicId.value || currentSelectedItem.value.type !== 'group') return false;
    const key = `${currentSelectedItem.value.id}:${currentTopicId.value}`;
    const streams = sessionActiveStreams.value.get(key);
    return streams ? streams.size > 0 : false;
  });

  // 辅助方法：管理会话流状态
  const addSessionStream = (itemId: string, topicId: string, messageId: string) => {
    const key = `${itemId}:${topicId}`;
    if (!sessionActiveStreams.value.has(key)) {
      sessionActiveStreams.value.set(key, new Set());
    }
    sessionActiveStreams.value.get(key)!.add(messageId);
  };

  const removeSessionStream = (itemId: string, topicId: string, messageId: string) => {
    const key = `${itemId}:${topicId}`;
    const streams = sessionActiveStreams.value.get(key);
    if (streams) {
      streams.delete(messageId);
      if (streams.size === 0) {
        sessionActiveStreams.value.delete(key);
      }
    }
  };

  // 非当前视图流的轻量级内容缓存 (messageId -> { content: string, topicId: string, itemId: string })
  const backgroundStreamingBuffers = ref<Map<string, { content: string, topicId: string, itemId: string }>>(new Map());

  // 用于高性能同步的指纹缓存
  const lastTopicFingerprint = ref<TopicFingerprint | null>(null);

  // 暂存的附件列表，准备随下一条消息发送
  const stagedAttachments = ref<Attachment[]>([]);

  const streamManager = useStreamManagerStore();
  const settingsStore = useSettingsStore();
  const assistantStore = useAssistantStore();
  const modelStore = useModelStore();
  const topicStore = useTopicStore();

  // 用于拦截重新生成时的输入框补全
  const editMessageContent = ref('');

  let listenersRegistered = false;

  /**
   * 尝试为话题生成 AI 总结标题 (对齐桌面端 attemptTopicSummarization)
   */
  const summarizeTopic = async () => {
    if (!currentTopicId.value || !currentSelectedItem.value?.id) return;

    const topicId = currentTopicId.value;
    const itemId = currentSelectedItem.value.id;

    // 只有“未命名”话题且消息数达到阈值才总结 (桌面端策略)
    const topic = topicStore.topics.find(t => t.id === topicId);
    const isUnnamed = !topic || topic.name.includes('新话题') || topic.name.includes('topic_') || topic.name.includes('group_topic_') || topic.name === '主要群聊';
    const messageCount = currentChatHistory.value.filter(m => m.role !== 'system').length;

    if (isUnnamed && messageCount >= 4) {
      console.log(`[ChatManager] Triggering AI summary for topic: ${topicId}`);
      try {
        const agentName = assistantStore.agents.find((a: any) => a.id === itemId)?.name || 'AI';
        const newTitle = await invoke<string>('summarize_topic', {
          itemId,
          topicId,
          agentName
        });

        if (newTitle) {
          console.log(`[ChatManager] AI Summarized Title: ${newTitle}`);
          await topicStore.updateTopicTitle(itemId, topicId, newTitle);
        }
      } catch (e) {
        console.error('[ChatManager] AI Summary failed:', e);
      }
    }
  };

  /**
   * 处理消息中的本地资源路径 (头像、附件)，使用 Tauri 原生 asset:// 协议绕过 WebView 限制
   */
  const resolveMessageAssets = (msg: ChatMessage) => {
    // 处理头像
    if (msg.avatarUrl && !msg.avatarUrl.startsWith('http') &&
      !msg.avatarUrl.startsWith('data:')) {
      try {
        msg.resolvedAvatarUrl = convertFileSrc(msg.avatarUrl);
      } catch (err) {
        console.warn(`[ChatManager] Failed to convert avatar path for message ${msg.id}:`, err);
      }
    }

    // 处理附件 (仅处理图片类型)
    if (msg.attachments && msg.attachments.length > 0) {
      msg.attachments.forEach((att) => {
        if (att.type.startsWith('image/') && att.src && !att.src.startsWith('http') &&
          !att.src.startsWith('data:')) {
          try {
            att.resolvedSrc = convertFileSrc(att.src);
          } catch (err) {
            console.warn(`[ChatManager] Failed to convert attachment image path ${att.name}:`, err);
          }
        }
      });
    }
  };

  /**
   * 触发原生文件选择器并暂存附件
   */
  const handleAttachment = async () => {
    try {
      // 调用 Rust 端原生的文件选择和存储逻辑
      const attachmentData = await invoke<any>('pick_and_store_attachment');

      if (attachmentData) {
        console.log('[ChatManager] Attachment picked and stored:', attachmentData);

        // 将后端返回的元数据转为前端格式并推入暂存区
        stagedAttachments.value.push({
          type: attachmentData.mime_type,
          src: attachmentData.internal_path,
          name: attachmentData.name,
          size: attachmentData.size,
          hash: attachmentData.hash,
        });
      }
    } catch (e) {
      console.error('[ChatManager] Failed to pick or store attachment:', e);
      // TODO: 添加 Toast 提示用户
    }
  };

  /**
   * 对消息应用正则清洗 (Rust 下沉逻辑)
   */
  const processRegex = async (msg: ChatMessage, agentId: string) => {
    // 只有 assistant 消息或需要清洗的用户消息才处理，且避免重复处理
    const contentToProcess = msg.content;
    if (msg.processedContent || !contentToProcess) return;

    // 计算深度 (对齐桌面端逻辑)
    const index = currentChatHistory.value.findIndex(m => m.id === msg.id);
    const depth = index === -1 ? 0 : currentChatHistory.value.length - 1 - index;

    try {
      const processed = await invoke<string>('process_regex_for_message', {
        agentId,
        content: contentToProcess,
        scope: 'frontend',
        role: msg.role,
        depth: depth,
      });
      msg.processedContent = processed;
    } catch (e) {
      console.error('[ChatManager] Regex processing failed:', e);
      msg.processedContent = contentToProcess; // 降级处理
    }
  };

  /**
   * 加载聊天历史 (已优化：数据在 Rust 端完成预处理)
   */
  const loadHistory = async (itemId: string, topicId: string, limit: number = 50, offset: number = 0) => {
    console.log(`[ChatManager] Loading history for ${itemId}, topic: ${topicId}`);
    loading.value = true;
    try {
      const history = await invoke<ChatMessage[]>('load_chat_history', {
        itemId,
        topicId,
        limit,
        offset
      });

      if (offset === 0) {
        currentChatHistory.value = history;
        
        // 恢复后台缓存中的流式内容 (如果用户切回了正在流式的话题)
        backgroundStreamingBuffers.value.forEach((buffer, msgId) => {
          if (buffer.topicId === topicId) {
            const msg = currentChatHistory.value.find(m => m.id === msgId);
            if (msg) {
              console.log(`[ChatManager] Restoring background buffer for ${msgId} into current view.`);
              msg.content += buffer.content;
              msg.isThinking = false;
              // 同时也同步给 streamManager 确保平滑显示
              streamManager.appendChunk(msgId, '', (text) => {
                msg.displayedContent = text;
              });
            }
          }
        });
      } else {
        currentChatHistory.value = [...history, ...currentChatHistory.value];
      }

      currentTopicId.value = topicId;

      if (!currentSelectedItem.value || currentSelectedItem.value.id !== itemId) {
        currentSelectedItem.value = { id: itemId };
      }

      // 异步预处理正则并解析本地资源路径
      await Promise.all(history.map(async (msg) => {
        resolveMessageAssets(msg);
        await processRegex(msg, itemId);
      }));

      // 获取并更新初始指纹，用于后续同步
      lastTopicFingerprint.value = await invoke<TopicFingerprint>('get_topic_fingerprint', {
        itemId,
        topicId
      });

      console.log(`[ChatManager] History loaded: ${history.length} messages (Pre-processed by Rust)`);
    } catch (e) {
      console.error('[ChatManager] Failed to load history:', e);
    } finally {
      loading.value = false;
    }
  };

  /**
   * 保存聊天历史
   */
  const saveHistory = async () => {
    if (!currentSelectedItem.value || !currentTopicId.value) return;

    const itemId = currentSelectedItem.value.id;
    const topicId = currentTopicId.value;

    try {
      await invoke('signal_internal_save');
      await invoke('save_chat_history', {
        itemId,
        topicId,
        history: currentChatHistory.value,
      });

      // 保存后立即刷新指纹，防止刚刚保存的动作触发外部同步
      lastTopicFingerprint.value = await invoke<TopicFingerprint>('get_topic_fingerprint', {
        itemId,
        topicId
      });
    } catch (e) {
      console.error('[ChatManager] Failed to save history:', e);
    }
  };

  /**
   * 增量同步聊天历史 (已优化：指纹预检 + Rust 预处理)
   */
  const syncHistoryWithDelta = async () => {
    if (!currentSelectedItem.value || !currentTopicId.value) return;

    const itemId = currentSelectedItem.value.id;
    const topicId = currentTopicId.value;

    try {
      // 1. 获取最新指纹并与本地缓存比对
      const newFingerprint = await invoke<TopicFingerprint>('get_topic_fingerprint', {
        itemId,
        topicId
      });

      if (lastTopicFingerprint.value &&
        newFingerprint.mtime === lastTopicFingerprint.value.mtime &&
        newFingerprint.size === lastTopicFingerprint.value.size &&
        newFingerprint.msg_count === currentChatHistory.value.length) {
        return;
      }

      console.log(`[ChatManager] Fingerprint mismatch, calculating delta for topic: ${topicId}`);

      // 2. 获取 Rust 端计算出的差异块
      const delta = await invoke<TopicDelta>('get_topic_delta', {
        itemId,
        topicId,
        currentHistory: currentChatHistory.value,
        fingerprint: lastTopicFingerprint.value
      });

      if (delta.sync_skipped) {
        lastTopicFingerprint.value = newFingerprint;
        return;
      }

      if (delta.added.length === 0 && delta.updated.length === 0 && delta.deleted_ids.length === 0) {
        lastTopicFingerprint.value = newFingerprint;
        return;
      }

      // 3. 处理删除的消息
      if (delta.deleted_ids.length > 0) {
        currentChatHistory.value = currentChatHistory.value.filter(
          m => !delta.deleted_ids.includes(m.id)
        );
      }

      // 4. 处理更新的消息
      for (const updatedMsg of delta.updated) {
        if (updatedMsg.id === streamingMessageId.value) {
          const index = currentChatHistory.value.findIndex(m => m.id === updatedMsg.id);
          if (index > -1) {
            const { content, displayedContent, ...meta } = updatedMsg;
            currentChatHistory.value[index] = {
              ...currentChatHistory.value[index],
              ...meta
            };
          }
          continue;
        }

        const index = currentChatHistory.value.findIndex(m => m.id === updatedMsg.id);
        if (index > -1) {
          currentChatHistory.value[index] = {
            ...currentChatHistory.value[index],
            ...updatedMsg
          };
        }
      }

      // 5. 处理新增的消息
      for (const addedMsg of delta.added) {
        if (!currentChatHistory.value.some(m => m.id === addedMsg.id)) {
          currentChatHistory.value.push(addedMsg);
        }
      }

      // 6. 重新排序以确保时间轴一致
      currentChatHistory.value.sort((a, b) => a.timestamp - b.timestamp);

      // 更新指纹缓存
      lastTopicFingerprint.value = newFingerprint;

      console.log(`[ChatManager] Delta sync complete (Optimized). Changes: +${delta.added.length} / ~${delta.updated.length} / -${delta.deleted_ids.length}`);
    } catch (e) {
      console.error('[ChatManager] Delta sync failed:', e);
    }
  };

  /**
   * 删除指定消息及之后的所有消息 (通常用于重新生成或回退)
   * 如果 deleteAfter 为 true，则相当于时间回溯
   */
  const deleteMessage = async (messageId: string, deleteAfter: boolean = false) => {
    if (!currentSelectedItem.value || !currentTopicId.value) return;

    const targetIndex = currentChatHistory.value.findIndex(m => m.id === messageId);
    if (targetIndex === -1) return;

    if (deleteAfter) {
      // 删除自身以及后面所有的
      currentChatHistory.value.splice(targetIndex);
    } else {
      // 仅删除自身
      currentChatHistory.value.splice(targetIndex, 1);
    }

    // 触发保存与文件同步
    await saveHistory();
  };

  /**
   * 强行中止正在生成的流式请求
   */
  /**
   * 中止指定消息的生成
   */
  const stopMessage = async (messageId: string) => {
    console.log(`[ChatManager] Sending interrupt signal for message: ${messageId}`);
    try {
      await invoke('interruptRequest', { message_id: messageId });
      // 本地伪造一个 end 事件，防止假死
      streamManager.finalizeStream(messageId);

      // 确保清理状态
      const msgIndex = currentChatHistory.value.findIndex(m => m.id === messageId);
      if (msgIndex !== -1) {
        const msg = currentChatHistory.value[msgIndex];
        msg.isThinking = false;
      }

      if (currentSelectedItem.value?.id && currentTopicId.value) {
        removeSessionStream(currentSelectedItem.value.id, currentTopicId.value, messageId);
      }

      if (streamingMessageId.value === messageId) {
        streamingMessageId.value = null;
      }
      await saveHistory();
    } catch (e) {
      console.error(`[ChatManager] Failed to interrupt stream for ${messageId}:`, e);
    }
  };

  /**
   * 强行中止正在生成的流式请求 (兼容旧接口)
   */
  const stopGenerating = async () => {
    if (activeStreamingIds.value.size > 0) {
      // 中止所有当前活跃的流
      const ids = Array.from(activeStreamingIds.value);
      await Promise.all(ids.map(id => stopMessage(id as string)));
    }
  };

  /**
   * 更新某条消息的内容（用于全屏编辑消息）
   */
  const updateMessageContent = async (messageId: string, newContent: string) => {
    const msg = currentChatHistory.value.find(m => m.id === messageId);
    if (!msg) return;

    msg.content = newContent;
    // 清理可能存在的显示缓存，确保触发重新渲染
    if (msg.displayedContent) {
      msg.displayedContent = '';
    }
    msg.processedContent = undefined;

    await saveHistory();
  };

  /**
   * 重新生成回复 (历史切片回溯 + 无参请求)
   */
  const sendMessage = async (content: string) => {
    if (!currentSelectedItem.value || !currentTopicId.value || (!content.trim() && stagedAttachments.value.length === 0)) return;

    const agentId = currentSelectedItem.value.id;
    const currentStaged = [...stagedAttachments.value];

    // 构造用户消息
    const now = Date.now();
    const userMsg: ChatMessage = {
      id: `msg_${now}_user_${Math.random().toString(36).substring(2, 7)}`,
      role: 'user',
      content,
      timestamp: now,
      attachments: currentStaged.length > 0 ? currentStaged : undefined,
    };

    currentChatHistory.value.push(userMsg);

    // 清空暂存区
    stagedAttachments.value = [];

    // 构造 AI 思考占位消息
    const thinkingId = `msg_${now}_assistant_${Math.random().toString(36).substring(2, 7)}`;
    const thinkingMsg: ChatMessage = {
      id: thinkingId,
      role: 'assistant',
      content: '',
      timestamp: now,
      isThinking: true,
    };

    currentChatHistory.value.push(thinkingMsg);
    streamingMessageId.value = thinkingId;
    if (currentSelectedItem.value?.id && currentTopicId.value) {
      addSessionStream(currentSelectedItem.value.id, currentTopicId.value, thinkingId);
    }

    try {
      // 立即保存一次历史记录 (包含用户消息和思考态)
      await saveHistory();

      const settings = settingsStore.settings;
      if (!settings) {
        throw new Error('应用尚未完成初始化，缺少设置数据，无法发送消息');
      }

      const vcpUrl = settings.vcpServerUrl || '';
      const vcpApiKey = settings.vcpApiKey || '';

      // --- 群组消息路由 ---
      if (currentSelectedItem.value?.type === 'group') {
        const groupId = currentSelectedItem.value.id;

        const groupPayload = {
          groupId,
          topicId: currentTopicId.value,
          userMessage: userMsg,
          vcpUrl,
          vcpApiKey
        };

        console.log('[ChatManager] Sending group payload:', groupPayload);
        // 直接调用 Rust 端群组调度器，不再设置前端硬超时
        await invoke('handle_group_chat_message', { payload: groupPayload });

        // 注意：这里不再立即移除 thinkingId，由后续的 vcp-stream type='end' 或 type='error' 来清理
        // 或者由下一次 loadHistory/syncHistory 全量覆盖
        return;
      }

      // --- 普通单 Agent 消息逻辑 ---
      let agentConfig = assistantStore.agents.find((a: any) => a.id === agentId);
      if (!agentConfig) {
        agentConfig = await invoke('read_agent_config', { agentId, allowDefault: true });
      }

      const messagesForVcp: any[] = [];
      if (agentConfig?.systemPrompt) {
        let systemPrompt = agentConfig.systemPrompt;
        systemPrompt = systemPrompt.replace(/\{\{AgentName\}\}/g, agentConfig.name || 'AI');
        messagesForVcp.push({ role: 'system', content: systemPrompt });
      }

      // 核心：处理历史记录并转换多模态 Payload
      const historyForVcp = await Promise.all(currentChatHistory.value
        .filter(m => !m.isThinking)
        .map(async m => {
          // 如果没有附件且是纯文本，保持简单格式
          if (!m.attachments || m.attachments.length === 0) {
            return { role: m.role, content: m.content, name: m.name };
          }

          const contentParts: any[] = [];
          let combinedText = m.content;

          for (const att of m.attachments) {
            // 1. 文本提取注入 (对齐桌面端)
            if (att.extractedText) {
              combinedText += `\n\n[附加文件: ${att.name}]\n${att.extractedText}\n[/附加文件结束: ${att.name}]`;
            }

            // 2. 多模态识别 (图片/音频/视频)
            const isImage = att.type.startsWith('image/');
            const isAudio = att.type.startsWith('audio/');
            const isVideo = att.type.startsWith('video/');

            if (isImage || isAudio || isVideo) {
              // 关键重构：不再在前端读 Base64，而是传引用，由 Rust 后端在发送前动态读取
              // 这样做极大减少了 IPC 通讯的内存压力，同时也绕过了 50MB 的限制 (Rust 端目前支持更大)
              contentParts.push({
                type: 'local_file',
                path: att.src,
                mime: att.type
              });
            } else if (!att.extractedText) {
              // 既非文本提取也非多模态（如普通压缩包），仅注入路径占位
              combinedText += `\n\n[附加文件: ${att.name}] (不支持直接读取内容)`;
            }
          }

          if (combinedText.trim()) {
            contentParts.unshift({ type: 'text', text: combinedText });
          }

          return {
            role: m.role,
            content: contentParts.length > 0 ? contentParts : m.content,
            name: m.name
          };
        }));

      messagesForVcp.push(...historyForVcp);

      const payload = {
        vcpUrl,
        vcpApiKey,
        messages: messagesForVcp,
        modelConfig: {
          model: agentConfig?.model || 'gemini-2.0-flash',
          temperature: agentConfig?.temperature ?? 0.7,
          top_p: agentConfig?.topP,
          top_k: agentConfig?.topK,
          max_tokens: agentConfig?.maxOutputTokens,
          contextTokenLimit: agentConfig?.contextTokenLimit,
          stream: true,
        },
        messageId: thinkingId,
        context: { agentId, topicId: currentTopicId.value }
      };

      console.log('[ChatManager] Sending payload to VCP:', payload);

      if (payload.modelConfig.model) {
        modelStore.recordUsage(payload.modelConfig.model);
      }

      await invoke('sendToVCP', { payload });
    } catch (e) {
      console.error('[ChatManager] Failed to send message:', e);

      const errorText = `\n\n> VCP错误: ${e instanceof Error ? e.message : String(e)}`;

      const msgIndex = currentChatHistory.value.findIndex(m => m.id === thinkingId);
      if (msgIndex !== -1) {
        const msg = currentChatHistory.value[msgIndex];
        msg.isThinking = false;
        msg.content += errorText;
        if (msg.displayedContent !== undefined) {
          msg.displayedContent += errorText;
        }
        if (currentSelectedItem.value?.id && currentTopicId.value) {
          removeSessionStream(currentSelectedItem.value.id, currentTopicId.value, thinkingId);
        }
      } else {
        // Fallback if message was somehow lost
        currentChatHistory.value.push({
          id: `msg_${Date.now()}_system_error`,
          role: 'system',
          content: errorText.trim(),
          timestamp: Date.now()
        });
      }

      streamingMessageId.value = null;
      await saveHistory();
    }
  };

  /**
   * 重新生成消息
   * @param targetMessageId 用户想要重新生成的 AI 回复的 ID
   */
  const regenerateResponse = async (targetMessageId: string) => {
    // 1. 查找此 AI 消息前的一条 用户消息
    const targetIndex = currentChatHistory.value.findIndex(m => m.id === targetMessageId);
    if (targetIndex === -1) return;

    // 我们采取“时间回退”策略，将聊天截断到这条 AI 消息之前，然后触发上一次的 prompt 再次请求
    // 注意：桌面端通常需要回溯找到最近的一条 user 消息来作为输入，但在我们的架构下，
    // 我们直接切片历史记录并发起空的 content 请求即可，因为 VCP 会自动拾取最新的完整 messages 数组

    await deleteMessage(targetMessageId, true);

    // 再次触发发送，留空内容即可，VCP 后端会用最后一句话作为基准续写
    await sendMessage('');
  };

  // --- 初始化与销毁 (Lifecycle) ---

  const ensureEventListenersRegistered = async () => {
    if (listenersRegistered) return;
    listenersRegistered = true;
    console.log('[ChatManager] Registering Tauri listeners');

    // 监听 AI 流式输出事件
    listen('vcp-stream', (event: any) => {
      // 适配 Rust 端默认序列化使用下划线命名法 (message_id)
      const { message_id, messageId: legacyMessageId, chunk, type, context } = event.payload;
      const actualMessageId = message_id || legacyMessageId;
      // 无论是否在当前视图，都尝试更新数据
      let msg = currentChatHistory.value.find(m => m.id === actualMessageId);
      const ctx = context || {};
      const topicId = ctx.topicId || currentTopicId.value;
      const itemId = ctx.agentId || ctx.groupId || currentSelectedItem.value?.id;

      // [关键修复] 如果是群聊并行流，且消息尚未在 currentChatHistory 中（因为是 Rust 端刚发起的），
      // 我们需要根据 context 自动创建一个占位消息，以便立即展示流式内容。
      if (!msg && context && context.isGroupMessage && context.groupId === currentSelectedItem.value?.id) {
        console.log(`[ChatManager] Creating placeholder for group message: ${actualMessageId}`);
        msg = {
          id: actualMessageId,
          role: 'assistant',
          name: context.agentName,
          content: '',
          timestamp: Date.now(),
          isThinking: true,
          extra: {
            agentId: context.agentId
          }
        };
        currentChatHistory.value.push(msg);
        // 保持排序
        currentChatHistory.value.sort((a, b) => a.timestamp - b.timestamp);
      }
      
      if (type === 'data') {
        if (msg) {
          msg.isThinking = false;
        }
        
        // 确保在活动流集合中
        if (itemId && topicId) {
          addSessionStream(itemId, topicId, actualMessageId);
        }

        // 解析 chunk 提取文本内容
        let textChunk = '';
        if (typeof chunk === 'string') {
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
              const latestMsg = currentChatHistory.value.find(m => m.id === actualMessageId);
              if (latestMsg) {
                latestMsg.displayedContent = text;
              }
            });
          } else {
            // 2. 更新后台缓存 (如果不在当前视图)
            if (topicId && itemId) {
              const buffer = backgroundStreamingBuffers.value.get(actualMessageId) || { content: '', topicId, itemId };
              buffer.content += textChunk;
              backgroundStreamingBuffers.value.set(actualMessageId, buffer);
            }

            // 同时也喂给 streamManager，防止切回来时 buffer 为空
            streamManager.appendChunk(actualMessageId, textChunk, () => {});
          }
        }
      } else if (type === 'end' || type === 'error') {
        const errorMsg = event.payload.error;
        console.log(`[ChatManager] Stream ${type} for ${actualMessageId}${errorMsg ? ': ' + errorMsg : ''}. Draining queue...`);
        
        if (msg) {
          msg.isThinking = false;
        }
        
        // 流式结束时，等待 streamManager 缓冲队列排空后再切换状态
        streamManager.finalizeStream(actualMessageId, () => {
          const latestMsg = currentChatHistory.value.find(m => m.id === actualMessageId);
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
          
          if (type === 'error' && errorMsg && errorMsg !== '请求已中止') {
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
            // 重新获取一次最新引用进行正则处理
            if (currentSelectedItem.value?.id) {
              processRegex(latestMsg, currentSelectedItem.value.id);
            }
            saveHistory();
            // 话题自动总结逻辑
            summarizeTopic();
          } else {
            // 如果不在当前视图，从后台缓存中提取并尝试保存一次
            const buffer = backgroundStreamingBuffers.value.get(actualMessageId);
            if (buffer) {
              console.log(`[ChatManager] Finalizing background stream for ${actualMessageId}, triggering silent save.`);
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
    listen('vcp-file-change', async (event: any) => {
      const paths = event.payload as string[];
      console.log('[ChatManager] File change detected by Rust Watcher:', paths);

      if (!currentTopicId.value || !currentSelectedItem.value?.id) return;

      // 检查变更的文件路径是否包含当前正在查看的 topicId
      const isCurrentTopicChanged = paths.some(p =>
        p.includes(currentTopicId.value!) && p.endsWith('history.json')
      );

      if (isCurrentTopicChanged) {
        console.log(`[ChatManager] Current topic ${currentTopicId.value} history changed externally. Syncing...`);
        await syncHistoryWithDelta();
      }
    });

    listen('vcp-group-turn-finished', (event: any) => {
      const { groupId, topic_id, topicId: legacyTopicId } = event.payload;
      const actualTopicId = topic_id || legacyTopicId;

      if (groupId === currentSelectedItem.value?.id && actualTopicId === currentTopicId.value) {
        console.log(`[ChatManager] Group turn finished for ${groupId}`);
        // 强制同步一次，确保所有并行 Agent 的最终结果都已落盘并同步到前端
        syncHistoryWithDelta();
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
    activeStreamingIds
  };
});
