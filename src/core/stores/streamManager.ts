import { defineStore } from 'pinia';
import { ref } from 'vue';

/**
 * StreamManager — 流状态管理器（简化版）
 *
 * Aurora 语义沉淀逻辑已移入 Rust 流式管道。
 * 前端 streamManager 仅负责：
 * 1. 跟踪活跃流消息 ID（供 UI 判断流式状态）
 * 2. 提供 finalizeStream 入口（流结束时执行清理回调）
 */
export const useStreamManagerStore = defineStore('streamManager', () => {
  const activeStreams = ref(new Set<string>());

  /**
   * 标记流式消息为完成状态，并执行可选的完成回调
   */
  const finalizeStream = (messageId: string, onComplete?: () => void) => {
    activeStreams.value.delete(messageId);
    if (onComplete) {
      onComplete();
    }
  };

  return {
    activeStreams,
    finalizeStream
  };
});
