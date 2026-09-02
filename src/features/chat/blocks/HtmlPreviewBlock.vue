<script setup lang="ts">
import { ref, watch, computed, onUnmounted } from 'vue';
import { useThemeStore } from '../../../core/stores/theme';
import { useModalHistory } from '../../../core/composables/useModalHistory';

const props = withDefaults(defineProps<{
  content: string;
  messageId: string;
  highlightedContent?: string;
  isStreaming?: boolean;
}>(), {
  isStreaming: false,
});

const themeStore = useThemeStore();
const isPreviewing = ref(false); // 默认开启代码模式，减小开销
const isFullScreen = ref(false);
const fullScreenTab = ref<'code' | 'preview'>('code');

watch(
  () => props.isStreaming,
  (streaming) => {
    if (!streaming) return;
    isPreviewing.value = false;
    isFullScreen.value = false;
    fullScreenTab.value = 'code';
  },
  { immediate: true },
);

const { registerModal, unregisterModal } = useModalHistory();
const modalId = `HtmlPreviewBlockFullScreen_${Math.random().toString(36).substring(2, 9)}`;
const imageNonce = Math.random().toString(36).substring(2, 15);

watch(isFullScreen, (newVal) => {
  if (newVal) {
    registerModal(modalId, () => {
      isFullScreen.value = false;
    });
  } else {
    unregisterModal(modalId);
  }
});

// 代码预览转义处理 (优先使用后端预渲染 syntect 高亮，无值时回退为安全 HTML 转义)
const highlightedCode = computed(() => {
  if (props.highlightedContent) {
    return props.highlightedContent;
  }
  return props.content
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#039;');
});

// 复制功能
const copyCode = async () => {
  if (props.isStreaming) return;
  try {
    await navigator.clipboard.writeText(props.content);
    // 这里如果以后有 Toast 提示可以加上
    console.log('[HTML Block] Code copied');
  } catch (err) {
    console.error('[HTML Block] Copy failed', err);
  }
};

// 构造沙箱 HTML
// 统一门禁基线（见 plan/vcpmobile-widget-system-research/01 §6.1）：
// 不做内容清洗——iframe sandbox + opaque origin 才是真边界，保留 <script> 的 DOMPurify 形同虚设。
const sandboxHtml = computed(() => {
  const content = props.content;
  const isDark = themeStore.isDarkResolved;

  const vcpInjections = `
    <style>
      ::-webkit-scrollbar { width: 5px !important; height: 5px !important; }
      ::-webkit-scrollbar-track { background: transparent !important; }
      ::-webkit-scrollbar-thumb { background: ${isDark ? 'rgba(255,255,255,0.1)' : 'rgba(0,0,0,0.1)'} !important; border-radius: 10px !important; }
      html, body { 
        background-color: transparent !important; 
        color: ${isDark ? '#d1d5db' : '#374151'}; 
      }
      body { margin: 0; padding: 16px; box-sizing: border-box; min-height: 100%; }
      canvas, img, video, iframe { max-width: 100% !important; }
    </style>
    <` + `script>
      document.addEventListener('click', function(e) {
        const target = e.target.closest('a');
        if (target) {
          const href = target.getAttribute('href');
          if (!href || href === '#' || href.startsWith('javascript:')) {
            e.preventDefault();
          }
        }

        // 捕获并拦截图片点击，发送 postMessage 放大查看
        const img = e.target.closest('img');
        if (img) {
          e.preventDefault();
          window.parent.postMessage({
            source: 'vcp-mobile',
            type: 'rendered-image-click',
            nonce: '${imageNonce}',
            image: {
              src: img.src,
              alt: img.alt || '',
              title: img.title || ''
            }
          }, '*');
        }
      }, true);
    <` + `/script>
  `;

  if (/<head[^>]*>/i.test(content)) {
    return content.replace(/<head[^>]*>/i, `$&${vcpInjections}`);
  } else {
    return `<!DOCTYPE html><html><head>${vcpInjections}</head>${content}</html>`;
  }
});

const openFullScreen = () => {
  if (props.isStreaming) return;
  isFullScreen.value = true;
  fullScreenTab.value = isPreviewing.value ? 'preview' : 'code';
};

let refreshTimer: ReturnType<typeof setTimeout> | null = null;

