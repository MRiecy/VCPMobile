<script setup lang="ts">
import { ref, onMounted, watch, computed, defineAsyncComponent } from "vue";
import { useModalHistory } from "../../core/composables/useModalHistory";
import { useSettingsStore, type AppSettings } from "../../core/stores/settings";
import SlidePage from "../../components/ui/SlidePage.vue";

// 原子组件与高频子页面：静态 import，无需等待
import UserProfileSection from "./components/UserProfileSection.vue";
import SyncSettingsSection from "./components/SyncSettingsSection.vue";
import VcpCoreSettingsSection from "./components/VcpCoreSettingsSection.vue";
import ThemePicker from "./ThemePicker.vue";
import ModelSelector from "../../components/ModelSelector.vue";
import SettingsCard from "../../components/settings/SettingsCard.vue";
import SettingsRow from "../../components/settings/SettingsRow.vue";
import AboutSection from "./components/AboutSection.vue"; // 实测解析延迟明显，保持静态

// 低频子页面（advanced / power）：懒加载，用户点进子页面时才解析
const AssistantSettingsSection = defineAsyncComponent(() => import("./components/AssistantSettingsSection.vue"));
const TopicSummarySection = defineAsyncComponent(() => import("./components/TopicSummarySection.vue"));
const DistributedSettingsSection = defineAsyncComponent(() => import("../distributed/DistributedSettingsSection.vue"));
const MaintenanceSection = defineAsyncComponent(() => import("./components/MaintenanceSection.vue"));
const BatteryOptimizationGuide = defineAsyncComponent(() => import("./components/BatteryOptimizationGuide.vue"));


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
const { registerModal, unregisterModal } = useModalHistory();
const SUBPAGE_MODAL_ID = "SettingsSubPage";

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
  enableAssistant: false,
  assistantAgentId: "",
});

const loading = ref(true);
const showSummaryModelSelector = ref(false);
const currentSubPage = ref<string | null>(null);
const visibleSubPage = ref<string | null>(null);

const categories = [
  { id: "identity", title: "用户身份", description: "头像、用户名与管理员账号" },
  { id: "connection", title: "服务器连接", description: "VCP Server API 与数据同步" },
  { id: "theme", title: "主题切换", description: "视觉风格与壁纸" },
  { id: "advanced", title: "高级功能", description: "话题总结、分布式节点与数据维护" },
  { id: "power", title: "电池优化", description: "电池白名单与进程锁定设置" },
  { id: "about", title: "关于", description: "版本与更新" },
];

const subPageTitle = computed(() => {
  return categories.find((c) => c.id === currentSubPage.value)?.title || "";
});

const onSummaryModelSelect = (modelId: string) => {
  settings.value.topicSummaryModel = modelId;
  saveSettings();
};

const closeSettings = () => {
  currentSubPage.value = null;
  emit("close");
};

const goBack = async () => {
  try {
    await settingsStore.saveSettings(settings.value);
  } catch (e) {
    console.error("Failed to save settings:", e);
  }
  currentSubPage.value = null;
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
      currentSubPage.value = null; // 重新打开时，先确保重置回设置主页，防止滞留子页面
      loadSettings();
    } else {
      // 关闭时绝不重置，防止与外层 SlidePage 的退场 Transition 动画产生 DOM 突兀卸载冲突
    }
  },
);

// 纯展示型子页面：无可编辑内容，物理返回时跳过 saveSettings
const READ_ONLY_SUBPAGES = new Set(['about', 'power']);

