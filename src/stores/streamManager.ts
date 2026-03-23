// streamManager.ts
import { defineStore } from 'pinia';
import { ref } from 'vue';

export const useStreamManagerStore = defineStore('streamManager', () => {
  const streamBuffers = new Map<string, {
    fullText: string;
    displayedText: string;
    semanticQueue: string[]; // 新增：语义化分块队列
    lastUpdateTime: number;
    isFinishing: boolean;
  }>();

  const activeStreams = ref(new Set<string>());

  /**
   * 语义化分块：按词组、短语或中文字符拆分，而不是生硬的字节数
   */
  const intelligentChunkSplit = (text: string): string[] => {
    if (!text) return [];
    // 匹配：中文字符、连续字母数字、标点符号、空白符
    const regex = /[\u4e00-\u9fa5]|[a-zA-Z0-9]+|[^\u4e00-\u9fa5a-zA-Z0-9\s]+|\s+/g;
    return text.match(regex) || [text];
  };

  const appendChunk = (messageId: string, chunk: string, onUpdate: (text: string) => void) => {
    if (!streamBuffers.has(messageId)) {
      const semanticUnits = intelligentChunkSplit(chunk);
      streamBuffers.set(messageId, {
        fullText: chunk,
        displayedText: '',
        semanticQueue: semanticUnits,
        lastUpdateTime: performance.now(),
        isFinishing: false
      });
      activeStreams.value.add(messageId);
      
      // 核心渲染循环
      const loop = () => {
        const buf = streamBuffers.get(messageId);
        if (!buf) {
          activeStreams.value.delete(messageId);
          return;
        }
        
        // 检查队列中是否有待显示的语义块
        if (buf.semanticQueue.length > 0) {
          // 计算步长：如果积压严重，一帧多出几个词；正常情况下每帧 1-2 个词
          const backlog = buf.semanticQueue.length;
          const step = buf.isFinishing ? backlog : Math.max(1, Math.ceil(backlog / 6));
          
          for (let i = 0; i < step; i++) {
            const unit = buf.semanticQueue.shift();
            if (unit) {
              buf.displayedText += unit;
            }
          }

          try {
            onUpdate(buf.displayedText);
          } catch(e) {
            console.error('[StreamManager] UI Update failed:', e);
          }
        }
        
        // 结束判定：没有积压且后端已发送 [DONE]
        if (buf.isFinishing && buf.semanticQueue.length === 0) {
          activeStreams.value.delete(messageId);
          streamBuffers.delete(messageId);
        } else {
          requestAnimationFrame(loop);
        }
      };
      requestAnimationFrame(loop);
    } else {
      const buffer = streamBuffers.get(messageId)!;
      buffer.fullText += chunk;
      
      // 如果之前是因为 [DONE] 标记为结束但又有新 chunk 进来，重置状态
      if (buffer.isFinishing) buffer.isFinishing = false;
      
      // 实时追加到语义队列
      const newUnits = intelligentChunkSplit(chunk);
      buffer.semanticQueue.push(...newUnits);
    }
  };

  const finalizeStream = (messageId: string, onUpdate?: (text: string) => void) => {
    const buffer = streamBuffers.get(messageId);
    if (buffer) {
      // 标记为结束，loop 会在清空队列后自动退出
      buffer.isFinishing = true;
      
      // 如果需要立即刷新（例如组件卸载或强制跳转）
      if (onUpdate) {
        // 合并所有积压的队列内容
        buffer.displayedText += buffer.semanticQueue.join('');
        buffer.semanticQueue = [];
        onUpdate(buffer.displayedText);
        activeStreams.value.delete(messageId);
        streamBuffers.delete(messageId);
      }
    }
  };

  return {
    activeStreams,
    appendChunk,
    finalizeStream
  };
});