const refreshPreview = () => {
  if (props.isStreaming) return;
  const iframe = isFullScreen.value 
    ? document.querySelector('.vcp-fullscreen-iframe') as HTMLIFrameElement
    : document.querySelector('.vcp-inline-iframe') as HTMLIFrameElement;
  
  if (iframe) {
    const currentSrc = iframe.srcdoc;
    iframe.srcdoc = '';
    if (refreshTimer) clearTimeout(refreshTimer);
    refreshTimer = setTimeout(() => {
      iframe.srcdoc = currentSrc;
    }, 50);
  }
};

// 同步普通视图与全屏视图的状态
watch(isPreviewing, (val) => {
  if (isFullScreen.value) {
    fullScreenTab.value = val ? 'preview' : 'code';
  }
});

watch(fullScreenTab, (val) => {
  isPreviewing.value = val === 'preview';
});

onUnmounted(() => {
  if (refreshTimer) clearTimeout(refreshTimer);
  unregisterModal(modalId);
});
</script>

<template>
  <div class="html-preview-block mb-4 rounded-2xl border overflow-hidden transition-all duration-300"
    :class="themeStore.isDarkResolved ? 'border-white/10 bg-[#0d1117]/80' : 'border-black/5 bg-white/90'">
    
    <!-- 全屏页面 (Kimi 风格沙箱) -->
    <Teleport to="body">
      <Transition
        enter-active-class="transition duration-300 ease-out"
        enter-from-class="translate-y-10 opacity-0"
        enter-to-class="translate-y-0 opacity-100"
        leave-active-class="transition duration-200 ease-in"
        leave-from-class="translate-y-0 opacity-100"
        leave-to-class="translate-y-10 opacity-0"
      >
        <div v-if="isFullScreen" class="vcp-safe-inline fixed inset-0 z-editor flex flex-col pb-[var(--vcp-safe-bottom,0px)]"
          :class="themeStore.isDarkResolved ? 'bg-[#0d1117]' : 'bg-[#f6f8fa] text-gray-900'">
          
          <!-- 全屏 Header -->
          <div class="h-14 flex items-center justify-between px-4 border-b pt-[var(--vcp-safe-top,0px)] box-content"
            :class="themeStore.isDarkResolved ? 'border-white/5 bg-[#0d1117]' : 'border-black/5 bg-white'">
            <div class="flex items-center gap-4">
              <button @click="isFullScreen = false" class="p-2 -ml-2 active:scale-90 transition-transform">
                <div class="i-ph:caret-left-bold w-5 h-5" :class="themeStore.isDarkResolved ? 'text-gray-400' : 'text-gray-600'"></div>
              </button>
              <div class="flex flex-col">
                <span class="text-sm font-bold uppercase tracking-wider" :class="themeStore.isDarkResolved ? 'text-gray-200' : 'text-gray-800'">html</span>
              </div>
            </div>

            <div class="flex items-center gap-4">
              <button v-if="fullScreenTab === 'preview'" :disabled="isStreaming" @click="refreshPreview" class="p-2 active:rotate-180 transition-transform duration-500 disabled:opacity-30">
                <div class="i-ph:arrow-clockwise-bold w-5 h-5 text-gray-400"></div>
              </button>
              <button v-else :disabled="isStreaming" @click="copyCode" class="p-2 active:scale-90 transition-transform disabled:opacity-30">
                <div class="i-ph:copy-bold w-5 h-5 text-gray-400"></div>
              </button>
              
              <div v-if="!isStreaming" class="flex p-1 rounded-xl border transition-colors duration-300" :class="themeStore.isDarkResolved ? 'bg-white/5 border-white/5' : 'bg-black/5 border-black/5'">
                <button @click="fullScreenTab = 'code'"
                  :class="[fullScreenTab === 'code' ? (themeStore.isDarkResolved ? 'bg-white/10 text-white shadow-md border-white/5' : 'bg-white text-gray-900 shadow-sm border-black/5') : (themeStore.isDarkResolved ? 'text-gray-400' : 'text-gray-500')]"
                  class="px-4 py-1 text-[11px] font-bold rounded-lg transition-all border border-transparent">代码</button>
                <button @click="fullScreenTab = 'preview'"
                  :class="[fullScreenTab === 'preview' ? (themeStore.isDarkResolved ? 'bg-white/10 text-white shadow-md border-white/5' : 'bg-white text-gray-900 shadow-sm border-black/5') : (themeStore.isDarkResolved ? 'text-gray-400' : 'text-gray-500')]"
                  class="px-4 py-1 text-[11px] font-bold rounded-lg transition-all border border-transparent">预览</button>
              </div>
            </div>
          </div>

          <!-- 全屏内容区 -->
          <div class="flex-1 overflow-hidden relative" :class="themeStore.isDarkResolved ? 'bg-[#0d1117]' : 'bg-white'">
            <div v-show="fullScreenTab === 'code'" 
              class="absolute inset-0 overflow-auto p-4 text-xs font-mono leading-relaxed vcp-scrollable"
              :class="[
                themeStore.isDarkResolved 
                  ? 'bg-[#0d1117] text-[#c9d1d9]' 
                  : 'bg-[#f6f8fa] text-[#24292e]'
              ]">
              <div v-if="highlightedContent" class="vcp-html-highlighted-wrapper" v-html="highlightedCode"></div>
              <pre v-else><code class="hljs" v-html="highlightedCode"></code></pre>
            </div>
            <iframe 
              v-show="fullScreenTab === 'preview'"
              class="vcp-fullscreen-iframe w-full h-full border-none"
              sandbox="allow-scripts"
              loading="lazy"
              :srcdoc="sandboxHtml"
              :data-vcp-image-nonce="imageNonce"
            ></iframe>
          </div>
        </div>
      </Transition>
    </Teleport>

    <!-- 普通视图 Header (比全屏模式略小一点点，保持呼吸感) -->
    <div class="h-12 flex items-center justify-between px-3.5 border-b relative z-10 box-content transition-colors duration-300"
      :class="themeStore.isDarkResolved ? 'bg-[#161b22] border-white/5' : 'bg-[#f6f8fa] border-black/5'">
      <div class="flex items-center gap-2.5">
        <div class="i-ph:code-block-bold w-4 h-4 text-emerald-500"></div>
        <span class="text-xs font-bold uppercase tracking-wider" :class="themeStore.isDarkResolved ? 'text-gray-200' : 'text-gray-800'">html</span>
      </div>
      
      <div class="flex items-center gap-3">
        <!-- 功能按钮：尺寸适中 -->
        <button v-if="isPreviewing" :disabled="isStreaming" @click.stop="refreshPreview"
          class="p-1.5 active:rotate-180 transition-transform duration-500 opacity-60 hover:opacity-100 disabled:opacity-30">
          <div class="i-ph:arrow-clockwise-bold w-5 h-5" :class="themeStore.isDarkResolved ? 'text-gray-400' : 'text-gray-600'"></div>
        </button>
        <button v-else :disabled="isStreaming" @click.stop="copyCode"
          class="p-1.5 active:scale-90 transition-transform opacity-60 hover:opacity-100 disabled:opacity-30">
          <div class="i-ph:copy-bold w-5 h-5" :class="themeStore.isDarkResolved ? 'text-gray-400' : 'text-gray-600'"></div>
        </button>

        <button :disabled="isStreaming" @click.stop="openFullScreen"
          class="p-1.5 active:scale-90 transition-transform opacity-60 hover:opacity-100 disabled:opacity-30">
          <div class="i-ph:arrows-out-bold w-4.5 h-4.5" :class="themeStore.isDarkResolved ? 'text-gray-400' : 'text-gray-600'"></div>
        </button>

        <div v-if="!isStreaming" class="flex p-0.8 rounded-xl border transition-colors duration-300"
          :class="themeStore.isDarkResolved ? 'bg-white/5 border-white/5' : 'bg-black/5 border-black/5'">
          <button @click.stop="isPreviewing = false"
            :class="[!isPreviewing ? (themeStore.isDarkResolved ? 'bg-white/10 text-white shadow-md border-white/5' : 'bg-white text-gray-900 shadow-sm border-black/5') : (themeStore.isDarkResolved ? 'text-gray-400' : 'text-gray-500')]"
            class="px-3 py-1 text-[10px] font-bold rounded-lg transition-all border border-transparent">代码</button>
          <button @click.stop="isPreviewing = true"
            :class="[isPreviewing ? (themeStore.isDarkResolved ? 'bg-white/10 text-white shadow-md border-white/5' : 'bg-white text-gray-900 shadow-sm border-black/5') : (themeStore.isDarkResolved ? 'text-gray-400' : 'text-gray-500')]"
            class="px-3 py-1 text-[10px] font-bold rounded-lg transition-all border border-transparent">预览</button>
        </div>
      </div>
    </div>

    <!-- 普通视图内容 (自适应优化：预览时保持 380px，展示代码时根据内容自适应收缩) -->
    <div class="relative transition-all duration-300 overflow-hidden no-swipe"
      :class="[isPreviewing ? 'h-[380px]' : 'h-auto']">
      <div v-show="!isPreviewing"
        class="w-full overflow-auto max-h-[380px] p-3 text-[10px] font-mono leading-relaxed vcp-scrollable no-swipe"
        :class="[
          themeStore.isDarkResolved 
            ? 'bg-[#0d1117] text-[#c9d1d9]' 
            : 'bg-[#f6f8fa] text-[#24292e]'
        ]">
        <slot name="code">
          <div v-if="highlightedContent" class="vcp-html-highlighted-wrapper" v-html="highlightedCode"></div>
          <pre v-else class="w-full min-w-max"><code class="hljs" v-html="highlightedCode"></code></pre>
        </slot>
      </div>

      <div v-if="isPreviewing" class="absolute inset-0 no-swipe" :class="themeStore.isDarkResolved ? 'bg-[#0d1117]' : 'bg-white'">
        <iframe 
          class="vcp-inline-iframe w-full h-full border-none no-swipe"
          sandbox="allow-scripts"
          loading="lazy"
          :srcdoc="sandboxHtml"
          :data-vcp-image-nonce="imageNonce"
        ></iframe>
      </div>
    </div>
  </div>
