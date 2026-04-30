<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue';
import { ChevronDown, ChevronUp, Settings, Loader2 } from 'lucide-vue-next';
import MarkdownBlock from './MarkdownBlock.vue';
import type { ContentBlock } from '../../../core/composables/useContentProcessor';

const props = defineProps<{
  type: 'tool-use' | 'tool-result';
  content?: string;
  block: ContentBlock;
}>();

const isExpanded = ref(props.type === 'tool-result' ? false : true);
const toolBlockRef = ref<HTMLElement | null>(null);
let observer: IntersectionObserver | null = null;

onMounted(() => {
  if (!toolBlockRef.value) return;
  observer = new IntersectionObserver((entries) => {
    entries.forEach((entry) => {
      if (entry.isIntersecting) {
        toolBlockRef.value?.classList.remove('vcp-animation-paused');
      } else {
        toolBlockRef.value?.classList.add('vcp-animation-paused');
      }
    });
  }, { threshold: 0 });
  observer.observe(toolBlockRef.value);
});

onUnmounted(() => {
  observer?.disconnect();
});

const toggleExpand = () => {
  isExpanded.value = !isExpanded.value;
};

// 检测工具结果值是否为图片（HTTP URL 或 base64 data URI）
const isImageValue = (key: string, value: string): boolean => {
  const imageKeys = ['可访问URL', '返回内容', 'url', 'image'];
  if (!imageKeys.includes(key)) return false;
  const isHttpImage = /^https?:\/\/[^\s]+$/i.test(value) && /\.(jpeg|jpg|png|gif|webp)([?&#]|$)/i.test(value);
  const isBase64Image = /^data:image\/(png|jpeg|jpg|gif|webp);base64,/i.test(value);
  return isHttpImage || isBase64Image;
};
</script>

<template>
  <div ref="toolBlockRef" class="vcp-tool-block my-2 rounded-xl transition-all duration-300 overflow-hidden" :class="[
    type === 'tool-use' ? 'is-tool-use' : 'is-tool-result',
    isExpanded ? 'shadow-md' : 'shadow-sm'
  ]">
    <!-- Header -->
    <div class="tool-header-content flex items-center justify-between p-3 cursor-pointer select-none"
      @click="toggleExpand">
      <div class="flex items-center gap-2">
        <div class="tool-icon-container p-1.5 rounded-lg">
          <Settings v-if="type === 'tool-use'" :size="14" />
          <span v-else class="text-lg leading-none">📊</span>
        </div>
        <div>
          <span class="tool-label text-[10px] font-bold block leading-none mb-1 flex items-center gap-1">
            {{ type === 'tool-use' ? 'VCP-ToolUse' : 'VCP-ToolResult' }}
            <Loader2 v-if="type === 'tool-use' && !block.is_complete" :size="10" class="animate-spin" />
          </span>
          <span class="tool-name text-xs font-bold font-mono">
            {{ block.tool_name || 'Unknown Tool' }}
          </span>
        </div>
      </div>

      <div class="flex items-center gap-2">
        <span v-if="block.status" class="tool-status text-[10px] px-1.5 py-0.5 rounded font-bold">
          {{ block.status }}
        </span>
        <component :is="isExpanded ? ChevronUp : ChevronDown" :size="16" class="opacity-50" />
      </div>
    </div>

    <!-- Content -->
    <div v-show="isExpanded"
      class="tool-header-content border-t border-black/10 dark:border-white/10 p-3 animate-slide-down tool-content-scrollable vcp-scrollable">
      <template v-if="type === 'tool-use'">
        <pre class="text-[11px] font-mono whitespace-pre-wrap break-words">{{ content }}</pre>
      </template>
      <template v-else>
        <div class="space-y-2">
          <div v-for="item in block.details" :key="item.key" class="text-xs flex flex-col sm:flex-row sm:items-start">
            <span class="detail-key font-bold mr-2 whitespace-nowrap mt-0.5">{{ item.key }}:</span>
            <div class="mt-1 sm:mt-0 flex-1 min-w-0">
              <!-- 图片值直接渲染为 img，其他值走 Markdown 管线 -->
              <template v-if="item.value && isImageValue(item.key, item.value)">
                <a :href="item.value" target="_blank" rel="noopener noreferrer" class="block">
                  <img :src="item.value" class="max-w-full rounded-lg" loading="lazy" alt="Generated Image" />
                </a>
              </template>
              <MarkdownBlock v-else :content="item.value || ''" class="compact-markdown" />
            </div>
          </div>
          <div v-if="block.footer" class="mt-2 pt-2 border-t border-black/10 dark:border-white/10 text-xs opacity-70">
            <MarkdownBlock :content="block.footer" class="compact-markdown" />
          </div>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
/* --- Animations --- */
@keyframes vcp-bubble-background-flow-kf {
  0% {
    background-position: 0% 50%;
  }

  50% {
    background-position: 100% 50%;
  }

  100% {
    background-position: 0% 50%;
  }
}

@keyframes vcp-bubble-border-flow-kf {
  0% {
    background-position: 0% 50%;
  }

  50% {
    background-position: 200% 50%;
  }

  100% {
    background-position: 0% 50%;
  }
}

@keyframes vcp-icon-rotate {
  0% {
    transform: rotate(0deg);
  }

  100% {
    transform: rotate(360deg);
  }
}

.animate-slide-down {
  animation: slideDown 0.3s cubic-bezier(0.4, 0, 0.2, 1);
}

@keyframes slideDown {
  from {
    opacity: 0;
    transform: translateY(-10px);
  }

  to {
    opacity: 1;
    transform: translateY(0);
  }
}

/* 离屏时暂停无限动画以节省 GPU */
.vcp-tool-block.vcp-animation-paused.is-tool-use,
.vcp-tool-block.vcp-animation-paused.is-tool-use::after,
.vcp-tool-block.vcp-animation-paused.is-tool-use .tool-icon-container {
  animation-play-state: paused !important;
}

/* --- Tool Use Bubble --- */
.vcp-tool-block.is-tool-use {
  background: linear-gradient(145deg, #3a7bd5 0%, #00d2ff 100%) !important;
  background-size: 200% 200% !important;
  animation: vcp-bubble-background-flow-kf 20s ease-in-out infinite;
  color: #ffffff !important;
  border: none !important;
  position: relative;
}

.vcp-tool-block.is-tool-use::after {
  content: "";
  position: absolute;
  box-sizing: border-box;
  top: 0;
  left: 0;
  width: 100%;
  height: 100%;
  border-radius: inherit;
  padding: 2px;
  background: linear-gradient(60deg, #76c4f7, #00d2ff, #3a7bd5, #ffffff, #3a7bd5, #00d2ff, #76c4f7);
  background-size: 300% 300%;
  animation: vcp-bubble-border-flow-kf 7s linear infinite;
  -webkit-mask: linear-gradient(#fff 0 0) content-box, linear-gradient(#fff 0 0);
  mask: linear-gradient(#fff 0 0) content-box, linear-gradient(#fff 0 0);
  -webkit-mask-composite: xor;
  mask-composite: exclude;
  z-index: 0;
  pointer-events: none;
}

.vcp-tool-block.is-tool-use .tool-header-content {
  position: relative;
  z-index: 1;
}

.vcp-tool-block.is-tool-use .tool-icon-container {
  background: transparent !important;
  color: rgba(255, 255, 255, 0.9) !important;
  animation: vcp-icon-rotate 4s linear infinite;
}

.vcp-tool-block.is-tool-use .tool-label {
  color: #f1c40f !important;
}

.vcp-tool-block.is-tool-use .tool-name {
  color: #ffffff !important;
}

.vcp-tool-block.is-tool-use pre {
  background-color: rgba(0, 0, 0, 0.2);
  color: #f0f0f0;
  border-radius: 6px;
  padding: 10px;
}

/* --- Tool Result Bubble (Light/Dark Theme Fixed) --- */

/* 修复：将亮色模式设为默认基础样式 */
.vcp-tool-block.is-tool-result {
  background: linear-gradient(145deg, #f4f6f8, #e8eaf0);
  border: 1px solid rgba(0, 0, 0, 0.1);
  color: #333;
}

.vcp-tool-block.is-tool-result .tool-label {
  color: #2e7d32 !important;
  /* 深一点的绿色适应亮色背景 */
}

.vcp-tool-block.is-tool-result .tool-name {
  color: #0277bd;
  background-color: rgba(2, 119, 189, 0.1);
  padding: 2px 6px;
  border-radius: 4px;
}

.vcp-tool-block.is-tool-result .tool-status {
  color: #1b5e20;
  background-color: rgba(76, 175, 80, 0.15);
}

.vcp-tool-block.is-tool-result .detail-key {
  color: #546e7a;
}

/* 修复：适配 Vue/Tailwind 标准的暗黑模式选择器 */
html.dark .vcp-tool-block.is-tool-result {
  background: linear-gradient(145deg, #1c1c1e, #2c2c2e);
  border: 1px solid rgba(255, 255, 255, 0.08);
  color: #f2f2f7;
}

html.dark .vcp-tool-block.is-tool-result .tool-label {
  color: #4caf50 !important;
}

html.dark .vcp-tool-block.is-tool-result .tool-name {
  color: #64d2ff;
  background-color: rgba(100, 210, 255, 0.15);
}

html.dark .vcp-tool-block.is-tool-result .tool-status {
  color: #c8e6c9;
  background-color: rgba(76, 175, 80, 0.2);
}

html.dark .vcp-tool-block.is-tool-result .detail-key {
  color: #8e8e93;
}

/* 修复：工具内子级 Markdown 的压缩排版，去除无意义的段落边距，恢复正常换行 */
:deep(.compact-markdown p) {
  margin-top: 0 !important;
  margin-bottom: 4px !important;
}

.tool-content-scrollable {
  max-height: 400px;
  overflow-y: auto;
}
</style>
