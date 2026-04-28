<script setup lang="ts">
import { ref, onMounted, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useAssistantStore } from "../../core/stores/assistant";
import { useChatManagerStore } from "../../core/stores/chatManager";
import SlidePage from "../../components/ui/SlidePage.vue";
import ModelSelector from "../../components/ModelSelector.vue";
import AvatarCropper from "../../components/ui/AvatarCropper.vue";
import VcpAvatar from "../../components/ui/VcpAvatar.vue";

interface AgentConfig {
  id: string;
  name: string;
  avatar?: string;
  avatarCalculatedColor?: string;
  // Prompt settings
  systemPrompt: string;
  // Model settings
  model: string;
  temperature: number;
  contextTokenLimit: number;
  maxOutputTokens: number;
  streamOutput: boolean;
}

const props = withDefaults(defineProps<{
  id?: string;
  isOpen?: boolean;
  zIndex?: number;
}>(), {
  isOpen: false,
  zIndex: 50,
});

const emit = defineEmits(["close", "delete"]);

const assistantStore = useAssistantStore();
const chatStore = useChatManagerStore();

const agentConfig = ref<AgentConfig>({
  id: props.id || "",
  name: "",
  avatar: "",
  systemPrompt: "",
  model: "gemini-3-flash-preview",
  temperature: 1.0,
  contextTokenLimit: 1000000,
  maxOutputTokens: 32000,
  streamOutput: true,
});

// UI State
const sections = ref({
  params: false,
});

const toggleSection = (section: keyof typeof sections.value) => {
  sections.value[section] = !sections.value[section];
};

// Avatar Upload Logic
const fileInput = ref<HTMLInputElement | null>(null);
const isCropping = ref(false);
const cropImg = ref("");
const avatarVersion = ref(0);

const triggerFileInput = () => {
  fileInput.value?.click();
};

const handleFileChange = (e: Event) => {
  const file = (e.target as HTMLInputElement).files?.[0];
  if (!file) return;

  const reader = new FileReader();
  reader.onload = (event) => {
    cropImg.value = event.target?.result as string;
    isCropping.value = true;
  };
  reader.readAsDataURL(file);
};

const onCropConfirm = async (blob: Blob) => {
  if (!agentConfig.value.id) return;

  isCropping.value = false;
  isSaving.value = true;

  try {
    const arrayBuffer = await blob.arrayBuffer();
    const bytes = new Uint8Array(arrayBuffer);

    // Use assistantStore to save avatar and get notification
    await assistantStore.saveAvatar("agent", agentConfig.value.id, blob.type, Array.from(bytes));

    // Update UI local state via version
    avatarVersion.value = Date.now();
  } catch (err) {
    console.error("Failed to save avatar:", err);
  } finally {
    isSaving.value = false;
  }
};

const showModelSelector = ref(false);
const onModelSelect = (modelId: string) => {
  agentConfig.value.model = modelId;
};

const isSaving = ref(false);
const saveSuccess = ref(false);
let saveTimeout: ReturnType<typeof setTimeout> | null = null;
let isFirstLoad = true;

const loadConfig = async () => {
  if (props.id) {
    try {
      const config = await invoke<AgentConfig>("read_agent_config", {
        agentId: props.id,
        allowDefault: true,
      });
      agentConfig.value = config;
      isFirstLoad = true; // Reset first load for autoSave watcher
    } catch (err) {
      console.error("Failed to load agent config:", err);
    }
  }
};

const autoSave = async () => {
  if (!agentConfig.value.id || !props.isOpen) return;

  isSaving.value = true;
  saveSuccess.value = false;

  try {
    // Use assistantStore to save config and get notification
    await assistantStore.saveAgent(agentConfig.value);
    saveSuccess.value = true;
    setTimeout(() => {
      saveSuccess.value = false;
    }, 2000);
  } catch (err) {
    console.error("Auto save failed:", err);
  } finally {
    isSaving.value = false;
  }
};

watch(
  agentConfig,
  () => {
    if (isFirstLoad) {
      isFirstLoad = false;
      return;
    }
    if (saveTimeout) {
      clearTimeout(saveTimeout);
    }
    saveTimeout = setTimeout(() => {
      autoSave();
    }, 800);
  },
  { deep: true },
);

