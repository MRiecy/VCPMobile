import { createApp } from "vue";
import { createPinia } from "pinia";
import { listen } from "@tauri-apps/api/event";
import App from "./App.vue";
import { router } from "./core/router";
import { vIntersectionObserver } from "./core/directives/intersectionObserver";
import { vLongpress } from "./core/directives/longpress";

import 'virtual:uno.css'
import "@unocss/reset/tailwind.css"

const app = createApp(App);
const pinia = createPinia();

app.use(pinia);

// 全局兜底：尽早注册 sync-completed 监听，防止 store 懒加载错过事件
listen("vcp-sync-completed", async (event) => {
  console.log("[GlobalSyncListener] vcp-sync-completed received:", event.payload);
  const { useAssistantStore } = await import("./core/stores/assistant");
  const assistantStore = useAssistantStore(pinia);
  await assistantStore.fetchAgents();
  await assistantStore.fetchGroups();

  const { useTopicStore } = await import("./core/stores/topicListManager");
  const topicStore = useTopicStore(pinia);
  topicStore.invalidateAllTopicCaches();
});

import('./core/stores/chatManager').then(({ useChatManagerStore }) => {
  const chatManagerStore = useChatManagerStore(pinia);
  chatManagerStore.ensureEventListenersRegistered();
});

app.use(router);
app.directive('intersection-observer', vIntersectionObserver);
app.directive('longpress', vLongpress);
app.mount("#app");
