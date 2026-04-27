<script setup lang="ts">
import { ref, watch, computed } from 'vue';
import DOMPurify from 'dompurify';
import hljs from 'highlight.js';
import { useThemeStore } from '../../../core/stores/theme';

const props = defineProps<{
  content: string;
  messageId: string;
}>();

const themeStore = useThemeStore();
const isPreviewing = ref(false); // 默认开启代码模式，减小开销
const isFullScreen = ref(false);
const fullScreenTab = ref<'code' | 'preview'>('code');

// 语法高亮处理
const highlightedCode = computed(() => {
  return hljs.highlight(props.content, { language: 'xml' }).value;
});

// 复制功能
const copyCode = async () => {
  try {
    await navigator.clipboard.writeText(props.content);
    // 这里如果以后有 Toast 提示可以加上
    console.log('[HTML Block] Code copied');
  } catch (err) {
    console.error('[HTML Block] Copy failed', err);
  }
};

// 构造沙箱 HTML
const getSandboxHtml = (content: string) => {
  const isDark = themeStore.isDarkResolved;
  
  const cleanHtml = DOMPurify.sanitize(content, {
    USE_PROFILES: { html: true, svg: true, mathMl: true },
    ADD_TAGS: ['style', 'iframe', 'canvas', 'script', 'link', 'meta'], 
    ADD_ATTR: ['*'],
    FORBID_TAGS: ['applet', 'embed', 'object'],
    ALLOW_UNKNOWN_PROTOCOLS: true,
    WHOLE_DOCUMENT: true,
    RETURN_DOM: false
  });

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
      }, true);

      const _originalAlert = window.alert;
      window.alert = function(msg) {
        console.log('[VCP Sandbox Alert]:', msg);
        try { _originalAlert(msg); } catch (e) {}
      };
    <` + `/script>
  `;

  if (/<head[^>]*>/i.test(cleanHtml)) {
    return cleanHtml.replace(/<head[^>]*>/i, `$&${vcpInjections}`);
  } else {
    return `<!DOCTYPE html><html><head>${vcpInjections}</head>${cleanHtml}</html>`;
  }
};

const openFullScreen = () => {
  isFullScreen.value = true;
  fullScreenTab.value = isPreviewing.value ? 'preview' : 'code';
};

const refreshPreview = () => {
  const iframe = isFullScreen.value 
    ? document.querySelector('.vcp-fullscreen-iframe') as HTMLIFrameElement
    : document.querySelector('.vcp-inline-iframe') as HTMLIFrameElement;
  
  if (iframe) {
    const currentSrc = iframe.srcdoc;
    iframe.srcdoc = '';
    setTimeout(() => {
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
</script>

<template>
  <div class="html-preview-block mb-4 rounded-2xl border overflow-hidden transition-all duration-300"
    :class="themeStore.isDarkResolved ? 'border-white/10 bg-[#0d1117]/80 backdrop-blur-md' : 'border-black/5 bg-white/90 backdrop-blur-sm'">
    
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
        <div v-if="isFullScreen" class="fixed inset-0 z-[10000] flex flex-col"
          :class="themeStore.isDarkResolved ? 'bg-[#0d1117]' : 'bg-[#f6f8fa] text-gray-900'">
          
          <!-- 全屏 Header -->
          <div class="h-14 flex items-center justify-between px-4 border-b pt-[env(safe-area-inset-top)] box-content"
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
              <button v-if="fullScreenTab === 'preview'" @click="refreshPreview" class="p-2 active:rotate-180 transition-transform duration-500">
                <div class="i-ph:arrow-clockwise-bold w-5 h-5 text-gray-400"></div>
              </button>
              <button v-else @click="copyCode" class="p-2 active:scale-90 transition-transform">
                <div class="i-ph:copy-bold w-5 h-5 text-gray-400"></div>
              </button>
              
              <div class="flex p-1 rounded-xl border" :class="themeStore.isDarkResolved ? 'bg-white/5 border-white/5' : 'bg-black/5 border-black/5'">
                <button @click="fullScreenTab = 'code'"
                  :class="[fullScreenTab === 'code' ? 'bg-white/10 text-white shadow-lg' : themeStore.isDarkResolved ? 'text-gray-500' : 'text-gray-400']"
                  class="px-4 py-1 text-[11px] font-bold rounded-lg transition-all">代码</button>
                <button @click="fullScreenTab = 'preview'"
                  :class="[fullScreenTab === 'preview' ? 'bg-white/10 text-white shadow-lg' : themeStore.isDarkResolved ? 'text-gray-500' : 'text-gray-400']"
                  class="px-4 py-1 text-[11px] font-bold rounded-lg transition-all">预览</button>
              </div>
            </div>
          </div>

          <!-- 全屏内容区 -->
          <div class="flex-1 overflow-hidden relative" :class="themeStore.isDarkResolved ? 'bg-[#0d1117]' : 'bg-white'">
            <div v-show="fullScreenTab === 'code'" 
              class="absolute inset-0 overflow-auto p-4 text-xs font-mono leading-relaxed vcp-scrollable"
              :class="themeStore.isDarkResolved ? 'text-gray-300' : 'text-gray-700'">
              <pre><code class="hljs" v-html="highlightedCode"></code></pre>
            </div>
            <iframe 
              v-show="fullScreenTab === 'preview'"
              class="vcp-fullscreen-iframe w-full h-full border-none"
              sandbox="allow-scripts allow-modals allow-forms allow-popups"
              :srcdoc="getSandboxHtml(content)"
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
        <button v-if="isPreviewing" @click.stop="refreshPreview" 
          class="p-1.5 active:rotate-180 transition-transform duration-500 opacity-60 hover:opacity-100">
          <div class="i-ph:arrow-clockwise-bold w-5 h-5" :class="themeStore.isDarkResolved ? 'text-gray-400' : 'text-gray-600'"></div>
        </button>
        <button v-else @click.stop="copyCode" 
          class="p-1.5 active:scale-90 transition-transform opacity-60 hover:opacity-100">
          <div class="i-ph:copy-bold w-5 h-5" :class="themeStore.isDarkResolved ? 'text-gray-400' : 'text-gray-600'"></div>
        </button>

        <button @click.stop="openFullScreen"
          class="p-1.5 active:scale-90 transition-transform opacity-60 hover:opacity-100">
          <div class="i-ph:arrows-out-bold w-4.5 h-4.5" :class="themeStore.isDarkResolved ? 'text-gray-400' : 'text-gray-600'"></div>
        </button>

        <div class="flex p-0.8 rounded-xl border transition-colors duration-300" 
          :class="themeStore.isDarkResolved ? 'bg-white/5 border-white/5' : 'bg-black/5 border-black/5'">
          <button @click.stop="isPreviewing = false"
            :class="[!isPreviewing ? 'bg-white/10 text-white shadow-md' : themeStore.isDarkResolved ? 'text-gray-500' : 'text-gray-400']"
            class="px-3 py-1 text-[10px] font-bold rounded-lg transition-all">代码</button>
          <button @click.stop="isPreviewing = true"
            :class="[isPreviewing ? 'bg-white/10 text-white shadow-md' : themeStore.isDarkResolved ? 'text-gray-500' : 'text-gray-400']"
            class="px-3 py-1 text-[10px] font-bold rounded-lg transition-all">预览</button>
        </div>
      </div>
    </div>

    <!-- 普通视图内容 (修复消失问题：使用 h-固定高度) -->
    <div class="relative h-[380px] transition-all duration-300 overflow-hidden no-swipe">
      <div v-show="!isPreviewing"
        class="absolute inset-0 overflow-auto p-3 text-[10px] font-mono leading-relaxed vcp-scrollable no-swipe"
        :class="themeStore.isDarkResolved ? 'bg-[#0d1117] text-gray-400' : 'bg-[#fafafa] text-gray-600'">
        <pre class="w-full min-w-max"><code class="hljs" v-html="highlightedCode"></code></pre>
      </div>

      <div v-show="isPreviewing" class="absolute inset-0 no-swipe" :class="themeStore.isDarkResolved ? 'bg-[#0d1117]' : 'bg-white'">
        <iframe 
          class="vcp-inline-iframe w-full h-full border-none no-swipe"
          sandbox="allow-scripts allow-modals allow-forms"
          :srcdoc="getSandboxHtml(content)"
        ></iframe>
      </div>
    </div>
  </div>
</template>

<style scoped>
.html-preview-block {
  box-shadow: 0 12px 40px -12px rgba(0, 0, 0, 0.5);
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
</style>
