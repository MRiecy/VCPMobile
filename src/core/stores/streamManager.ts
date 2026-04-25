// streamManager.ts
import { defineStore } from 'pinia';
import { ref } from 'vue';

export const useStreamManagerStore = defineStore('streamManager', () => {
  const streamBuffers = new Map<string, {
    fullText: string;
    stableContent: string; // 已确定的稳定内容 (HTML)
    tailContent: string;   // 正在生成的尾部内容 (Markdown/Text)
    semanticQueue: string[];
    lastUpdateTime: number;
    isFinishing: boolean;
    onCompleteCallback?: () => void;
  }>();

  const activeStreams = ref(new Set<string>());

  /**
   * 简单的 HTML 标签补全逻辑，防止流式输出截断导致 morphdom 崩溃
   */
  const balanceHtmlTags = (html: string) => {
    const tags = ['div', 'pre', 'code', 'p', 'span', 'blockquote'];
    let balanced = html;
    for (const tag of tags) {
      const openCount = (html.match(new RegExp(`<${tag}[^>]*>`, 'g')) || []).length;
      const closeCount = (html.match(new RegExp(`</${tag}>`, 'g')) || []).length;
      if (openCount > closeCount) {
        balanced += `</${tag}>`.repeat(openCount - closeCount);
      }
    }
    return balanced;
  };

  const appendChunk = (messageId: string, chunk: string, onUpdate: (data: { stable: string, tail: string }) => void) => {
    if (!streamBuffers.has(messageId)) {
      streamBuffers.set(messageId, {
        fullText: chunk,
        stableContent: '',
        tailContent: '',
        semanticQueue: [...chunk],
        lastUpdateTime: performance.now(),
        isFinishing: false
      });
      activeStreams.value.add(messageId);

      const loop = () => {
        const buf = streamBuffers.get(messageId);
        if (!buf) {
          activeStreams.value.delete(messageId);
          return;
        }

        if (buf.semanticQueue.length > 0) {
          const backlog = buf.semanticQueue.length;
          const step = Math.max(1, Math.ceil(backlog / 8));

          for (let i = 0; i < step; i++) {
            const char = buf.semanticQueue.shift();
            if (char) {
              buf.tailContent += char;
            }
          }

          // [VCP-Aurora 核心算法] 增量沉淀逻辑
          // 查找最后一个双换行符作为沉淀锚点
          const lastBreak = buf.tailContent.lastIndexOf('\n\n');
          if (lastBreak !== -1 && !buf.isFinishing) {
            const potentialStable = buf.tailContent.substring(0, lastBreak + 2);
            
            // 严谨检测是否处于非稳定块内部 (使用计数法增强健壮性)
            const countOpen = (str: string, pat: string) => (str.split(pat).length - 1);
            
            const isInCode = countOpen(potentialStable, '```') % 2 !== 0;
            const isInThink = countOpen(potentialStable, '<think') > countOpen(potentialStable, '</think');
            const isInVcpThink = countOpen(potentialStable, '[--- VCP元思考链') > countOpen(potentialStable, '[--- 元思考链结束 ---]');
            const isInTool = countOpen(potentialStable, '<<<[TOOL_REQUEST]>>>') > countOpen(potentialStable, '<<<[END_TOOL_REQUEST]>>>');

            if (!isInCode && !isInThink && !isInVcpThink && !isInTool) {
              // 确认为稳定内容，进行沉淀
              buf.stableContent += potentialStable;
              buf.tailContent = buf.tailContent.substring(lastBreak + 2);
              console.log('[Aurora] Sedimentation triggered, stable length:', buf.stableContent.length);
            }
          }

          onUpdate({ 
            stable: buf.stableContent, 
            tail: balanceHtmlTags(buf.tailContent) 
          });
        }

        if (buf.isFinishing && buf.semanticQueue.length === 0) {
          // 流结束时，将所有剩余 tailContent 彻底沉淀到 stableContent
          if (buf.tailContent) {
            buf.stableContent += buf.tailContent;
            buf.tailContent = '';
            onUpdate({ 
            stable: buf.stableContent, 
            tail: balanceHtmlTags(buf.tailContent) 
          });
          }
          
          if (buf.onCompleteCallback) buf.onCompleteCallback();
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
      if (buffer.isFinishing) buffer.isFinishing = false;
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
