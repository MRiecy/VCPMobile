import { createApp } from "vue";
import { createPinia } from "pinia";
import AssistantView from "./features/assistant/AssistantView.vue";

import "./appStyles";

const app = createApp(AssistantView);
const pinia = createPinia();
app.use(pinia);
app.mount("#app");
