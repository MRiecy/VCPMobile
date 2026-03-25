import { defineStore } from 'pinia';
import { ref, onMounted, onUnmounted } from 'vue';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';

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
  resolvedSrc?: string; // 用于 WebView 渲染的 asset:// 路径
  name: string;
  size: number;
  hash?: string;
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
  processedContent?: string; // 缓存正则清洗后的内容
  timestamp: number;
  isThinking?: boolean; // 修正为驼峰命名，对齐桌面端 history.json
  avatarUrl?: string;   // 桌面端扁平化传递的头像路径
  avatarColor?: string; // 桌面端扁平化传递的头像颜色
  resolvedAvatarUrl?: string; // 用于 WebView 渲染的 asset:// 路径
  attachments?: Attachment[];
  extra?: Record<string, any>;
}

/**
 * TopicDelta 接口定义，用于增量同步
 */
export interface TopicDelta {
  added: ChatMessage[];
  updated: ChatMessage[];
  deleted_ids: string[];
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
  const coreStatus = ref<'active' | 'error' | 'loading'>('loading');
  const coreErrorMsg = ref('');
  const streamingMessageId = ref<string | null>(null);
  const isGroupGenerating = ref(false);
  
  // 暂存的附件列表，准备随下一条消息发送
  const stagedAttachments = ref<Attachment[]>([]);
  
  const streamManager = useStreamManagerStore();
  const settingsStore = useSettingsStore();
  const assistantStore = useAssistantStore();
  const modelStore = useModelStore();
  const topicStore = useTopicStore();

  // 用于拦截重新生成时的输入框补全
  const editMessageContent = ref('');
  
  // 用于取消监听的清理函数
  let unlistenStreamPromise: Promise<UnlistenFn> | null = null;
  let unlistenFileChangePromise: Promise<UnlistenFn> | null = null;
  let unlistenGroupFinishedPromise: Promise<UnlistenFn> | null = null;

