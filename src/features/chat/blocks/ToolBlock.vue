<script setup lang="ts">
import { ref, watch, onUnmounted } from 'vue';
import { ChevronDown, ChevronUp, Settings, Maximize2, Copy, X } from 'lucide-vue-next';
import type { ContentBlock } from '../../../core/types/chat';
import { marked } from 'marked';
import { useNotificationStore } from '../../../core/stores/notification';
import { useModalHistory } from '../../../core/composables/useModalHistory';
import { filterTrustedRichHtml, withCodeBlockClass } from '../../../core/utils/astRenderer';

const notificationStore = useNotificationStore();

// Configure marked to support Github Flavored Markdown & breaks
marked.setOptions({
  gfm: true,
  breaks: true,
});

const parsedMarkdownCache = new Map<string, string>();
const MAX_MARKDOWN_CACHE_SIZE = 50;

const renderMarkdown = (text: string): string => {
  if (!text) return '';
  if (parsedMarkdownCache.has(text)) {
    return parsedMarkdownCache.get(text)!;
  }
  try {
    const rendered = filterTrustedRichHtml(withCodeBlockClass(marked.parse(text) as string));
    if (parsedMarkdownCache.size >= MAX_MARKDOWN_CACHE_SIZE) {
      const firstKey = parsedMarkdownCache.keys().next().value;
      if (firstKey !== undefined) {
        parsedMarkdownCache.delete(firstKey);
      }
    }
    parsedMarkdownCache.set(text, rendered);
    return rendered;
  } catch (e) {
    console.error('[ToolBlock] marked parse failed:', e);
    return text;
  }
};

const props = withDefaults(
  defineProps<{
    type: 'tool-use' | 'tool-result';
    content?: string;
    block: ContentBlock;
    defaultExpanded?: boolean;
  }>(),
  {
    defaultExpanded: false,
  }
);

const isExpanded = ref(props.defaultExpanded);

watch(
  () => props.defaultExpanded,
  (newVal) => {
    isExpanded.value = newVal;
  }
);

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
onUnmounted(() => {
  unregisterModal(modalId);
  parsedMarkdownCache.clear();
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
  <div class="vcp-tool-block my-2 rounded-xl transition-all duration-300 overflow-hidden" :class="[
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
