import { createApp } from "vue";
import { createPinia } from "pinia";
import piniaPluginPersistedstate from "pinia-plugin-persistedstate";
import App from "./App.vue";
import { router } from "./core/router";
import { vIntersectionObserver } from "./core/directives/intersectionObserver";
import { vLongpress } from "./core/directives/longpress";
import { installRuntimeDiagnostics } from "./core/utils/runtimeDiagnostics";
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';

import 'virtual:uno.css'
import "@unocss/reset/tailwind.css"
import "./assets/themes.css"
import "./assets/message-blocks.css"
import "katex/dist/katex.min.css"

const app = createApp(App);
const pinia = createPinia();
const currentWin = getCurrentWindow();
installRuntimeDiagnostics(app, currentWin.label);
pinia.use(piniaPluginPersistedstate);

app.use(pinia);

app.use(router);
app.directive('intersection-observer', vIntersectionObserver);
app.directive('longpress', vLongpress);
app.mount("#app");

// 标记前端启动成功（用于 OTA 回滚保护）
if (currentWin.label === 'main') {
  invoke('confirm_frontend_boot').catch(() => {});
}
