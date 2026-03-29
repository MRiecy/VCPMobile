<script setup lang="ts">
import { ref, watch, onMounted } from 'vue';

const props = defineProps<{
  content: string;
  messageId: string;
}>();

const isPreviewing = ref(false);
const iframeHeight = ref('100px');
const iframeRef = ref<HTMLIFrameElement | null>(null);

// 唯一 ID 用于 iframe 通讯
const frameId = `vcp-frame-${Math.random().toString(36).substr(2, 9)}`;

const togglePreview = () => {
  isPreviewing.value = !isPreviewing.value;
};

// 监听来自 iframe 的高度更新消息
onMounted(() => {
  window.addEventListener('message', (event) => {
    if (event.data && event.data.type === 'vcp-html-resize' && event.data.frameId === frameId) {
      iframeHeight.value = `${event.data.height}px`;
    }
  });
});

const iframeSrcdoc = ref('');

watch([() => props.content, isPreviewing], () => {
  if (isPreviewing.value) {
    // 构造沙箱 HTML 环境
    iframeSrcdoc.value = `
      <!DOCTYPE html>
      <html>
      <head>
          <meta charset="UTF-8">
          <!-- 核心防线 1：强制移动端视口，禁止缩放，保证渲染一致性 -->
          <meta name="viewport" content="width=device-width, initial-scale=1.0, maximum-scale=1.0, user-scalable=no">
          <style>
              /* 核心防线 2：横向溢出保护 */
              html, body {
                  margin: 0;
                  padding: 0;
                  /* 允许内部横向滚动，禁止纵向溢出（由外部撑开） */
                  overflow-x: auto;
                  overflow-y: hidden;
                  height: auto;
                  width: 100%;
              }
              body {
                  padding: 16px;
                  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
                  background: transparent;
                  /* 使用更中性的颜色，防止暗黑模式下文本不可见 */
                  color: #d1d5db;
                  line-height: 1.6;
                  box-sizing: border-box;
                  min-height: 60px;
              }
              * { box-sizing: border-box; }
              /* 强制约束大元素 */
              img, video, iframe, canvas, table { max-width: 100% !important; height: auto; }
              /* 防止长单词撑破屏幕 */
              p, div, span, h1, h2, h3, h4, h5, h6, td, th { word-wrap: break-word !important; word-break: break-word !important; }
              /* 美化内部代码块滚动条 */
              ::-webkit-scrollbar { width: 4px; height: 4px; }
              ::-webkit-scrollbar-thumb { background: rgba(255,255,255,0.2); border-radius: 4px; }
          </style>
      </head>
      <body>
          <div id="vcp-wrapper">${props.content}</div>
          <script>
              function updateHeight() {
                  const wrapper = document.getElementById('vcp-wrapper');
                  const height = Math.max(wrapper.scrollHeight + 40, document.body.scrollHeight);
                  window.parent.postMessage({
                      type: 'vcp-html-resize',
                      height: height,
                      frameId: '${frameId}'
                  }, '*');
              }
              window.onload = () => {
                  setTimeout(updateHeight, 50);
                  setTimeout(updateHeight, 500);
              };
              new ResizeObserver(updateHeight).observe(document.body);
          </` + `script>
      </body>
      </html>
    `.replace(/<script>/g, '<scr' + 'ipt>');
  }
}, { immediate: true });
</script>

<template>
  <div class="html-preview-block mb-2 rounded-xl border border-white/10 bg-black/5 overflow-hidden backdrop-blur-sm">
    <div class="flex items-center justify-between px-3 py-2 bg-white/5 border-b border-white/5">
      <span class="text-[10px] font-bold uppercase tracking-widest opacity-50 flex items-center gap-2">
        <div class="i-ph:code-block w-3 h-3 text-emerald-400"></div>
        HTML 代码块
      </span>
      <button 
        @click.stop="togglePreview"
        class="text-[10px] px-2 py-1 rounded-lg bg-emerald-500/10 text-emerald-400 hover:bg-emerald-500/20 transition-all flex items-center gap-1 border border-emerald-500/20"
      >
        <span v-if="isPreviewing" class="i-ph:code w-3 h-3"></span>
        <span v-else class="i-ph:play w-3 h-3"></span>
        {{ isPreviewing ? '返回代码' : '播放预览' }}
      </button>
    </div>
    
    <div class="p-0 transition-all duration-300">
      <!-- 代码视图 (优化了溢出与换行策略) -->
      <div v-show="!isPreviewing" class="max-h-[50vh] w-full overflow-x-auto overflow-y-auto bg-[#0d1117] p-3 text-[11px] font-mono text-gray-300 leading-relaxed rounded-b-xl custom-scrollbar">
        <!-- 去掉 break-all，改用原生水平滚动，保留代码格式缩进 -->
        <pre class="w-full min-w-max"><code class="whitespace-pre">{{ content }}</code></pre>
      </div>
      
      <!-- 预览视图 (iframe 沙箱：增加了最大宽度限制和滚动隔离) -->
      <div v-show="isPreviewing" class="w-full max-w-full relative bg-[#1e1e1e]/50 backdrop-blur-md rounded-b-xl overflow-hidden" :style="{ height: iframeHeight, transition: 'height 0.3s ease' }">
        <iframe
          v-if="isPreviewing"
          ref="iframeRef"
          :srcdoc="iframeSrcdoc"
          class="w-full h-full border-none absolute inset-0 block"
          sandbox="allow-scripts allow-popups"
        ></iframe>
      </div>
    </div>
  </div>
</template>

<style scoped>
.html-preview-block {
  box-shadow: 0 8px 24px -8px rgba(0, 0, 0, 0.2);
  /* 终极防线：组件级宽度约束 */
  max-width: 100%;
  min-width: 0;
}

/* 自定义代码块滚动条 */
.custom-scrollbar::-webkit-scrollbar {
  width: 6px;
  height: 6px;
}
.custom-scrollbar::-webkit-scrollbar-track {
  background: rgba(0, 0, 0, 0.1);
}
.custom-scrollbar::-webkit-scrollbar-thumb {
  background: rgba(255, 255, 255, 0.15);
  border-radius: 3px;
}
.custom-scrollbar::-webkit-scrollbar-thumb:hover {
  background: rgba(255, 255, 255, 0.25);
}
</style>
