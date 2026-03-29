import { createRouter, createWebHashHistory } from 'vue-router';
import ChatView from '../../features/chat/ChatView.vue';
import AgentSettingsView from '../../features/agent/AgentSettingsView.vue';

const routes = [
  { path: '/', redirect: '/chat' },
  { path: '/agents/:id', name: 'agent-settings', component: AgentSettingsView, props: true },
  { path: '/chat', name: 'chat', component: ChatView },
];

export const router = createRouter({
  history: createWebHashHistory(),
  routes,
});
