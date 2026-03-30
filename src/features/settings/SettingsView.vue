<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useSettingsStore, type AppSettings } from '../../core/stores/settings';

import UserProfileSection from './components/UserProfileSection.vue';
import SyncSettingsSection from './components/SyncSettingsSection.vue';
import VcpCoreSettingsSection from './components/VcpCoreSettingsSection.vue';
import AiLogicSettingsSection from './components/AiLogicSettingsSection.vue';
import TopicSummarySection from './components/TopicSummarySection.vue';
import MaintenanceSection from './components/MaintenanceSection.vue';
import ThemePicker from './ThemePicker.vue';
import ModelSelector from '../chat/ModelSelector.vue';

const props = withDefaults(defineProps<{
  isOpen?: boolean;
}>(), {
  isOpen: false
});

const emit = defineEmits<{
  close: [];
  openSync: [];
}>();

const settingsStore = useSettingsStore();

const settings = ref<AppSettings>({
  userName: 'User',
  vcpServerUrl: '',
  vcpApiKey: '',
  vcpLogUrl: '',
  vcpLogKey: '',
  enableAgentBubbleTheme: false,
  enableSmoothStreaming: false,
  enableDistributedServer: true,
  enableDistributedServerLogs: false,
  enableVcpToolInjection: false,
  syncServerIp: '',
  syncServerPort: 5974,
  syncToken: '',
  sidebarWidth: 260,
  notificationsSidebarWidth: 300,
  networkNotesPaths: [],
  minChunkBufferSize: 1,
  smoothStreamIntervalMs: 25,
  assistantAgent: '',
  agentMusicControl: false,
  combinedItemOrder: [],
  agentOrder: [],
  flowlockContinueDelay: 5,
  topicSummaryModel: 'gemini-2.5-flash',
  topicSummaryModelTemperature: 0.7
});

const loading = ref(true);
const showSummaryModelSelector = ref(false);

const onSummaryModelSelect = (modelId: string) => {
  settings.value.topicSummaryModel = modelId;
};

const closeSettings = () => {
  emit('close');
};

const openSyncView = () => {
  emit('openSync');
};

const loadSettings = async () => {
  try {
    await settingsStore.fetchSettings();
    if (settingsStore.settings) {
      settings.value = JSON.parse(JSON.stringify(settingsStore.settings));
    }
  } catch (e) {
    console.error('[SettingsView] Failed to load settings:', e);
  } finally {
    loading.value = false;
  }
};

const saveSettings = async () => {
  try {
    await invoke('write_app_settings', { settings: settings.value });
    await settingsStore.fetchSettings();
    console.log('Settings saved!');
  } catch (e) {
    console.error('Failed to save settings:', e);
  }
};

onMounted(() => {
  console.log('[SettingsView] Mounted, isOpen:', props.isOpen);
});

onUnmounted(() => {
  console.info('[SettingsView][Debug] component unmounted');
});

watch(() => props.isOpen, (val: boolean) => {
  console.log('[SettingsView] isOpen changed:', val);
  if (val) {
    loadSettings();
  }
});
</script>

<template>
  <Teleport to="#vcp-feature-overlays" :disabled="!props.isOpen">
    <Transition name="fade">
      <div v-if="props.isOpen" class="settings-view fixed inset-0 flex flex-col bg-secondary-bg text-primary-text pointer-events-auto">
        <!-- Header -->
          <header class="p-4 flex items-center justify-between border-b border-white/10 pt-[calc(var(--vcp-safe-top,24px)+20px)] pb-6 shrink-0">
            <h2 class="text-xl font-bold">全局设置</h2>
            <button @click="closeSettings" class="p-2.5 bg-white/10 rounded-full active:scale-90 transition-all flex items-center justify-center">
              <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>
            </button>
          </header>

          <!-- Scrollable Form Area -->
          <div v-if="loading" class="flex-1 flex items-center justify-center opacity-60 text-sm font-bold tracking-widest uppercase">
            正在加载设置...
          </div>
          <div v-else class="flex-1 overflow-y-auto p-5 space-y-8 pb-safe">
            <UserProfileSection :settings="settings" />

            <SyncSettingsSection :settings="settings" @save-request="saveSettings" @open-sync="openSyncView" />

            <VcpCoreSettingsSection :settings="settings" @save-request="saveSettings" />

            <AiLogicSettingsSection :settings="settings" />

            <TopicSummarySection :settings="settings" @open-model-selector="showSummaryModelSelector = true" />

            <MaintenanceSection />

          <section class="space-y-4">
              <div class="flex items-center gap-2 px-1">
                <div class="w-1 h-4 bg-orange-500 rounded-full"></div>
                <h3 class="text-xs font-black uppercase tracking-[0.2em] opacity-50 dark:opacity-40">视觉长廊</h3>
              </div>
              <ThemePicker />
            </section>

            <div class="h-4"></div>

            <!-- 保存按钮 -->
            <button @click="saveSettings"
                    class="w-full py-4.5 bg-blue-600 hover:bg-blue-500 text-white active:scale-95 transition-all rounded-[1.25rem] font-black uppercase tracking-widest text-xs shadow-xl shadow-blue-900/20">
                    保存并应用变更
            </button>

            <!-- 版本信息 -->
            <div class="text-center opacity-10 text-[9px] py-8 pb-12 font-mono uppercase tracking-widest">
              VCP MOBILE · PROJECT AVATAR<br/>INTERNAL RELEASE 2026.03.13
            </div>
          </div>

          <!-- 话题总结模型选择器 -->
          <ModelSelector
            :model-value="showSummaryModelSelector"
            @update:model-value="showSummaryModelSelector = $event"
            :current-model="settings.topicSummaryModel"
            title="选择总结专用模型"
            @select="onSummaryModelSelect"
          />
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.settings-view {
  background-color: color-mix(in srgb, var(--primary-bg) 85%, transparent);
  backdrop-filter: blur(30px) saturate(180%);
}

/* 确保子组件中的 card-modern 样式生效 */
:deep(.card-modern) {
  @apply bg-white/5 border border-white/10 rounded-[2rem] p-6 backdrop-blur-xl shadow-2xl;
}

:deep(input[type="number"]::-webkit-inner-spin-button),
:deep(input[type="number"]::-webkit-outer-spin-button) {
  -webkit-appearance: none;
  margin: 0;
}
</style>
