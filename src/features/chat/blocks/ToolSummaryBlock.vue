<script setup lang="ts">
import type { ContentBlock } from "../../../core/types/chat";

defineProps<{
  block: ContentBlock;
}>();

const statusLabelMap: Record<string, string> = {
  success: '成功',
  failure: '失败',
  timeout: '超时',
  rejected: '拒绝',
  cancelled: '取消',
  skipped: '跳过',
  unknown: '未知'
};

function getStatusLabel(status: string): string {
  return statusLabelMap[status] || '未知';
}
</script>

<template>
  <div class="vcp-tool-call-summary-bubble" data-vcp-block-type="tool-call-summary" data-vcp-preserve-children="true">
    <div class="vcp-tool-call-summary-header">
      <span class="vcp-tool-call-summary-icon">🧾</span>
      <span class="vcp-tool-call-summary-title">本轮工具调用摘要</span>
    </div>
    
    <div v-if="block.items && block.items.length > 0" class="vcp-tool-call-summary-list">
      <span 
        v-for="(item, index) in block.items" 
        :key="index" 
        class="vcp-tool-call-summary-chip" 
        :class="`status-${item.status}`"
      >
        <span class="vcp-tool-call-summary-tool">{{ item.tool_name }}</span>
        <span class="vcp-tool-call-summary-status">{{ getStatusLabel(item.status) }}</span>
      </span>
    </div>
    <div v-else class="vcp-tool-call-summary-raw">
      {{ block.raw_content || '无摘要内容' }}
    </div>
  </div>
</template>
