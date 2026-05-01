<script setup lang="ts">
import { ref, onMounted, watch } from "vue";
import { useSettingsStore, type AppSettings } from "../../core/stores/settings";
import SlidePage from "../../components/ui/SlidePage.vue";

import UserProfileSection from "./components/UserProfileSection.vue";
import SyncSettingsSection from "./components/SyncSettingsSection.vue";
import VcpCoreSettingsSection from "./components/VcpCoreSettingsSection.vue";
import TopicSummarySection from "./components/TopicSummarySection.vue";
import MaintenanceSection from "./components/MaintenanceSection.vue";
import UpdateSection from "./components/UpdateSection.vue";
import ThemePicker from "./ThemePicker.vue";
import ModelSelector from "../../components/ModelSelector.vue";
import DistributedSettingsSection from "../distributed/DistributedSettingsSection.vue";
import ToolInteractionOverlay from "../distributed/ToolInteractionOverlay.vue";
import SensorCollector from "../distributed/SensorCollector.vue";

// 原子组件
import SettingsSection from "../../components/settings/SettingsSection.vue";
import SettingsCard from "../../components/settings/SettingsCard.vue";
import SettingsActionButton from "../../components/settings/SettingsActionButton.vue";
import SettingsDisclosure from "../../components/settings/SettingsDisclosure.vue";

const props = withDefaults(
  defineProps<{
    isOpen?: boolean;
    zIndex?: number;
  }>(),
  {
    isOpen: false,
    zIndex: 50,
  },
);

const emit = defineEmits<{
  close: [];
}>();

const settingsStore = useSettingsStore();

const settings = ref<AppSettings>({
  userName: "User",
  vcpServerUrl: "",
  vcpApiKey: "",
  vcpLogUrl: "",
  vcpLogKey: "",
  syncServerUrl: "",
  syncHttpUrl: "",
  syncToken: "",
  adminUsername: "",
  adminPassword: "",
  fileKey: "",
  agentOrder: [],
  groupOrder: [],
  topicSummaryModel: "gemini-2.5-flash",
  syncLogLevel: "INFO",
});

const loading = ref(true);
const showSummaryModelSelector = ref(false);

const onSummaryModelSelect = (modelId: string) => {
  settings.value.topicSummaryModel = modelId;
};

const closeSettings = () => {
  emit("close");
};

const loadSettings = async () => {
  try {
    await settingsStore.fetchSettings();
    if (settingsStore.settings) {
      settings.value = JSON.parse(JSON.stringify(settingsStore.settings));
    }
  } catch (e) {
    console.error("[SettingsView] Failed to load settings:", e);
  } finally {
    loading.value = false;
  }
};

const saveSettings = async () => {
  try {
    await settingsStore.saveSettings(settings.value);
    console.log("Settings saved!");
  } catch (e) {
    console.error("Failed to save settings:", e);
  }
};

onMounted(() => {
  if (props.isOpen) loadSettings();
});

watch(
  () => props.isOpen,
  (val: boolean) => {
    if (val) {
      loadSettings();
    }
  },
);
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex">
    <div
      class="settings-view flex flex-col h-full w-full bg-secondary-bg text-primary-text pointer-events-auto"
    >
      <!-- Header -->
      <header
        class="p-4 flex items-center justify-between border-b border-white/10 pt-[calc(var(--vcp-safe-top,24px)+20px)] pb-6 shrink-0"
      >
        <h2 class="text-xl font-bold">全局设置</h2>
        <button
          @click="closeSettings"
          class="p-2.5 bg-white/10 rounded-full active:scale-90 transition-all flex items-center justify-center"
        >
          <svg
            width="22"
            height="22"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2.5"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </header>

      <!-- Scrollable Form Area -->
      <div
        v-if="loading"
        class="flex-1 flex items-center justify-center opacity-60 text-sm font-bold tracking-widest uppercase"
      >
        正在加载设置...
      </div>
      <div v-else class="flex-1 overflow-y-auto p-5 space-y-6 pb-safe">
        <!-- 用户档案 -->
        <UserProfileSection :settings="settings" />

        <!-- 核心连接 -->
        <SettingsDisclosure
          title="核心连接"
          description="VCP Server API 与 WebSocket 鉴权"
          :default-open="true"
          accent-color="bg-blue-500"
        >
          正在加载设置...
        </div>
        <div v-else class="flex-1 overflow-y-auto p-5 space-y-6 pb-safe">
          <!-- 用户档案 -->
          <UserProfileSection :settings="settings" />

          <!-- 核心连接 -->
          <SettingsDisclosure
            title="核心连接"
            description="VCP Server API 与 WebSocket 鉴权"
            :default-open="true"
            accent-color="bg-blue-500"
          >
            <VcpCoreSettingsSection
              :settings="settings"
              @save-request="saveSettings"
            />
          </SettingsDisclosure>

          <!-- 数据同步 -->
          <SettingsDisclosure
            title="数据同步"
            description="连接桌面端同步插件"
            accent-color="bg-green-500"
          >
            <SyncSettingsSection
              :settings="settings"
              @save-request="saveSettings"
              @open-sync="openSyncView"
            />
          </SettingsDisclosure>

          <!-- 话题总结 -->
          <SettingsDisclosure
            title="话题总结"
            description="配置总结专用模型"
            accent-color="bg-yellow-500"
          >
            <TopicSummarySection
              :settings="settings"
              @open-model-selector="showSummaryModelSelector = true"
            />
          </SettingsDisclosure>

          <!-- 分布式节点 -->
          <SettingsDisclosure
            title="分布式节点"
            description="作为移动端工具节点接入主服务器"
            accent-color="bg-purple-500"
          >
            <DistributedSettingsSection
              :settings="settings"
              @save-request="saveSettings"
            />
          </SettingsDisclosure>

          <!-- 视觉长廊 -->
          <SettingsSection title="视觉长廊" accent-color="bg-orange-500">
            <SettingsCard no-padding>
              <div class="p-4">
                <ThemePicker />
              </div>
            </SettingsCard>
          </SettingsSection>

          <!-- 数据维护 -->
          <SettingsSection title="数据维护" accent-color="bg-red-500">
            <SettingsCard>
              <MaintenanceSection />
            </SettingsCard>
          </SettingsSection>

          <div class="h-4"></div>

          <SettingsActionButton
            variant="primary"
            size="lg"
            full-width
            @click="saveSettings"
          >
            保存并应用变更
          </SettingsActionButton>

        <div
          class="text-center opacity-10 text-[9px] py-8 pb-12 font-mono uppercase tracking-widest"
        >
          VCP MOBILE · PROJECT AVATAR<br />INTERNAL RELEASE 2026.04.07
        </div>

        <ToolInteractionOverlay />
        <SensorCollector />

        <ModelSelector
          :model-value="showSummaryModelSelector"
          @update:model-value="showSummaryModelSelector = $event"
          :current-model="settings.topicSummaryModel"
          title="选择总结专用模型"
          @select="onSummaryModelSelect"
        />
      </div>

      <ModelSelector
        :model-value="showSummaryModelSelector"
        @update:model-value="showSummaryModelSelector = $event"
        :current-model="settings.topicSummaryModel"
        title="选择总结专用模型"
        @select="onSummaryModelSelect"
      />
    </div>
  </SlidePage>
</template>

<style scoped>
.settings-view {
  background-color: color-mix(in srgb, var(--primary-bg) 92%, transparent);
  backdrop-filter: blur(40px) saturate(180%);
}

@media (hover: none) and (pointer: coarse) {
  .settings-view {
    backdrop-filter: blur(4px) saturate(180%);
  }
}
</style>