import { createApp } from "vue";
import { createPinia } from "pinia";
import piniaPluginPersistedstate from "pinia-plugin-persistedstate";
import App from "./App.vue";
import { router } from "./core/router";
import { vIntersectionObserver } from "./core/directives/intersectionObserver";
import { vLongpress } from "./core/directives/longpress";

import "./appStyles";
import "./assets/message-blocks.css"
import "katex/dist/katex.min.css"

const app = createApp(App);
const pinia = createPinia();
pinia.use(piniaPluginPersistedstate);

// === 全局错误捕获与日志输出 ===
function formatAndLogError(type: string, error: any, context?: string) {
  const time = new Date().toISOString();
  let message = '';
  let stack = '';

  if (error instanceof Error) {
    message = error.message;
    stack = error.stack || '';
  } else if (typeof error === 'object' && error !== null) {
    try {
      message = JSON.stringify(error);
    } catch {
      message = String(error);
    }
  } else {
    message = String(error);
  }

  console.error(
    `[FRONTEND_ERROR][${time}][${type}] ${message}\n` +
    (context ? `Context: ${context}\n` : '') +
    (stack ? `Stack:\n${stack}` : 'No stack trace available')
  );
}

app.config.errorHandler = (err, _instance, info) => {
  formatAndLogError('VueErrorHandler', err, info);
};

window.addEventListener('error', (event) => {
  // 避免重复捕获已经被 Vue 处理的错误
  if (event.error) {
    formatAndLogError('GlobalError', event.error);
  } else {
    formatAndLogError('GlobalError', event.message);
  }
});

window.addEventListener('unhandledrejection', (event) => {
  formatAndLogError('UnhandledRejection', event.reason);
});
// ============================

app.use(pinia);

app.use(router);
app.directive('intersection-observer', vIntersectionObserver);
app.directive('longpress', vLongpress);
app.mount("#app");


// 标记前端启动成功（用于 OTA 回滚保护）
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
const currentWin = getCurrentWindow();
if (currentWin.label === 'main') {
  invoke('confirm_frontend_boot').catch(() => {});
}
