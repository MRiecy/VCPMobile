import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import { router } from "./core/router";
import { vIntersectionObserver } from "./core/directives/intersectionObserver";
import { vLongpress } from "./core/directives/longpress";

import 'virtual:uno.css'
import "@unocss/reset/tailwind.css"

const app = createApp(App);
const pinia = createPinia();

app.use(pinia);

import('./core/stores/chatManager').then(({ useChatManagerStore }) => {
  const chatManagerStore = useChatManagerStore(pinia);
  chatManagerStore.ensureEventListenersRegistered();
});

app.use(router);
app.directive('intersection-observer', vIntersectionObserver);
app.directive('longpress', vLongpress);
app.mount("#app");

// 标记前端启动成功（用于 OTA 回滚保护）
import { invoke } from '@tauri-apps/api/core';
invoke('confirm_frontend_boot').catch(() => {});
