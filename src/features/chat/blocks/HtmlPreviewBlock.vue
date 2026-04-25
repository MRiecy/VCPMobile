<script setup lang="ts">
import { ref, watch, onMounted, nextTick } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import DOMPurify from 'dompurify';

const props = defineProps<{
  content: string;
  messageId: string;
}>();

const isPreviewing = ref(false);
const shadowContainerRef = ref<HTMLElement | null>(null);
let shadowRoot: ShadowRoot | null = null;

const togglePreview = () => {
  isPreviewing.value = !isPreviewing.value;
};

// 开启原生全屏传送门 (极限性能 + UI 壳子)
const openNativePortal = async () => {
  try {
    // 1. 安全清洗 (全屏特许配置：允许脚本和交互)
    // 注意：这里我们放宽限制，因为这是一个独立的“原生门户”窗口
    const cleanHtml = DOMPurify.sanitize(props.content, {
      USE_PROFILES: { html: true, svg: true, mathMl: true },
      ADD_TAGS: ['style', 'iframe', 'canvas', 'script'],
      ADD_ATTR: ['*'], // 全面放行属性，包括各类触摸和滚轮手势
      ALLOW_UNKNOWN_PROTOCOLS: true,
      FORCE_BODY: true
    });

    // 2. 构建 UI 壳子 (包含顶部导航和返回按钮)
    // 关键修复：使用 window.__TAURI_INTERNALS__.invoke('plugin:window|close') 强制关闭窗口
    const fullPageHtml = `
      <!DOCTYPE html>
      <html>
      <head>
        <meta charset="UTF-8">
        <meta name="viewport" content="width=device-width, initial-scale=1.0, maximum-scale=1.0, user-scalable=no, viewport-fit=cover">
        <style>
          body { margin: 0; padding: 0; background: #000; color: #fff; font-family: -apple-system, sans-serif; overflow-x: hidden; }
          .vcp-portal-header {
            position: fixed; top: 0; left: 0; right: 0; height: 64px;
            display: flex; align-items: center; justify-content: space-between;
            padding: 0 16px; padding-top: env(safe-area-inset-top);
            background: rgba(0,0,0,0.6); backdrop-filter: blur(20px); -webkit-backdrop-filter: blur(20px);
            border-bottom: 1px solid rgba(255,255,255,0.1); z-index: 99999;
          }
          .vcp-portal-title { font-size: 13px; font-weight: 600; opacity: 0.6; text-transform: uppercase; letter-spacing: 1px; }
          .vcp-close-btn {
            background: rgba(226, 54, 56, 0.2); border: 1px solid rgba(226, 54, 56, 0.3);
            color: #ff6b6b; padding: 6px 14px; border-radius: 12px; font-size: 12px; font-weight: 600;
            cursor: pointer; transition: all 0.2s;
          }
          .vcp-close-btn:active { transform: scale(0.95); background: rgba(226, 54, 56, 0.4); }
          .vcp-portal-content { padding-top: calc(64px + env(safe-area-inset-top)); min-height: 100vh; box-sizing: border-box; }
          
          /* 允许 3D/Canvas 内容自由发挥 */
          img, video, iframe { max-width: 100% !important; }
          canvas { display: block; max-width: 100%; }
        </style>
      </head>
      <body>
        <div class="vcp-portal-header">
          <div class="vcp-portal-title">VCP Native Render</div>
          <button class="vcp-close-btn" onclick="exitPortal()">退出预览</button>
        </div>
        <div class="vcp-portal-content">
          ${cleanHtml}
        </div>
        <` + `script>
          function exitPortal() {
            try {
              // Tauri v2 标准关闭指令 (已修复：调用自定义的无限制关闭 API)
              if (window.__TAURI_INTERNALS__) {
                window.__TAURI_INTERNALS__.invoke('close_native_portal');
              } else {
                window.close();
              }
            } catch (e) {
              console.error('Failed to close window:', e);
              window.close();
            }
          }
        <` + `/script>
      </body>
      </html>
    `;

    await invoke('open_native_portal', { html: fullPageHtml });
  } catch (e) {
    console.error('[HtmlPreviewBlock] Failed to open native portal:', e);
  }
};

