<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useAssistantStore } from "../../core/stores/assistant";
import { useChatSessionStore } from "../../core/stores/chatSessionStore";
import SlidePage from "../../components/ui/SlidePage.vue";
import ModelSelector from "../../components/ModelSelector.vue";
import AvatarCropper from "../../components/ui/AvatarCropper.vue";
import VcpAvatar from "../../components/ui/VcpAvatar.vue";
import SettingsSection from "../../components/settings/SettingsSection.vue";
import SettingsRow from "../../components/settings/SettingsRow.vue";
import SettingsSwitch from "../../components/settings/SettingsSwitch.vue";

interface GroupConfig {
  id: string;
  name: string;
  avatar?: string;
  avatarCalculatedColor?: string;
  members: string[];
  mode: string;
  memberTags: Record<string, string>;
  groupPrompt: string;
  invitePrompt: string;
  useUnifiedModel: boolean;
  unifiedModel?: string;
  tagMatchMode: string;
}

interface Agent {
  id: string;
  name: string;
  avatar?: string;
}

const props = withDefaults(defineProps<{
  id: string;
  isOpen?: boolean;
  zIndex?: number;
}>(), {
  isOpen: false,
  zIndex: 50,
});

const emit = defineEmits(["close"]);

const assistantStore = useAssistantStore();
const sessionStore = useChatSessionStore();

const groupConfig = ref<GroupConfig>({
  id: props.id,
  name: "",
  avatar: "",
  members: [],
  mode: "sequential",
  memberTags: {},
  groupPrompt: "",
  invitePrompt: "",
  useUnifiedModel: false,
  unifiedModel: "",
  tagMatchMode: "strict",
});

const allAgents = ref<Agent[]>([]);
const isSaving = ref(false);
const saveSuccess = ref(false);
let saveTimeout: ReturnType<typeof setTimeout> | null = null;
let isFirstLoad = true;

onUnmounted(() => {
  if (saveTimeout) {
    clearTimeout(saveTimeout);
    saveTimeout = null;
  }
});

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
  if (!groupConfig.value.id) return;

  isCropping.value = false;
  isSaving.value = true;

  try {
    const arrayBuffer = await blob.arrayBuffer();
    const bytes = new Uint8Array(arrayBuffer);

    // Use assistantStore to save avatar and get notification
    await assistantStore.saveAvatar("group", groupConfig.value.id, blob.type, Array.from(bytes));

    // Update UI local state via version
    avatarVersion.value = Date.now();
  } catch (err) {
    console.error("Failed to save avatar:", err);
  } finally {
    isSaving.value = false;
  }
};

const fetchAgents = async () => {
  try {
    const agents = await invoke<any[]>("get_agents");
    allAgents.value = agents.map(a => ({
      id: a.id,
      name: a.name,
      avatar: a.avatar
    }));
  } catch (err) {
    console.error("Failed to fetch agents:", err);
  }
};

const fetchGroupConfig = async () => {
  if (!props.id) return;
  try {
    const config = await invoke<any>("read_group_config", { groupId: props.id });
    const memberTags = config.memberTags || {};

    groupConfig.value = {
      ...config,
      memberTags: typeof memberTags === 'object' ? memberTags : {}
    };
    isFirstLoad = true;
  } catch (err) {
    console.error("Failed to load group config:", err);
  }
};

const autoSave = async () => {
  if (!groupConfig.value.id || !props.isOpen) return;

  isSaving.value = true;
  saveSuccess.value = false;

  try {
    // Use assistantStore to save group config and get notification
    await assistantStore.saveGroup(groupConfig.value);
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
  groupConfig,
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
    }, 1000);
  },
  { deep: true },
);

watch(() => props.isOpen, (val) => {
  if (val) {
    fetchAgents();
    fetchGroupConfig();
  }
});

const toggleMember = (agentId: string) => {
  const index = groupConfig.value.members.indexOf(agentId);
  if (index === -1) {
    groupConfig.value.members.push(agentId);
    if (!groupConfig.value.memberTags[agentId]) {
      const agent = allAgents.value.find(a => a.id === agentId);
      groupConfig.value.memberTags[agentId] = agent?.name || agentId;
    }
  } else {
    groupConfig.value.members.splice(index, 1);
  }
};

const isMember = (agentId: string) => groupConfig.value.members.includes(agentId);

const showModelSelector = ref(false);
const onModelSelect = (modelId: string) => {
  groupConfig.value.unifiedModel = modelId;
};

const handleDelete = async () => {
  if (confirm("确定要删除这个群组吗？所有聊天记录将被标记为删除。")) {
    try {
      await assistantStore.deleteGroup(props.id);
      if (sessionStore.currentSelectedItem?.id === props.id) {
        sessionStore.currentSelectedItem = null;
      }
      emit("close");
    } catch (err) {
      alert("删除失败: " + err);
    }
  }
};

