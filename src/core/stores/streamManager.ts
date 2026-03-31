// streamManager.ts
import { defineStore } from 'pinia';
import { ref } from 'vue';

export const useStreamManagerStore = defineStore('streamManager', () => {
  const streamBuffers = new Map<string, {
    fullText: string;
    displayedText: string;
    semanticQueue: string[]; // 现已简化为纯字符队列
    lastUpdateTime: number;
    isFinishing: boolean;
    onCompleteCallback?: () => void;
  }>();

  const activeStreams = ref(new Set<string>());

  const appendChunk = (messageId: string, chunk: string, onUpdate: (text: string) => void) => {
    if (!streamBuffers.has(messageId)) {
      const chars = [...chunk];
      streamBuffers.set(messageId, {
        fullText: chunk,
        displayedText: '',
        semanticQueue: chars,
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
        
        // 检查队列中是否有待显示的字符
        if (buf.semanticQueue.length > 0) {
          // 计算步长：如果积压严重，一帧多出几个字符；正常情况下每帧 1-2 个
          const backlog = buf.semanticQueue.length;
          // [保留神级底盘] 平滑排空
          const step = Math.max(1, Math.ceil(backlog / 8));
          
          for (let i = 0; i < step; i++) {
            const char = buf.semanticQueue.shift();
            if (char) {
              buf.displayedText += char;
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
          if (buf.onCompleteCallback) {
            buf.onCompleteCallback();
          }
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
      
      // [修复建议] 绝对禁止使用 push(...chunk)，防止栈溢出崩溃
      for (const char of chunk) {
        buffer.semanticQueue.push(char);
      }
    }
  };

  const finalizeStream = (messageId: string, onComplete?: () => void) => {
    const buffer = streamBuffers.get(messageId);
    if (buffer) {
      // 如果已经标记为结束，不要重复设置回调，但可以更新它
      if (buffer.isFinishing) {
        const oldCallback = buffer.onCompleteCallback;
        buffer.onCompleteCallback = () => {
          if (oldCallback) oldCallback();
          if (onComplete) onComplete();
        };
        return;
      }
      
      // 标记为结束，loop 会在清空队列后触发回调并自动退出
      buffer.isFinishing = true;
      buffer.onCompleteCallback = onComplete;
    } else {
      // 如果 buffer 已经不存在（比如未经过 stream 过程就结束了），直接触发回调
      activeStreams.value.delete(messageId);
      if (onComplete) onComplete();
    }
  };

  return {
    activeStreams,
    appendChunk,
    finalizeStream
  };
});