watch(currentSubPage, (val) => {
  if (val) {
    visibleSubPage.value = val;
    registerModal(SUBPAGE_MODAL_ID, () => {
      if (READ_ONLY_SUBPAGES.has(currentSubPage.value ?? '')) {
        currentSubPage.value = null;
      } else {
        goBack();
      }
    });
  } else {
    unregisterModal(SUBPAGE_MODAL_ID);
    // 延迟清空子页面，防止与外层 SlidePage 的退场 Transition 动画（0.3s-0.35s）产生 DOM 突兀卸载冲突
    setTimeout(() => {
      if (!currentSubPage.value) {
        visibleSubPage.value = null;
      }
    }, 350);
  }
});
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex">
    <div
      class="settings-view flex flex-col h-full w-full bg-secondary-bg text-primary-text pointer-events-auto"
    >
      <!-- Header -->
      <header
        v-if="currentSubPage !== 'about'"
        class="px-4 py-3 flex items-center justify-between border-b border-white/10 pt-[calc(var(--vcp-safe-top,24px)+12px)] pb-3 shrink-0"
      >
        <h2 class="text-xl font-bold">{{ currentSubPage ? subPageTitle : '全局设置' }}</h2>
        <button
          @click="currentSubPage ? goBack() : closeSettings()"
          class="p-2 active:scale-90 transition-all flex items-center justify-center opacity-70 active:opacity-100"
        >
          <svg
            v-if="currentSubPage"
            width="22"
            height="22"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2.5"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <path d="m15 18-6-6 6-6"/>
          </svg>
          <svg
            v-else
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

      <!-- Scrollable Area -->
      <div
        v-if="loading"
        class="flex-1 flex items-center justify-center opacity-60 text-sm font-bold tracking-widest uppercase"
      >
        正在加载设置...
      </div>
      <div v-else class="flex-1 overflow-y-auto relative no-rubber-band">
        <!-- 主页 -->
        <div class="px-3 pt-8 pb-safe min-h-full flex flex-col">
          <SettingsCard>
            <div class="divide-y divide-black/5 dark:divide-white/5">
              <SettingsRow
                v-for="cat in categories"
                :key="cat.id"
                :title="cat.title"
                :description="cat.description"
                clickable
                @click="currentSubPage = cat.id"
              />
            </div>
          </SettingsCard>

          <!-- Footer -->
          <div class="mt-auto text-center opacity-15 text-[8px] py-8 pb-12 font-mono uppercase tracking-[0.25em] select-none">
            VCP MOBILE // PROJECT AVATAR // 2026.04.07
          </div>
        </div>

        <!-- 子页面层 -->
        <Transition name="slide-subpage">
          <div
            v-if="currentSubPage"
            class="absolute inset-0 flex flex-col z-10 transition-colors duration-300 bg-[var(--primary-bg)]"
          >

            <div 
              class="flex-1 pb-safe"
              :class="currentSubPage === 'about' ? 'overflow-hidden flex flex-col' : 'overflow-y-auto px-3 pt-6 pb-5 space-y-6 no-rubber-band'"
            >
              <!-- 用户身份 -->
              <template v-if="visibleSubPage === 'identity'">
                <UserProfileSection :settings="settings" />
              </template>

              <!-- 服务器连接 -->
              <template v-if="visibleSubPage === 'connection'">
                <div class="space-y-6">
                  <div>
                    <h3 class="text-[11px] font-black uppercase tracking-[0.15em] opacity-50 mb-3 px-1">核心连接</h3>
                    <SettingsCard>
                      <VcpCoreSettingsSection
                        :settings="settings"
                        @save-request="saveSettings"
                      />
                    </SettingsCard>
                  </div>
                  <div>
                    <h3 class="text-[11px] font-black uppercase tracking-[0.15em] opacity-50 mb-3 px-1">数据同步</h3>
                    <SettingsCard>
                      <SyncSettingsSection
                        :settings="settings"
                        @save-request="saveSettings"
                      />
                    </SettingsCard>
                  </div>
                </div>
              </template>

              <!-- 主题切换 -->
              <template v-if="visibleSubPage === 'theme'">
                <SettingsCard no-padding>
                  <div class="p-4">
                    <ThemePicker />
                  </div>
                </SettingsCard>
              </template>

              <!-- 高级功能 -->
              <template v-if="visibleSubPage === 'advanced'">
                <div class="space-y-6">
                  <div>
                    <h3 class="text-[11px] font-black uppercase tracking-[0.15em] opacity-50 mb-3 px-1">划词悬浮助手</h3>
                    <SettingsCard>
                      <AssistantSettingsSection :settings="settings" @save-request="saveSettings" />
                    </SettingsCard>
                  </div>
                  <div>
                    <h3 class="text-[11px] font-black uppercase tracking-[0.15em] opacity-50 mb-3 px-1">话题总结</h3>
                    <SettingsCard>
                      <TopicSummarySection
                        :settings="settings"
                        @open-model-selector="showSummaryModelSelector = true"
                        @save-request="saveSettings"
                      />
                    </SettingsCard>
                  </div>
                  <div>
                    <h3 class="text-[11px] font-black uppercase tracking-[0.15em] opacity-50 mb-3 px-1">分布式节点</h3>
                    <SettingsCard>
                      <DistributedSettingsSection
                        :settings="settings"
                        @save-request="saveSettings"
                      />
                    </SettingsCard>
                  </div>
                  <div>
                    <h3 class="text-[11px] font-black uppercase tracking-[0.15em] opacity-50 mb-3 px-1">数据维护</h3>
                    <SettingsCard>
                      <MaintenanceSection />
                    </SettingsCard>
                  </div>
                </div>
              </template>

              <!-- 后台保活 -->
              <template v-if="visibleSubPage === 'power'">
                <BatteryOptimizationGuide />
              </template>

              <!-- 关于 -->
              <template v-if="visibleSubPage === 'about'">
                <AboutSection @back="currentSubPage = null" />
              </template>
            </div>
          </div>
        </Transition>

        <ModelSelector
          :model-value="showSummaryModelSelector"
          @update:model-value="showSummaryModelSelector = $event"
          :current-model="settings.topicSummaryModel"
          title="选择总结专用模型"
          @select="onSummaryModelSelect"
        />
      </div>
    </div>
  </SlidePage>
</template>

<style scoped>
.settings-view {
  background-color: color-mix(in srgb, var(--primary-bg) 100%, transparent);
}

.slide-subpage-enter-active {
  transition: transform 0.35s cubic-bezier(0.32, 0.72, 0, 1);
}

.slide-subpage-leave-active {
  transition: transform 0.3s cubic-bezier(0.32, 0.72, 0, 1);
}

.slide-subpage-enter-from,
.slide-subpage-leave-to {
  transform: translateX(100%);
}
</style>