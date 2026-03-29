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
app.use(router);
app.directive('intersection-observer', vIntersectionObserver);
app.directive('longpress', vLongpress);
app.mount("#app");