onMounted(async () => {
  if (props.isOpen) {
    await fetchAgents();
    await fetchGroupConfig();
  }
});

const modeOptions = [
  { value: "sequential", label: "顺序发言", desc: "成员按预定顺序轮流发言" },
  { value: "naturerandom", label: "自然随机", desc: "基于标签和 @提及智能选择发言者" },
  { value: "invite_only", label: "邀请发言", desc: "由用户手动点击或提示词邀约" },
];

const tagModeOptions = [
  { value: "strict", label: "严格模式", desc: "原始行为，严格匹配标签" },
  { value: "natural", label: "自然模式", desc: "区分 Tag 来源，避免循环触发" },
];
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex">
    <div class="group-settings-view flex flex-col h-full w-full bg-secondary-bg text-primary-text pointer-events-auto">
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
          <h2 class="text-base font-bold">群组设置</h2>
        </div>
        <div class="text-xs font-bold transition-opacity duration-300" :class="{
          'opacity-100': isSaving || saveSuccess,
          'opacity-0': !isSaving && !saveSuccess,
        }">
          <span v-if="isSaving" class="text-blue-400 animate-pulse">保存中...</span>
          <span v-else-if="saveSuccess" class="text-green-500">已保存 ✅</span>
        </div>
      </header>

      <div class="flex-1 overflow-y-auto p-5 space-y-8 pb-safe">
        <!-- 1. Basic Info -->
        <section class="flex flex-col items-center gap-6 py-2">
          <div class="relative group" @click="triggerFileInput">
            <VcpAvatar
              owner-type="group"
              :owner-id="props.id"
              :version="avatarVersion"
              :fallback-name="groupConfig.name"
              size="w-24 h-24"
              rounded="rounded-full"
              outer-border
              :dominant-color="groupConfig.avatarCalculatedColor"
              class="border-2 border-dashed border-black/10 dark:border-white/20 shadow-inner group-active:scale-95 transition-all"
            />
            <div
              class="absolute inset-0 bg-black/40 opacity-0 group-hover:opacity-100 rounded-full flex items-center justify-center transition-opacity cursor-pointer z-20">
              <span class="text-[10px] text-white font-bold tracking-widest uppercase">更换头像</span>
            </div>
            <input type="file" ref="fileInput" class="hidden" accept="image/*" @change="handleFileChange" />
          </div>

          <div class="w-full">
            <label class="text-[11px] uppercase font-black tracking-widest opacity-40 mb-2 block text-center">群组名称</label>
            <input v-model="groupConfig.name" placeholder="设置群组名称..."
              class="bg-black/5 dark:bg-white/5 border border-black/5 dark:border-white/10 w-full rounded-2xl focus:border-blue-500/50 outline-none py-3.5 px-4 text-center text-lg font-bold transition-all text-primary-text shadow-sm" />
          </div>
        </section>

        <!-- 2. Members Section -->
        <SettingsSection title="群组成员" description="勾选要加入群组的助手，并设置其触发标签">
          <div class="card-modern overflow-hidden !p-0">
            <div v-for="agent in allAgents" :key="agent.id"
              class="flex items-center gap-3 p-3 border-b border-black/5 dark:border-white/5 last:border-0 active:bg-black/5 dark:active:bg-white/5 transition-all">
              <input type="checkbox" :checked="isMember(agent.id)" @change="toggleMember(agent.id)"
                class="w-5 h-5 rounded-md accent-blue-500 cursor-pointer" />

              <VcpAvatar
                owner-type="agent"
                :owner-id="agent.id"
                :fallback-name="agent.name"
                size="w-10 h-10"
                rounded="rounded-full"
                outer-border
                dominant-color="var(--primary)"
              />

              <div class="flex-1 min-w-0">
                <div class="text-sm font-bold truncate">{{ agent.name }}</div>
                <div v-if="isMember(agent.id)" class="mt-1">
                  <input v-model="groupConfig.memberTags[agent.id]" placeholder="设置触发标签..."
                    class="w-full bg-black/5 dark:bg-white/5 rounded-lg px-2 py-1.5 text-[11px] outline-none border border-transparent focus:border-blue-500/30 transition-all font-mono" />
                </div>
              </div>
            </div>
          </div>
        </SettingsSection>

        <!-- 3. Chat Modes -->
        <SettingsSection title="群聊模式" accent-color="bg-purple-500">
          <div class="card-modern space-y-4">
            <div>
              <label class="text-[10px] uppercase font-bold opacity-40 mb-3 block">发言逻辑 (Mode)</label>
              <div class="grid grid-cols-1 gap-2">
                <button v-for="opt in modeOptions" :key="opt.value" @click="groupConfig.mode = opt.value"
                  class="flex flex-col p-3 rounded-xl border transition-all text-left" :class="groupConfig.mode === opt.value
                    ? 'bg-blue-500/10 border-blue-500/30 text-blue-500'
                    : 'bg-black/5 dark:bg-white/5 border-transparent opacity-60'">
                  <span class="text-sm font-bold">{{ opt.label }}</span>
                  <span class="text-[10px] mt-0.5 opacity-60">{{ opt.desc }}</span>
                </button>
              </div>
            </div>

            <div class="pt-4 border-t border-black/5 dark:border-white/5">
              <label class="text-[10px] uppercase font-bold opacity-40 mb-3 block">Tag 匹配模式</label>
              <div class="flex gap-2">
                <button v-for="opt in tagModeOptions" :key="opt.value" @click="groupConfig.tagMatchMode = opt.value"
                  class="flex-1 py-2.5 rounded-xl border transition-all text-[12px] font-bold" :class="groupConfig.tagMatchMode === opt.value
                    ? 'bg-purple-500/10 border-purple-500/30 text-purple-500'
                    : 'bg-black/5 dark:bg-white/5 border-transparent opacity-60'">
                  {{ opt.label }}
                </button>
              </div>
              <p class="mt-2 text-[9px] opacity-30 leading-tight">
                自然模式会区分 Tag 来源，尽量避免 Agent 因引用自身历史发言而重复触发。
              </p>
            </div>
          </div>
        </SettingsSection>

        <!-- 4. Model Settings -->
        <SettingsSection title="模型设置" accent-color="bg-orange-500">
          <div class="card-modern">
            <SettingsRow title="启用群组统一模型" description="所有成员强制使用同一模型，忽略其各自配置">
              <template #action>
                <SettingsSwitch v-model="groupConfig.useUnifiedModel" />
              </template>
            </SettingsRow>

            <div v-if="groupConfig.useUnifiedModel" class="mt-4 pt-4 border-t border-black/5 dark:border-white/5">
              <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block">选择群组统一模型</label>
              <div class="flex gap-2">
                <input v-model="groupConfig.unifiedModel" readonly @click="showModelSelector = true"
                  class="flex-1 bg-black/5 dark:bg-white/5 rounded-xl px-4 py-3 text-sm outline-none font-mono cursor-pointer" />
                <button @click="showModelSelector = true"
                  class="w-12 h-12 bg-orange-500/10 text-orange-500 rounded-xl flex items-center justify-center active:scale-90 transition-all">
                  <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                    stroke-linecap="round" stroke-linejoin="round">
                    <path d="M8.25 15L12 18.75 15.75 15m-7.5-6L12 5.25 15.75 9"></path>
                  </svg>
                </button>
              </div>
            </div>
          </div>
        </SettingsSection>

        <!-- 5. Prompts -->
        <SettingsSection title="提示词设置" accent-color="bg-green-500">
          <div class="space-y-4">
            <div class="card-modern">
              <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block">群组提示词 (Group Prompt)</label>
              <textarea v-model="groupConfig.groupPrompt" placeholder="在这里输入群组的全局背景信息..."
                class="w-full bg-black/5 dark:bg-white/5 rounded-xl p-3 text-sm outline-none min-h-[100px] resize-none focus:bg-black/10 transition-all leading-relaxed"></textarea>
            </div>

            <div class="card-modern">
              <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block">邀请提示词 (Invite Prompt)</label>
              <textarea v-model="groupConfig.invitePrompt" placeholder="当轮到某位助手发言时的提示语..."
                class="w-full bg-black/5 dark:bg-white/5 rounded-xl p-3 text-xs outline-none min-h-[80px] resize-none focus:bg-black/10 transition-all leading-relaxed font-mono"></textarea>
              <p v-pre class="mt-2 text-[9px] opacity-30">使用 {{VCPChatAgentName}} 作为占位符。</p>
            </div>
          </div>
        </SettingsSection>

        <!-- Actions -->
        <div class="pt-4 pb-8">
          <button @click="handleDelete"
            class="w-full py-3.5 bg-transparent border border-red-500/20 text-red-500/60 hover:bg-red-500/5 active:bg-red-500/10 active:scale-95 transition-all rounded-2xl font-black uppercase tracking-widest text-[11px]">
            删除此群组
          </button>
        </div>
      </div>

      <ModelSelector v-model="showModelSelector" :current-model="groupConfig.unifiedModel" title="选择群组统一模型"
        @select="onModelSelect" />
    </div>
  </SlidePage>
  <!-- 头像裁剪器 -->
  <AvatarCropper v-if="isCropping" :img="cropImg" @cancel="isCropping = false" @confirm="onCropConfirm" />
</template>

<style scoped>
.group-settings-view {
  background-color: color-mix(in srgb, var(--primary-bg) 85%, transparent);
  backdrop-filter: blur(20px) saturate(180%);
}

.card-modern {
  @apply bg-black/5 dark:bg-white/5 border border-black/5 dark:border-white/10 rounded-2xl p-4 shadow-sm;
}

textarea::-webkit-scrollbar {
  display: none;
}
</style>