import { createRouter, createWebHashHistory } from 'vue-router';
import ChatView from '../views/ChatView.vue';
import SettingsView from '../views/SettingsView.vue';
import AgentSettingsView from '../views/AgentSettingsView.vue';

const routes = [
  { path: '/', redirect: '/chat' },
  { path: '/agents/:id', name: 'agent-settings', component: AgentSettingsView, props: true },
  { path: '/chat', name: 'chat', component: ChatView },
  { path: '/settings', name: 'settings', component: SettingsView },
];

export const router = createRouter({
  history: createWebHashHistory(),
  routes,
});
