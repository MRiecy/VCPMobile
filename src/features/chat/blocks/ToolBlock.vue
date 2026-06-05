<script setup lang="ts">
import { ref, watch, onMounted, onUnmounted } from 'vue';
import { ChevronDown, ChevronUp, Settings, Loader2, Maximize2, Copy, X } from 'lucide-vue-next';
import type { ContentBlock } from '../../../core/types/chat';
import { marked } from 'marked';
import { useNotificationStore } from '../../../core/stores/notification';
import { useModalHistory } from '../../../core/composables/useModalHistory';

const notificationStore = useNotificationStore();

// Configure marked to support Github Flavored Markdown & breaks
marked.setOptions({
  gfm: true,
  breaks: true,
});

const renderMarkdown = (text: string): string => {
  if (!text) return '';
  try {
    return marked.parse(text) as string;
  } catch (e) {
    console.error('[ToolBlock] marked parse failed:', e);
    return text;
  }
};

const props = defineProps<{
  type: 'tool-use' | 'tool-result';
  content?: string;
  block: ContentBlock;
}>();

const isExpanded = ref(false);
const isFullScreen = ref(false);

const { registerModal, unregisterModal } = useModalHistory();
const modalId = `ToolBlockFullScreen_${Math.random().toString(36).substring(2, 9)}`;

watch(isFullScreen, (newVal) => {
  if (newVal) {
    registerModal(modalId, () => {
      isFullScreen.value = false;
    });
  } else {
    unregisterModal(modalId);
  }
});

const copyDetailValue = (key: string, value: string) => {
  if (!value) return;
  navigator.clipboard.writeText(value)
    .then(() => {
      notificationStore.addNotification({
        type: "success",
        title: "复制成功",
        message: `${key} 已复制到剪贴板`,
        toastOnly: true
      });
    })
    .catch((err) => {
      console.error('[ToolBlock] Copy failed:', err);
    });
};

const copyAllDetails = () => {
  const detailsText = props.block.details
    ?.map((item: any) => `${item.key}: ${item.value}`)
    .join('\n') || '';
  const footerText = props.block.footer ? `\n\n${props.block.footer}` : '';
  const fullText = `${detailsText}${footerText}`;
  if (!fullText) return;
  navigator.clipboard.writeText(fullText)
    .then(() => {
      notificationStore.addNotification({
        type: "success",
        title: "复制全部成功",
        message: "所有工具返回结果已复制到剪贴板",
        toastOnly: true
      });
    })
    .catch((err) => {
      console.error('[ToolBlock] Copy all failed:', err);
    });
};
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
  unregisterModal(modalId);
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
    type === 'tool-use' ? 'is-tool-use' : 'is-tool-result tool-bubble',
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
            <Loader2 v-if="type === 'tool-use' && !block.is_complete" :size="10" class="custom-spin" />
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
        <button 
          v-if="isExpanded && type === 'tool-result'"
          @click.stop="isFullScreen = true"
          class="p-1 rounded-lg hover:bg-black/5 dark:hover:bg-white/5 active:scale-90 transition-transform opacity-60 hover:opacity-100 flex items-center justify-center mr-0.5"
        >
          <Maximize2 :size="13" />
        </button>
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
        <div class="space-y-3">
          <div v-for="(item, index) in block.details" :key="item.key" 
            class="text-xs flex flex-col"
            :class="[index > 0 ? 'border-t border-black/5 dark:border-white/5 pt-3 mt-3' : '']"
          >
            <div class="flex items-center justify-between mb-1.5">
              <span class="detail-key font-bold font-mono text-[11px] opacity-80">{{ item.key }}:</span>
              <button 
                v-if="item.value"
                @click="copyDetailValue(item.key, item.value)"
                class="p-1 rounded hover:bg-black/5 dark:hover:bg-white/5 active:scale-95 transition-all opacity-60 hover:opacity-100 flex items-center justify-center gap-1 text-[10px]"
              >
                <Copy :size="10" />
                <span>复制</span>
              </button>
            </div>
            <div class="min-w-0">
              <!-- 图片值直接渲染为 img，其他值走 Markdown 管线 -->
              <template v-if="item.value && isImageValue(item.key, item.value)">
                <a :href="item.value" target="_blank" rel="noopener noreferrer" class="block">
                  <img :src="item.value" class="max-w-full rounded-lg" loading="lazy" alt="Generated Image" />
                </a>
              </template>
              <div v-else class="text-xs opacity-90 vcp-markdown-block compact-markdown select-text" v-html="renderMarkdown(item.value || '')"></div>
            </div>
          </div>
          <div v-if="block.footer" class="mt-2.5 pt-2.5 border-t border-black/10 dark:border-white/10 text-xs opacity-70 vcp-markdown-block compact-markdown select-text" v-html="renderMarkdown(block.footer)"></div>
        </div>
      </template>
    </div>
  </div>

  <!-- Fullscreen Viewer Drawer -->
  <Teleport to="body">
    <Transition name="fade-slide">
      <div v-if="isFullScreen" 
        class="vcp-fullscreen-tool-panel fixed inset-0 z-viewer flex flex-col"
        :class="[type === 'tool-result' ? 'is-tool-result' : '']"
      >
        <!-- Fullscreen Header -->
        <div class="vcp-fullscreen-header flex items-center justify-between pb-3 mb-4 border-b">
          <div class="flex items-center gap-2">
            <span class="text-lg leading-none">📊</span>
            <div>
              <span class="text-[10px] font-bold block opacity-60 leading-none mb-1">
                {{ block.tool_name || 'Unknown Tool' }} - 详细结果
              </span>
              <span class="text-xs font-mono font-bold">全屏浏览</span>
            </div>
          </div>
          <div class="flex items-center gap-2">
            <button 
              @click="copyAllDetails"
              class="px-2.5 py-1.5 text-xs rounded-lg bg-black/5 dark:bg-white/5 hover:bg-black/10 dark:hover:bg-white/10 active:scale-95 transition-all opacity-80 hover:opacity-100 flex items-center gap-1.5 font-bold"
            >
              <Copy :size="12" />
              <span>复制全部</span>
            </button>
            <button 
              @click="isFullScreen = false"
              class="p-1.5 rounded-lg hover:bg-black/5 dark:hover:bg-white/5 active:scale-90 transition-transform opacity-70 hover:opacity-100 flex items-center justify-center"
            >
              <X :size="16" />
            </button>
          </div>
        </div>

        <!-- Fullscreen Scrollable Content -->
        <div class="flex-1 overflow-y-auto vcp-scrollable space-y-4 pr-1">
          <div v-for="(item, index) in block.details" :key="item.key" 
            class="flex flex-col"
            :class="[index > 0 ? 'border-t border-black/5 dark:border-white/5 pt-4 mt-4' : '']"
          >
            <div class="flex items-center justify-between mb-2">
              <span class="detail-key font-bold font-mono text-xs opacity-90">{{ item.key }}:</span>
              <button 
                v-if="item.value"
                @click="copyDetailValue(item.key, item.value)"
                class="px-2 py-1 rounded hover:bg-black/5 dark:hover:bg-white/5 active:scale-95 transition-all opacity-70 hover:opacity-100 flex items-center justify-center gap-1 text-[11px]"
              >
                <Copy :size="11" />
                <span>复制</span>
              </button>
            </div>
            <div class="min-w-0">
              <template v-if="item.value && isImageValue(item.key, item.value)">
                <a :href="item.value" target="_blank" rel="noopener noreferrer" class="block">
                  <img :src="item.value" class="max-w-full rounded-lg" loading="lazy" alt="Generated Image" />
                </a>
              </template>
              <div v-else class="text-sm opacity-95 vcp-markdown-block select-text" v-html="renderMarkdown(item.value || '')"></div>
            </div>
          </div>
          
          <div v-if="block.footer" class="mt-4 pt-4 border-t border-black/10 dark:border-white/10 text-xs opacity-70 vcp-markdown-block select-text" v-html="renderMarkdown(block.footer)"></div>
        </div>
      </div>
    </Transition>
  </Teleport>
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
    transform: rotate(0deg) translate3d(0, 0, 0);
  }

  100% {
    transform: rotate(360deg) translate3d(0, 0, 0);
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
  /* GPU 硬件加速与合成层隔离 */
  will-change: transform, opacity;
  transform: translate3d(0, 0, 0);
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
  /* 隔离复杂的遮罩平移重绘污染 */
  will-change: transform, opacity;
  transform: translate3d(0, 0, 0);
}

