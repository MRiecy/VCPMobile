import { createRouter, createWebHashHistory } from 'vue-router';
import ChatView from '../../features/chat/ChatView.vue';

const routes = [
  { path: '/', redirect: '/chat' },
  { path: '/chat', name: 'chat', component: ChatView },
  { path: '/assistant', name: 'assistant', component: () => import('../../features/assistant/AssistantView.vue') },
];

export const router = createRouter({
  history: createWebHashHistory(),
  routes,
});