watch(() => props.isOpen, (val) => {
  if (val) loadConfig();
});

const handleDelete = async () => {
  if (confirm("确定要删除这个 Agent 吗？此操作不可撤销。")) {
    try {
      await assistantStore.deleteAgent(agentConfig.value.id);
      if (chatStore.currentSelectedItem?.id === agentConfig.value.id) {
        chatStore.currentSelectedItem = null;
      }
      emit("close");
    } catch (err) {
      console.error("Failed to delete agent:", err);
    }
  }
};

onMounted(async () => {
  if (props.isOpen) loadConfig();
});
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex">
    <div class="agent-settings-view flex flex-col h-full w-full bg-secondary-bg text-primary-text pointer-events-auto">
      <!-- Header -->
      <header
        class="p-3 flex items-center justify-between border-b border-black/10 dark:border-white/10 pt-[calc(var(--vcp-safe-top,24px)+10px)] pb-3 shrink-0 bg-black/5 dark:bg-white/5">
        <div class="flex items-center gap-2">
          <button @click="emit('close')"
            class="p-2 hover:bg-black/5 dark:hover:bg-white/10 rounded-lg active:scale-95 transition-all">
            <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
              stroke-linecap="round" stroke-linejoin="round">
              <line x1="19" y1="12" x2="5" y2="12"></line>
              <polyline points="12 19 5 12 12 5"></polyline>
            </svg>
          </button>
          <h2 class="text-base font-bold">助手设置</h2>
        </div>
        <div class="text-xs font-bold transition-opacity duration-300" :class="{
          'opacity-100': isSaving || saveSuccess,
          'opacity-0': !isSaving && !saveSuccess,
        }">
          <span v-if="isSaving" class="text-blue-400 animate-pulse">保存中...</span>
          <span v-else-if="saveSuccess" class="text-green-500">已自动保存 ✅</span>
        </div>
      </header>

      <!-- Scrollable Form Area -->
      <div class="flex-1 overflow-y-auto p-5 space-y-6 pb-safe">
        <!-- 1. Identity Section -->
        <section class="card-modern">
          <div class="flex flex-col items-center gap-6">
            <div class="relative group" @click="triggerFileInput">
              <VcpAvatar
                owner-type="agent"
                :owner-id="props.id || ''"
                :version="avatarVersion"
                :fallback-name="agentConfig.name"
                size="w-24 h-24"
                rounded="rounded-full"
                outer-border
                :dominant-color="agentConfig.avatarCalculatedColor"
                class="border-2 border-dashed border-black/10 dark:border-white/20 shadow-inner group-active:scale-95 transition-all"
              />
              <div
                class="absolute inset-0 bg-black/40 opacity-0 group-hover:opacity-100 rounded-full flex items-center justify-center transition-opacity cursor-pointer z-20">
                <span class="text-[10px] text-white font-bold tracking-widest uppercase">更换头像</span>
              </div>
              <input type="file" ref="fileInput" class="hidden" accept="image/*" @change="handleFileChange" />
            </div>

            <div class="w-full">
              <label
                class="text-[11px] uppercase font-black tracking-widest opacity-40 dark:opacity-30 mb-2 block text-center">Agent
                名称</label>
              <input v-model="agentConfig.name" placeholder="为你的助手起个名字..."
                class="bg-black/5 dark:bg-white/5 border border-black/5 dark:border-white/10 w-full rounded-2xl focus:border-blue-500/50 outline-none py-3.5 px-4 text-center text-lg font-bold transition-all text-primary-text" />
            </div>
          </div>
        </section>

        <!-- 2. System Prompt Section -->
        <section class="space-y-3">
          <div class="flex items-center gap-2 px-2 py-1">
            <div class="w-1 h-4 bg-purple-500 rounded-full"></div>
            <h3 class="text-xs font-black uppercase tracking-[0.2em] opacity-50">
              系统提示词 (System Prompt)
            </h3>
          </div>
          <div class="card-modern">
            <textarea v-model="agentConfig.systemPrompt" placeholder="在这里输入助手的核心指令..."
              class="w-full bg-black/5 dark:bg-white/5 rounded-2xl p-4 text-sm outline-none min-h-[150px] resize-none focus:bg-black/10 transition-all leading-relaxed"></textarea>
            <p class="mt-3 text-[10px] opacity-30 px-1 leading-normal">
              提示：系统提示词定义了助手的性格、知识范围和行为准则。
            </p>
          </div>
        </section>

        <!-- 3. Model Parameters (Collapsible) -->
        <section class="space-y-3">
          <button @click="toggleSection('params')" class="w-full flex items-center justify-between px-2 py-1">
            <div class="flex items-center gap-2">
              <div class="w-1 h-4 bg-blue-500 rounded-full"></div>
              <h3 class="text-xs font-black uppercase tracking-[0.2em] opacity-50">
                模型参数配置
              </h3>
            </div>
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3"
              stroke-linecap="round" stroke-linejoin="round" class="transition-transform duration-300"
              :class="{ 'rotate-180': sections.params }">
              <polyline points="6 9 12 15 18 9"></polyline>
            </svg>
          </button>

          <div v-if="sections.params" class="card-modern space-y-5 animate-in fade-in slide-in-from-top-2 duration-300">
            <div>
              <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block">模型名称</label>
              <div class="flex gap-2">
                <input v-model="agentConfig.model"
                  class="flex-1 bg-black/5 dark:bg-white/5 rounded-xl px-4 py-3 text-sm outline-none focus:bg-black/10 transition-all font-mono" />
                <button @click="showModelSelector = true"
                  class="w-12 h-12 bg-blue-500/10 text-blue-500 rounded-xl flex-center active:scale-90 transition-all">
                  <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                    stroke-linecap="round" stroke-linejoin="round">
                    <path d="M8.25 15L12 18.75 15.75 15m-7.5-6L12 5.25 15.75 9"></path>
                  </svg>
                </button>
              </div>
            </div>

            <div class="grid grid-cols-2 gap-5">
              <div>
                <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block">上下文 Token 上限</label>
                <input type="number" v-model.number="agentConfig.contextTokenLimit"
                  class="w-full bg-black/5 dark:bg-white/5 rounded-xl px-4 py-3 text-sm outline-none font-mono" />
              </div>
              <div>
                <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block">最大输出 Token</label>
                <input type="number" v-model.number="agentConfig.maxOutputTokens"
                  class="w-full bg-black/5 dark:bg-white/5 rounded-xl px-4 py-3 text-sm outline-none font-mono" />
              </div>
            </div>

            <div class="flex justify-between items-center py-2">
              <span class="text-sm font-medium">流式输出</span>
              <label class="relative inline-flex items-center cursor-pointer">
                <input type="checkbox" v-model="agentConfig.streamOutput" class="sr-only peer" />
                <div
                  class="w-10 h-5 bg-black/10 dark:bg-white/10 rounded-full peer peer-checked:bg-blue-500 after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:after:translate-x-5">
                </div>
              </label>
            </div>
          </div>
        </section>

        <div class="h-4"></div>

        <!-- Actions -->
        <div class="space-y-4">
          <button @click="handleDelete"
            class="w-full py-3 bg-transparent border border-red-500/20 text-red-500/60 hover:bg-red-500/5 active:bg-red-500/10 active:scale-95 transition-all rounded-xl font-bold uppercase tracking-widest text-[11px]">
            删除此 Agent
          </button>
        </div>
      </div>

      <!-- 模型选择器 -->
      <ModelSelector v-model="showModelSelector" :current-model="agentConfig.model" title="选择助手模型"
        @select="onModelSelect" />
    </div>
  </SlidePage>
  <!-- 头像裁剪器 (移出主视图以防被 Transition/v-if 干扰) -->
  <AvatarCropper v-if="isCropping" :img="cropImg" @cancel="isCropping = false" @confirm="onCropConfirm" />
</template>

<style scoped>
.agent-settings-view {
  background-color: color-mix(in srgb, var(--primary-bg) 85%, transparent);
  backdrop-filter: blur(20px) saturate(180%);
}

.card-modern {
  @apply bg-black/5 dark:bg-white/5 border border-black/5 dark:border-white/10 rounded-xl p-4 shadow-sm;
}

input[type="number"]::-webkit-inner-spin-button,
input[type="number"]::-webkit-outer-spin-button {
  -webkit-appearance: none;
  margin: 0;
}

.flex-center {
  @apply flex items-center justify-center;
}
</style>