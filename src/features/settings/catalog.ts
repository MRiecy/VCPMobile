import { defineAsyncComponent } from 'vue';
import type { SettingsPageEntry } from './contracts';

// Stable, lazy page components. Importing the directory never starts a store or task.
export const settingsPages: readonly SettingsPageEntry[] = [
  {
    id: 'identity', title: '用户身份', description: '头像、用户名与管理员账号', frame: 'list',
    component: defineAsyncComponent(() => import('./pages/IdentitySettingsPage.vue')),
  },
  {
    id: 'connection', title: '服务器连接', description: 'VCP Server API 与数据同步', frame: 'list',
    component: defineAsyncComponent(() => import('./pages/ConnectionSettingsPage.vue')),
  },
  {
    id: 'theme', title: '主题切换', description: '主题预览与切换', frame: 'viewport',
    component: defineAsyncComponent(() => import('./ThemePicker.vue')),
  },
  {
    id: 'rendering', title: '聊天偏好', description: '消息排版、内容宽度与流式体验', frame: 'list',
    component: defineAsyncComponent(() => import('../chat/settings/ChatPreferencesPage.vue')),
  },
  {
    id: 'voice', title: '语音设置', description: '语音预设、识别与朗读', frame: 'viewport',
    component: defineAsyncComponent(() => import('../chat/voice/VoiceSettingsPage.vue')),
  },
  {
    id: 'advanced', title: '高级功能', description: '话题总结与数据维护', frame: 'list',
    component: defineAsyncComponent(() => import('./pages/AdvancedSettingsPage.vue')),
  },
  {
    id: 'guide', title: '帮助与指引', description: '交互教学与功能指引回放', frame: 'list',
    component: defineAsyncComponent(() => import('../guide/components/GuideCenterSection.vue')),
  },
  {
    id: 'about', title: '关于', description: '版本与更新', frame: 'immersive',
    component: defineAsyncComponent(() => import('./components/AboutSection.vue')),
  },
  {
    id: 'developer', title: '开发者选项', description: '触感与调试工具', frame: 'list', parent: 'about',
    component: defineAsyncComponent(() => import('./developer/DeveloperSettingsPage.vue')),
  },
];
