import { createRouter, createWebHashHistory } from 'vue-router';
import ChatView from '../../features/chat/ChatView.vue';
import AssistantView from '../../features/assistant/AssistantView.vue';

const routes = [
  { path: '/', redirect: '/chat' },
  { path: '/chat', name: 'chat', component: ChatView },
  { path: '/assistant', name: 'assistant', component: AssistantView },
];

export const router = createRouter({
  history: createWebHashHistory(),
  routes,
});