</template>

<style scoped>
.html-preview-block {
  /* 极致轻盈的现代双层散焦微阴影，杜绝死黑与大范围污染 */
  box-shadow: 0 4px 20px -6px rgba(0, 0, 0, 0.12), 0 2px 8px -2px rgba(0, 0, 0, 0.04);
}

:root.dark .html-preview-block {
  /* 暗色模式下微调投影透明度，维持极简科技感，避免脏底 */
  box-shadow: 0 4px 20px -6px rgba(0, 0, 0, 0.35);
}

/* 高亮代码基础样式 */
.hljs { display: block; overflow-x: auto; padding: 0; background: transparent; }

/* 暗色模式高亮 (GitHub Dark 风格适配) */
.html-preview-block :deep(.hljs-tag), 
.html-preview-block :deep(.hljs-name), 
.html-preview-block :deep(.hljs-keyword) { color: #ff7b72; }
.html-preview-block :deep(.hljs-attr) { color: #79c0ff; }
.html-preview-block :deep(.hljs-string) { color: #a5d6ff; }
.html-preview-block :deep(.hljs-comment) { color: #8b949e; font-style: italic; }
.html-preview-block :deep(.hljs-meta) { color: #ff7b72; }

/* 亮色模式高亮适配 (GitHub Light 风格适配) */
/* 使用 :not(.dark) 或通过父级类名区分 */
.bg-white .hljs-tag, 
.bg-white .hljs-name, 
.bg-white .hljs-keyword { color: #d73a49; }
.bg-white .hljs-attr { color: #005cc5; }
.bg-white .hljs-string { color: #032f62; }
.bg-white .hljs-comment { color: #6a737d; font-style: italic; }
.bg-white .hljs-meta { color: #d73a49; }

/* 专属 vcp-html-block 样式隔离与重置 */
.vcp-html-highlighted-wrapper :deep(pre),
.vcp-html-highlighted-wrapper :deep(code) {
  margin: 0 !important;
  padding: 0 !important;
  background: transparent !important;
  border: none !important;
  font-size: inherit !important;
  font-family: inherit !important;
  line-height: inherit !important;
  box-shadow: none !important;
  white-space: pre !important;
  overflow-x: auto !important;
}
.vcp-html-highlighted-wrapper :deep(span) {
  display: inline !important;
  white-space: pre !important;
}
.vcp-html-highlighted-wrapper :deep(code) {
  padding: 0 !important;
  background: transparent !important;
}
</style>
