import { createApp } from "vue";
import { createPinia } from "pinia";
import AssistantView from "./features/assistant/AssistantView.vue";

import "virtual:uno.css";
import "@unocss/reset/tailwind.css";

const app = createApp(AssistantView);
const pinia = createPinia();
app.use(pinia);
app.mount("#app");