  /**
   * 尝试为话题生成 AI 总结标题 (对齐桌面端 attemptTopicSummarization)
   */
  const summarizeTopic = async () => {
    if (!currentTopicId.value || !currentSelectedItem.value?.id) return;
    
    const topicId = currentTopicId.value;
    const itemId = currentSelectedItem.value.id;
    
    // 只有“未命名”话题且消息数达到阈值才总结 (桌面端策略)
    const topic = topicStore.topics.find(t => t.id === topicId);
    const isUnnamed = !topic || topic.name.includes('新话题') || topic.name.includes('topic_') || topic.name === '主要群聊';
    const messageCount = currentChatHistory.value.filter(m => m.role !== 'system').length;

    if (isUnnamed && messageCount >= 4) {
      console.log(`[ChatManager] Triggering AI summary for topic: ${topicId}`);
      try {
        const agentName = assistantStore.agents.find(a => a.id === itemId)?.name || 'AI';
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
    if (msg.avatarUrl && !msg.avatarUrl.startsWith('http') && !msg.avatarUrl.startsWith('data:')) {
      try {
        msg.resolvedAvatarUrl = convertFileSrc(msg.avatarUrl);
      } catch (err) {
        console.warn(`[ChatManager] Failed to convert avatar path for message ${msg.id}:`, err);
      }
    }
    
    // 处理附件 (仅处理图片类型)
    if (msg.attachments && msg.attachments.length > 0) {
      msg.attachments.forEach((att) => {
        if (att.type.startsWith('image/') && att.src && !att.src.startsWith('http') && !att.src.startsWith('data:')) {
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
      // TODO: 可以在这里添加 Toast 提示用户
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
   * 加载聊天历史 (支持分页加载)
   */
  const loadHistory = async (itemId: string, topicId: string, limit: number = 50, offset: number = 0) => {
    console.log(`[ChatManager] Loading history for ${itemId}, topic: ${topicId}, limit: ${limit}, offset: ${offset}`);
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
      } else {
        // 如果是加载更早的历史记录，我们将其前置拼接到当前历史记录的最前面
        currentChatHistory.value = [...history, ...currentChatHistory.value];
      }
      
      currentTopicId.value = topicId;
      
      // 默认选中项初始化 (如果尚未设置)
      if (!currentSelectedItem.value || currentSelectedItem.value.id !== itemId) {
        currentSelectedItem.value = { id: itemId };
      }

      // 异步预处理正则并解析本地资源路径
      await Promise.all(history.map(async (msg) => {
        resolveMessageAssets(msg);
        await processRegex(msg, itemId);
      }));
      
      console.log(`[ChatManager] History loaded: ${history.length} messages`);
    } catch (e) {
      console.error('[ChatManager] Failed to load history:', e);
    } finally {
      loading.value = false;
    }
  };

  /**
   * 保存聊天历史
   * 在保存前会发出 signal_internal_save 信号，防止文件监听器触发自循环同步
   */
  const saveHistory = async () => {
    if (!currentSelectedItem.value || !currentTopicId.value) return;
    
    const itemId = currentSelectedItem.value.id;
    const topicId = currentTopicId.value;

    try {
      console.log(`[ChatManager] Internal save triggered for ${itemId}/${topicId}`);
      // 1. 发出内部保存信号 (Rust 端会记录时间戳)
      await invoke('signal_internal_save');
      
      // 2. 执行保存操作
      await invoke('save_chat_history', {
        itemId,
        topicId,
        history: currentChatHistory.value,
      });
    } catch (e) {
      console.error('[ChatManager] Failed to save history:', e);
    }
  };

  /**
   * 增量同步聊天历史 (Delta Sync)
   * 对应桌面端的 syncHistoryFromFile 逻辑
   */
  const syncHistoryWithDelta = async () => {
    if (!currentSelectedItem.value || !currentTopicId.value) return;

    const itemId = currentSelectedItem.value.id;
    const topicId = currentTopicId.value;

    try {
      console.log(`[ChatManager] Syncing delta for topic: ${topicId}`);
      
      // 获取 Rust 端计算出的差异块
      const delta = await invoke<TopicDelta>('get_topic_delta', {
        itemId,
        topicId,
        currentHistory: currentChatHistory.value,
      });

      if (delta.added.length === 0 && delta.updated.length === 0 && delta.deleted_ids.length === 0) {
        console.log('[ChatManager] No changes detected, sync skipped.');
        return;
      }

      // 1. 处理删除的消息
      if (delta.deleted_ids.length > 0) {
        currentChatHistory.value = currentChatHistory.value.filter(
          m => !delta.deleted_ids.includes(m.id)
        );
      }

      // 2. 处理更新的消息
      for (const updatedMsg of delta.updated) {
        // 如果是当前正在流式输出的消息，我们要谨慎合并，防止覆盖前端正在平滑显示的内容
        if (updatedMsg.id === streamingMessageId.value) {
           const index = currentChatHistory.value.findIndex(m => m.id === updatedMsg.id);
           if (index > -1) {
             // 仅同步头像、附件等元数据，保留 content 和 displayedContent 由流式管线控制
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
            ...updatedMsg,
            processedContent: undefined, // 内容变更，重置缓存触发重算
          };
          resolveMessageAssets(currentChatHistory.value[index]);
          await processRegex(currentChatHistory.value[index], itemId);
        }
      }

      // 3. 处理新增的消息
      for (const addedMsg of delta.added) {
        // 简单去重保护
        if (!currentChatHistory.value.some(m => m.id === addedMsg.id)) {
          resolveMessageAssets(addedMsg);
          currentChatHistory.value.push(addedMsg);
          await processRegex(addedMsg, itemId);
        }
      }

      // 4. 重新排序以确保时间轴一致
      currentChatHistory.value.sort((a, b) => a.timestamp - b.timestamp);

      console.log(`[ChatManager] Delta sync complete. Changes: +${delta.added.length} / ~${delta.updated.length} / -${delta.deleted_ids.length}`);
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
  const stopGenerating = async () => {
    if (streamingMessageId.value) {
      console.log(`[ChatManager] Sending interrupt signal for message: ${streamingMessageId.value}`);
      try {
        await invoke('interruptRequest', { messageId: streamingMessageId.value });
        // 本地伪造一个 end 事件，防止假死
        streamManager.finalizeStream(streamingMessageId.value);

        // 确保清理状态及无用空消息
        const msgIndex = currentChatHistory.value.findIndex(m => m.id === streamingMessageId.value);
        if (msgIndex !== -1) {
          const msg = currentChatHistory.value[msgIndex];
          msg.isThinking = false;
          // 若是被中断在思考态且没有返回内容，直接清理消息
          if (!msg.content.trim()) {
            currentChatHistory.value.splice(msgIndex, 1);
          }
        }

        streamingMessageId.value = null;
        await saveHistory();
      } catch (e) {
        console.error('[ChatManager] Failed to interrupt stream:', e);
      }
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
    
    // 构造用户消息
    const userMsg: ChatMessage = {
      id: `user_${Date.now()}_${Math.random().toString(36).substring(2, 7)}`,
      role: 'user',
      content,
      timestamp: Date.now(),
      attachments: stagedAttachments.value.length > 0 ? [...stagedAttachments.value] : undefined,
    };
    
    currentChatHistory.value.push(userMsg);
    
    // 清空暂存区
    stagedAttachments.value = [];

    // 构造 AI 思考占位消息
    const thinkingId = `assistant_${Date.now()}_${Math.random().toString(36).substring(2, 7)}`;
    const thinkingMsg: ChatMessage = {
      id: thinkingId,
      role: 'assistant',
      content: '',
      timestamp: Date.now(),
      isThinking: true,
    };
    
    currentChatHistory.value.push(thinkingMsg);
    streamingMessageId.value = thinkingId;

    try {
      // 立即保存一次历史记录 (包含用户消息和思考态)
      await saveHistory();

      // 确保设置已加载
      if (!settingsStore.settings) {
        await settingsStore.fetchSettings();
      }
      
      const vcpUrl = settingsStore.settings?.vcpServerUrl || '';
      const vcpApiKey = settingsStore.settings?.vcpApiKey || '';

      // --- 群组消息路由 ---
      if (currentSelectedItem.value?.type === 'group') {
        const groupId = currentSelectedItem.value.id;
        isGroupGenerating.value = true;
        
        // 注意：群组模式下，多个 Agent 会轮流发言。
        // 我们不能简单地在这里清空 streamingMessageId，否则后续的流式事件会被拦截。
        // 但我们也需要允许第一个发言的 Agent 建立它自己的思考占位。
        
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
      let agentConfig = assistantStore.agents.find(a => a.id === agentId);
      if (!agentConfig) {
        agentConfig = await invoke('read_agent_config', { agentId, allowDefault: true });
      }

      const messagesForVcp: any[] = [];
      if (agentConfig?.systemPrompt) {
        let systemPrompt = agentConfig.systemPrompt;
        systemPrompt = systemPrompt.replace(/\{\{AgentName\}\}/g, agentConfig.name || 'AI');
        messagesForVcp.push({ role: 'system', content: systemPrompt });
      }

      const historyForVcp = currentChatHistory.value
        .filter(m => !m.isThinking)
        .map(m => ({
          role: m.role,
          content: m.content,
          name: m.name
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

      console.log('[ChatManager] Sending single payload to VCP:', payload);
      
      // 记录模型使用频率
      if (payload.modelConfig.model) {
        modelStore.recordUsage(payload.modelConfig.model);
      }

      // 直接发起请求，移除 30s 前端硬超时
      await invoke('sendToVCP', { payload });
    } catch (e) {
      console.error('[ChatManager] Failed to send message:', e);
      // 发生错误时移除思考态消息
      currentChatHistory.value = currentChatHistory.value.filter(m => m.id !== thinkingId);
      
      currentChatHistory.value.push({
        id: `error_${Date.now()}`,
        role: 'system',
        content: `VCP错误: ${e instanceof Error ? e.message : String(e)}`,
        timestamp: Date.now()
      });
      
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

  onMounted(async () => {
    // 监听 AI 流式输出事件
    unlistenStreamPromise = listen('vcp-stream', (event: any) => {
      // 适配 Rust 端默认序列化使用下划线命名法 (message_id)
      const { message_id, messageId: legacyMessageId, chunk, type } = event.payload;
      const actualMessageId = message_id || legacyMessageId;
      
      if (actualMessageId === streamingMessageId.value) {
        const msg = currentChatHistory.value.find(m => m.id === actualMessageId);
        if (msg) {
          if (type === 'data') {
            msg.isThinking = false;

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
              msg.content += textChunk;
              // 使用 streamManager 平滑更新 displayedContent
              // 注意：callback 内部必须重新根据 ID 查找最新对象，防止 reactivity orphan
              streamManager.appendChunk(actualMessageId, textChunk, (text) => {
                const latestMsg = currentChatHistory.value.find(m => m.id === actualMessageId);
                if (latestMsg) {
                  latestMsg.displayedContent = text;
                }
              });
            }
            } else if (type === 'end') {
            console.log(`[ChatManager] Stream ended for ${actualMessageId}. Draining queue...`);
            msg.isThinking = false;
            // 流式结束时，等待 streamManager 缓冲队列排空后再切换状态
            streamManager.finalizeStream(actualMessageId, () => {
              const latestMsg = currentChatHistory.value.find(m => m.id === actualMessageId);
              if (latestMsg) {
                // 确保最终内容一致
                latestMsg.displayedContent = latestMsg.content;
              }
              streamingMessageId.value = null;

              // 重新获取一次最新引用进行正则处理
              const finalMsg = currentChatHistory.value.find(m => m.id === actualMessageId);
              if (finalMsg && currentSelectedItem.value?.id) {
                processRegex(finalMsg, currentSelectedItem.value.id);
              }
              saveHistory();
              // 话题自动总结逻辑 (桌面端对齐)
              summarizeTopic();
            });
            }
 else if (type === 'error') {
            console.error(`[ChatManager] Stream error for ${actualMessageId}:`, event.payload.error);
            msg.isThinking = false;
            streamManager.finalizeStream(actualMessageId);
            streamingMessageId.value = null;
            currentChatHistory.value.push({
              id: `error_${Date.now()}`,
              role: 'system',
              content: `VCP流式错误: ${event.payload.error || '未知错误'}`,
              timestamp: Date.now()
            });
            saveHistory();
          }
        }
      }
    });

    // 监听外部文件变更 (对应桌面端的 history-file-updated)
    unlistenFileChangePromise = listen('vcp-file-change', async (event: any) => {
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

    unlistenGroupFinishedPromise = listen('vcp-group-turn-finished', (event: any) => {
      console.log('[ChatManager] Group turn finished:', event.payload);
      isGroupGenerating.value = false;
    });
  });

  onUnmounted(() => {
    if (unlistenStreamPromise) unlistenStreamPromise.then(unlisten => unlisten());
    if (unlistenFileChangePromise) unlistenFileChangePromise.then(unlisten => unlisten());
    if (unlistenGroupFinishedPromise) unlistenGroupFinishedPromise.then(unlisten => unlisten());
  });

  return {
    currentChatHistory,
    currentSelectedItem,
    currentTopicId,
    loading,
    coreStatus,
    coreErrorMsg,
    streamingMessageId,
    stagedAttachments,
    editMessageContent,
    loadHistory,
    saveHistory,
    syncHistoryWithDelta,
    sendMessage,
    handleAttachment,
    deleteMessage,
    stopGenerating,
    updateMessageContent,
    regenerateResponse,
    isGroupGenerating
  };
});