.vcp-tool-block.is-tool-use .tool-header-content {
  position: relative;
  z-index: 1;
}

.vcp-tool-block.is-tool-use .tool-icon-container {
  background: transparent !important;
  color: rgba(255, 255, 255, 0.9) !important;
  animation: vcp-icon-rotate 4s linear infinite;
  /* 图标高频旋转开启硬件加速 */
  will-change: transform;
  transform: translate3d(0, 0, 0);
}

.custom-spin {
  animation: vcp-spin 1s linear infinite;
  /* 提升至 GPU 合成层 */
  will-change: transform;
  transform: translate3d(0, 0, 0);
}

@keyframes vcp-spin {
  from {
    transform: rotate(0deg) translate3d(0, 0, 0);
  }

  to {
    transform: rotate(360deg) translate3d(0, 0, 0);
  }
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
  background: linear-gradient(145deg, #f9f9fb, #f2f2f7);
  border: 1px solid rgba(0, 0, 0, 0.06);
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

/* --- Fullscreen Panel Styles (No glassmorphism, matching is-tool-result) --- */
.vcp-fullscreen-tool-panel {
  padding-top: calc(1rem + env(safe-area-inset-top, 0px));
  padding-bottom: calc(1rem + env(safe-area-inset-bottom, 0px));
  padding-left: calc(1rem + env(safe-area-inset-left, 0px));
  padding-right: calc(1rem + env(safe-area-inset-right, 0px));
}

.vcp-fullscreen-tool-panel.is-tool-result {
  background: linear-gradient(145deg, #f9f9fb, #f2f2f7);
  color: #333;
}

.vcp-fullscreen-tool-panel.is-tool-result .vcp-fullscreen-header {
  border-color: rgba(0, 0, 0, 0.06);
}

html.dark .vcp-fullscreen-tool-panel.is-tool-result {
  background: linear-gradient(145deg, #1c1c1e, #2c2c2e);
  color: #f2f2f7;
}

html.dark .vcp-fullscreen-tool-panel.is-tool-result .vcp-fullscreen-header {
  border-color: rgba(255, 255, 255, 0.08);
}

/* --- Transitions --- */
.fade-slide-enter-active,
.fade-slide-leave-active {
  transition: opacity 0.25s cubic-bezier(0.4, 0, 0.2, 1), transform 0.25s cubic-bezier(0.4, 0, 0.2, 1);
}

.fade-slide-enter-from,
.fade-slide-leave-to {
  opacity: 0;
  transform: translateY(20px);
}
</style>