// 核心：更新影子沙箱内容 (移除专属解析，保持纯净 HTML)
const updateShadowContent = async () => {
  if (!shadowContainerRef.value || !isPreviewing.value) return;

  if (!shadowRoot) {
    shadowRoot = shadowContainerRef.value.attachShadow({ mode: 'open' });
  }

  const cleanHtml = DOMPurify.sanitize(props.content, {
    USE_PROFILES: { html: true, svg: true, mathMl: true },
    ADD_TAGS: ['style', 'iframe', 'canvas'], 
    ADD_ATTR: ['*'],
    FORBID_TAGS: ['script', 'object', 'embed', 'applet'],
    FORCE_BODY: true
  });

  const contentWrapper = document.createElement('div');
  contentWrapper.className = 'vcp-shadow-wrapper';
  contentWrapper.innerHTML = cleanHtml;

  const style = document.createElement('style');
  style.textContent = `
    :host { 
      display: block; width: 100%; background: transparent;
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
      color: var(--primary-text, #d1d5db); line-height: 1.6;
    }
    .vcp-shadow-wrapper { padding: 16px; box-sizing: border-box; overflow-x: auto; min-height: 250px; }
    * { box-sizing: border-box; }
    img, video, iframe, table { max-width: 100% !important; }
    canvas { max-width: 100%; height: auto; }
    p, div, span, h1, h2, h3, h4, h5, h6, td, th { word-wrap: break-word !important; word-break: break-word !important; }
    
    ::-webkit-scrollbar { width: 4px; height: 4px; }
    ::-webkit-scrollbar-thumb { background: rgba(255,255,255,0.1); border-radius: 4px; }
  `;

  shadowRoot.innerHTML = '';
  shadowRoot.appendChild(style);
  shadowRoot.appendChild(contentWrapper);
};

watch([() => props.content, isPreviewing], () => {
  if (isPreviewing.value) {
    nextTick(updateShadowContent);
  }
});

onMounted(() => {
  if (isPreviewing.value) updateShadowContent();
});
</script>

<template>
  <div class="html-preview-block mb-2 rounded-xl border border-white/10 bg-black/5 overflow-hidden backdrop-blur-sm">
    <div class="flex items-center justify-between px-3 py-2 bg-white/5 border-b border-white/5">
      <span class="text-[10px] font-bold uppercase tracking-widest opacity-50 flex items-center gap-2">
        <div class="i-ph:code-block w-3 h-3 text-emerald-400"></div>
        HTML 影子沙箱
      </span>
      <div class="flex items-center gap-2">
        <button @click.stop="openNativePortal"
          class="text-[10px] px-2 py-1 rounded-lg bg-blue-500/10 text-blue-400 hover:bg-blue-500/20 transition-all flex items-center gap-1 border border-blue-500/20">
          <span class="i-ph:frame-corners w-3 h-3"></span>
          原生全屏
        </button>
        <button @click.stop="togglePreview"
          class="text-[10px] px-2 py-1 rounded-lg bg-emerald-500/10 text-emerald-400 hover:bg-emerald-500/20 transition-all flex items-center gap-1 border border-emerald-500/20">
          <span v-if="isPreviewing" class="i-ph:code w-3 h-3"></span>
          <span v-else class="i-ph:play w-3 h-3"></span>
          {{ isPreviewing ? '返回代码' : '影子预览' }}
        </button>
      </div>
    </div>

    <div class="p-0 transition-all duration-300">
      <!-- 代码视图 -->
      <div v-show="!isPreviewing"
        class="max-h-[50vh] w-full overflow-x-auto overflow-y-auto bg-[#0d1117] p-3 text-[11px] font-mono text-gray-300 leading-relaxed rounded-b-xl custom-scrollbar">
        <pre class="w-full min-w-max"><code class="whitespace-pre">{{ content }}</code></pre>
      </div>

      <!-- 影子预览视图 (高度随内容自动撑开) -->
      <div v-show="isPreviewing"
        ref="shadowContainerRef"
        class="w-full max-w-full bg-[#1e1e1e]/30 backdrop-blur-md rounded-b-xl min-h-[60px]">
        <!-- Shadow Root 将挂载于此 -->
      </div>
    </div>
  </div>
</template>

<style scoped>
.html-preview-block {
  box-shadow: 0 8px 24px -8px rgba(0, 0, 0, 0.2);
  max-width: 100%;
  min-width: 0;
}

.custom-scrollbar::-webkit-scrollbar {
  width: 6px;
  height: 6px;
}

.custom-scrollbar::-webkit-scrollbar-track {
  background: rgba(0, 0, 0, 0.1);
}

.custom-scrollbar::-webkit-scrollbar-thumb:hover {
  background: rgba(255, 255, 255, 0.25);
}
</style>
