import { createRouter, createWebHashHistory } from 'vue-router';
import ChatView from '../views/ChatView.vue';
import SettingsView from '../views/SettingsView.vue';
import AgentSettingsView from '../views/AgentSettingsView.vue';

const routes = [
  { path: '/', redirect: '/chat' },
  { path: '/agents/:id', component: AgentSettingsView, props: true },
  { path: '/chat', component: ChatView },
  { path: '/settings', component: SettingsView },
];

export const router = createRouter({
  history: createWebHashHistory(),
  routes,
});
