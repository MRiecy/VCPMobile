<script setup lang="ts">
import { computed, ref, onUnmounted, watch } from "vue";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { deleteTempFile } from "tauri-plugin-vcp-mobile";
import { cancelFileOperation, pickFile } from "../../core/utils/nativeFilePicker";
import { useAssistantStore } from "../../core/stores/assistant";
import { useChatSessionStore } from "../../core/stores/chatSessionStore";
import { useNotificationStore } from "../../core/stores/notification";
import { useOverlayStore } from "../../core/stores/overlay";
import { SETTINGS_DEFAULTS } from "../../core/config/generated/defaults";
import { LAYER_PAGE_BASE } from "../../core/constants/layers";
import { useModalHistory } from "../../core/composables/useModalHistory";
import { useKeyboardInsets } from "../../core/composables/useKeyboardInsets";
import AgentVoiceSettings from "./AgentVoiceSettings.vue";
import { isAndroid, isIOS } from "../../core/utils/platform";
import SlidePage from "../../components/ui/SlidePage.vue";
import ModelSelector from "../../components/ModelSelector.vue";
import AvatarCropper from "../../components/ui/AvatarCropper.vue";
import VcpAvatar from "../../components/ui/VcpAvatar.vue";
import type { AgentConfigDto } from "../../core/types/assistant";
import { allowChatMutation, chatMutationBlockReason } from "../chat/chatMutationPolicy";

const props = withDefaults(defineProps<{
  id?: string;
  isOpen?: boolean;
  active?: boolean;
  zIndex?: number;
}>(), {
  isOpen: false,
  active: true,
  zIndex: LAYER_PAGE_BASE,
});

const emit = defineEmits(["close", "delete"]);

const assistantStore = useAssistantStore();
const sessionStore = useChatSessionStore();
const notificationStore = useNotificationStore();
const overlayStore = useOverlayStore();
const { registerModal } = useModalHistory();
const pageActive = computed(() => props.isOpen && props.active);
const formScroll = ref<HTMLElement | null>(null);
const { keyboardHeight } = useKeyboardInsets();
const revealFocusedField = () => {
  const scroller = formScroll.value;
  const field = document.activeElement;
  if (!pageActive.value || !scroller || !(field instanceof HTMLElement) || !scroller.contains(field)
    || !field.matches('input, textarea, [contenteditable="true"]')) return;
  const bounds = field.getBoundingClientRect();
  const viewport = scroller.getBoundingClientRect();
  // Move only this form's scroll position after its keyboard inset has been applied.
  if (bounds.top < viewport.top) scroller.scrollTop += bounds.top - viewport.top;
  else if (bounds.bottom > viewport.bottom) {
    scroller.scrollTop += Math.min(bounds.bottom - viewport.bottom, bounds.top - viewport.top);
  }
};
watch(keyboardHeight, height => { if (height > 0) revealFocusedField(); }, { flush: "post" });
const isLeaving = ref(false);
let mounted = true;
const createEmptyConfig = (id = ""): AgentConfigDto => ({
  id,
  name: "",
  systemPrompt: "",
  mobileSystemPrompt: "",
  model: "",
  temperature: 1.0,
  contextTokenLimit: 1000000,
  maxOutputTokens: 64000,
  streamOutput: true,
  useTemperature: false,
  avatarCalculatedColor: null,
  topics: [],
  voice: { enabled: true, ...SETTINGS_DEFAULTS.voice.tts },
});
const agentConfig = ref<AgentConfigDto>(createEmptyConfig(props.id));
const deleteBlockReason = computed(() => chatMutationBlockReason({
  ownerId: agentConfig.value.id, ownerType: "agent",
}));
let editorEpoch = 0;
const pendingSaves = new Map<string, Promise<boolean>>();
const retainedDrafts = new Map<string, { draft: AgentConfigDto; baseline: AgentConfigDto }>();
const isEpochCurrent = (epoch: number) => editorEpoch === epoch;
const isOpenEditorCurrent = (epoch: number, id: string) =>
  isEpochCurrent(epoch) && props.isOpen && props.id === id;

// Avatar Upload Logic
const isCropping = ref(false);
const isPickingAvatar = ref(false);
const cropImg = ref("");
const avatarWorkingPath = ref<string | null>(null);
let avatarOperationId: string | null = null;

const cancelAvatarPicker = () => {
  const operationId = avatarOperationId;
  avatarOperationId = null;
  if (operationId) void cancelFileOperation(operationId).catch(console.warn);
};

const releaseAvatarWorkingCopy = async () => {
  const filePath = avatarWorkingPath.value;
  avatarWorkingPath.value = null;
  cropImg.value = "";
  if (!filePath) return;
  try {
    await deleteTempFile(filePath);
  } catch (error) {
    console.warn("Failed to delete Agent avatar working copy:", error);
  }
};

const triggerFileInput = async () => {
  if (!pageActive.value || loading.value || isLeaving.value || !originalConfig.value
    || isPickingAvatar.value || isCropping.value || isSaving.value) return;
  const epoch = editorEpoch;
  const agentId = props.id || "";
  if (!agentId || !isOpenEditorCurrent(epoch, agentId)) return;
  if (!isAndroid && !isIOS) {
    notificationStore.addNotification({
      type: "info",
      title: "头像选图暂不可用",
      message: "当前预览环境不提供原生头像选图。",
      toastOnly: true,
    });
    return;
  }

  isPickingAvatar.value = true;
  const operationId = `avatar_${crypto.randomUUID()}`;
  avatarOperationId = operationId;
  try {
    await releaseAvatarWorkingCopy();
    if (avatarOperationId !== operationId || !isOpenEditorCurrent(epoch, agentId)) return;
    const picked = await pickFile("avatar", { operationId });
    if (avatarOperationId !== operationId || !isOpenEditorCurrent(epoch, agentId)) {
      await deleteTempFile(picked.path);
      return;
    }
    avatarWorkingPath.value = picked.path;
    cropImg.value = convertFileSrc(picked.path);
    isCropping.value = true;
  } catch (error) {
    if (!isOpenEditorCurrent(epoch, agentId)) return;
    if (!String(error).toLowerCase().includes("cancel")) {
      console.error("Failed to prepare Agent avatar image:", error);
      notificationStore.addNotification({
        type: "error",
        title: "头像读取失败",
        message: String(error) || "请选择其他图片后重试",
        toastOnly: true,
      });
    }
  } finally {
    if (avatarOperationId === operationId) avatarOperationId = null;
    if (isEpochCurrent(epoch)) isPickingAvatar.value = false;
  }
};

const cancelAvatarCrop = () => {
  isCropping.value = false;
  void releaseAvatarWorkingCopy();
};

const onCropConfirm = async (blob: Blob) => {
  const epoch = editorEpoch;
  const agentId = agentConfig.value.id;
  if (!agentId || !isOpenEditorCurrent(epoch, agentId)) {
    isCropping.value = false;
    await releaseAvatarWorkingCopy();
    return;
  }

  isCropping.value = false;
  isSaving.value = true;

  try {
    await releaseAvatarWorkingCopy();
    const arrayBuffer = await blob.arrayBuffer();
    if (!isOpenEditorCurrent(epoch, agentId)) return;
    const bytes = new Uint8Array(arrayBuffer);

    // Use assistantStore to save avatar and get notification
    await assistantStore.saveAvatar("agent", agentId, blob.type, bytes);

  } catch (err) {
    console.error("Failed to save avatar:", err);
  } finally {
    if (isEpochCurrent(epoch)) isSaving.value = false;
  }
};

const showModelSelector = ref(false);
const onModelSelect = (modelId: string) => {
  agentConfig.value.model = modelId;
};

const isSaving = ref(false);
const saveSuccess = ref(false);
let saveSuccessTimer: ReturnType<typeof setTimeout> | null = null;

// 原始配置快照，用于判断用户是否真正修改了内容
const originalConfig = ref<AgentConfigDto | null>(null);
const voiceSettings = ref<InstanceType<typeof AgentVoiceSettings> | null>(null);
const loading = ref(false);
const loadError = ref("");
const saveError = ref("");

onUnmounted(() => {
  mounted = false;
  void voiceSettings.value?.stop().catch(console.warn);
  cancelAvatarPicker();
  if (saveSuccessTimer) {
    clearTimeout(saveSuccessTimer);
    saveSuccessTimer = null;
  }
  void releaseAvatarWorkingCopy();
  const epoch = ++editorEpoch;
  void startSaveOnClose(epoch);
});

const loadConfig = async (epoch: number, agentId: string) => {
  if (agentId) {
    loading.value = true;
    loadError.value = "";
    try {
      const pendingSave = pendingSaves.get(agentId);
      if (pendingSave) {
        await pendingSave;
        if (!isOpenEditorCurrent(epoch, agentId)) return;
      }
      const retained = retainedDrafts.get(agentId);
      if (retained) {
        agentConfig.value = JSON.parse(JSON.stringify(retained.draft));
        originalConfig.value = JSON.parse(JSON.stringify(retained.baseline));
        retainedDrafts.delete(agentId);
        saveError.value = "上次保存失败，已保留未保存的修改。";
        return;
      }
      const config = await invoke<AgentConfigDto>("read_agent_config", {
        agentId,
        allowDefault: true,
      });
      if (!isOpenEditorCurrent(epoch, agentId)) return;
      agentConfig.value = config;
      originalConfig.value = JSON.parse(JSON.stringify(config));
    } catch (err) {
      console.error("Failed to load agent config:", err);
      if (isOpenEditorCurrent(epoch, agentId)) loadError.value = String(err);
    } finally {
      if (isOpenEditorCurrent(epoch, agentId)) loading.value = false;
    }
  }
};

const startSaveOnClose = (epoch: number): Promise<boolean> => {
  const agentId = agentConfig.value.id;
  const pending = pendingSaves.get(agentId);
  if (pending) return pending;
  const task = saveOnClose(epoch);
  if (agentId) pendingSaves.set(agentId, task);
  void task.finally(() => {
    if (pendingSaves.get(agentId) === task) pendingSaves.delete(agentId);
  });
  return task;
};

const saveOnClose = async (epoch: number): Promise<boolean> => {
  const draft = JSON.parse(JSON.stringify(agentConfig.value)) as AgentConfigDto;
  const baseline = originalConfig.value
    ? JSON.parse(JSON.stringify(originalConfig.value)) as AgentConfigDto
    : null;
  if (!draft.id) return true;
  const isCurrent = () => isEpochCurrent(epoch) && agentConfig.value.id === draft.id;

  // 仅在配置真正被修改时才触发保存，避免无意义的后端调用
  if (baseline && JSON.stringify(draft) !== JSON.stringify(baseline)) {
    if (isCurrent()) {
      isSaving.value = true;
      saveSuccess.value = false;
    }

    // 加固防重入：在 await 之前同步更新快照，拦截后续瞬时触发的并发保存调用
    if (isCurrent()) originalConfig.value = draft;

    try {
      const saved = await assistantStore.saveAgent(draft, baseline);
      retainedDrafts.delete(draft.id);
      if (!isCurrent()) return false;
      originalConfig.value = saved;
      agentConfig.value = JSON.parse(JSON.stringify(saved));
      saveError.value = "";
      saveSuccess.value = true;
      if (saveSuccessTimer) clearTimeout(saveSuccessTimer);
      saveSuccessTimer = setTimeout(() => {
        saveSuccess.value = false;
      }, 2000);
      return true;
    } catch (err: any) {
      retainedDrafts.set(draft.id, { draft, baseline });
      // 保存失败时回滚快照，以便后续有机会重新触发保存
      if (isCurrent()) {
        originalConfig.value = baseline;
        saveError.value = String(err) || "设置保存失败，未保存的修改已保留。";
      }
      console.error("Save config on close failed:", err);
      
      // 加固异常感知：通过 Toast 提示用户保存失败
      notificationStore.addNotification({
        type: "error",
        title: `${draft.name || draft.id} 设置保存失败`,
        message: err.toString() || "请重新打开设置后重试",
        toastOnly: true,
      });
      return false;
    } finally {
      if (isCurrent()) isSaving.value = false;
    }
  }
  return true;
};

const requestClose = (): boolean => {
  if (!mounted) return true;
  if (!pageActive.value || isLeaving.value || isSaving.value) return false;
  const epoch = editorEpoch;
  const agentId = props.id || "";
  isLeaving.value = true;
  cancelAvatarPicker();
  showModelSelector.value = false;
  void (async () => {
    try {
      await voiceSettings.value?.stop();
      if (!isOpenEditorCurrent(epoch, agentId) || !pageActive.value) return;
      const saved = await startSaveOnClose(epoch);
      if (saved && isOpenEditorCurrent(epoch, agentId) && pageActive.value) emit("close");
    } catch (error) {
      if (isOpenEditorCurrent(epoch, agentId)) saveError.value = `停止试听失败：${String(error)}`;
    } finally {
      if (isEpochCurrent(epoch)) isLeaving.value = false;
    }
  })();
  return false;
};

watch([pageActive, () => props.id], ([active, id]) => {
  if (active && id) registerModal(`Page:agentSettings:${id}`, requestClose, undefined, requestClose);
  else {
    showModelSelector.value = false;
    void voiceSettings.value?.stop().catch(console.warn);
  }
}, { immediate: true, flush: "sync" });

const reloadSavedConfig = async () => {
  if (isSaving.value || loading.value) return;
  const epoch = editorEpoch;
  const agentId = agentConfig.value.id;
  const confirmed = await overlayStore.showConfirm({
    title: "重新载入助手设置",
    message: "放弃当前未保存的修改，并读取已保存的设置？",
    isDanger: true,
  });
  if (!confirmed || !isOpenEditorCurrent(epoch, agentId)) return;
  retainedDrafts.delete(agentId);
  saveError.value = "";
  originalConfig.value = null;
  await loadConfig(epoch, agentId);
};

watch([() => props.isOpen, () => props.id], ([isOpen, id], [wasOpen, previousId]) => {
  void voiceSettings.value?.stop().catch(console.warn);
  cancelAvatarPicker();
  const epoch = ++editorEpoch;
  isPickingAvatar.value = false;
  isCropping.value = false;
  void releaseAvatarWorkingCopy();
  if (wasOpen && (previousId !== id || !isOpen)) void startSaveOnClose(epoch);
  if (isOpen && id) {
    agentConfig.value = createEmptyConfig(id);
    originalConfig.value = null;
    isSaving.value = false;
    isLeaving.value = false;
    saveSuccess.value = false;
    saveError.value = "";
    void loadConfig(epoch, id);
  }
}, { immediate: true });

const handleDelete = async () => {
  const epoch = editorEpoch;
  const agentId = agentConfig.value.id;
  if (!allowChatMutation({ ownerId: agentId, ownerType: "agent" })) return;
  const confirmed = await overlayStore.showConfirm({
    title: "删除 Agent",
    message: "确定要删除这个 Agent 吗？此操作不可撤销。",
    isDanger: true
  });
  if (confirmed && agentId && isOpenEditorCurrent(epoch, agentId)) {
    try {
      if (!await assistantStore.deleteAgent(agentId)) return;
      if (!isOpenEditorCurrent(epoch, agentId)) return;
      if (
        sessionStore.currentSelectedItem?.type === "agent" &&
        sessionStore.currentSelectedItem?.id === agentId
      ) {
        sessionStore.clearConversation();
      }
      // 删除成功后关闭页面不再把当前草稿自动保存到墓碑实体。
      originalConfig.value = JSON.parse(JSON.stringify(agentConfig.value));
      emit("close");
    } catch (err) {
      console.error("Failed to delete agent:", err);
    }
  }
};
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex" :inert="!pageActive" :aria-hidden="!pageActive">
    <div class="agent-settings-view flex flex-col h-full w-full bg-secondary-bg text-primary-text pointer-events-auto">
      <!-- Header -->
      <header
        class="p-3 flex items-center justify-between border-b border-black/10 dark:border-white/10 pt-[calc(var(--vcp-safe-top,24px)+10px)] pb-3 shrink-0 bg-black/5 dark:bg-white/5">
        <div class="flex items-center gap-2">
          <button @click="requestClose" :disabled="isLeaving || isSaving" aria-label="返回，关闭助手设置"
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
      <div ref="formScroll" class="agent-settings-scroll flex-1 min-h-0 overflow-y-auto p-5 space-y-6 pb-[calc(var(--vcp-safe-bottom,48px))] no-rubber-band"
        @focusin="revealFocusedField">
        <p v-if="loading" role="status" class="m-0 text-xs opacity-60">正在加载助手设置…</p>
        <p v-else-if="loadError" role="alert" class="m-0 text-xs text-red-500">
          无法读取助手设置：{{ loadError }}
          <button type="button" class="underline min-h-11 px-2" @click="loadConfig(editorEpoch, props.id || '')">重试</button>
        </p>
        <p v-if="saveError" role="alert" class="m-0 text-xs text-red-500">
          {{ saveError }}
          <button type="button" :disabled="isSaving" class="underline min-h-11 px-2" @click="startSaveOnClose(editorEpoch)">重试保存</button>
          <button type="button" :disabled="isSaving" class="underline min-h-11 px-2" @click="reloadSavedConfig">重新载入</button>
        </p>
        <fieldset :disabled="loading || isSaving || isLeaving || !originalConfig" class="m-0 p-0 border-0 min-w-0 space-y-6">
        <!-- 1. 基础身份 -->
        <section class="space-y-1.5">
          <div class="flex items-center gap-1.5 px-0.5">
            <span class="w-1.5 h-1.5 rounded-full bg-blue-500"></span>
            <h3 class="text-xs font-bold text-primary-text">基础身份</h3>
          </div>
          <div class="plate-container p-4">
            <div class="flex flex-col items-center gap-4">
              <div class="relative group cursor-pointer" @click="triggerFileInput">
                <VcpAvatar
                  owner-type="agent"
                  :owner-id="props.id || ''"
                  :fallback-name="agentConfig.name"
                  size="w-20 h-20"
                  rounded="rounded-full"
                  :dominant-color="agentConfig.avatarCalculatedColor"
                  class="border-2 border-dashed border-black/10 dark:border-white/20 shadow-inner group-active:scale-95 transition-all"
                />
                <div
                  class="absolute inset-0 bg-black/40 opacity-0 group-hover:opacity-100 rounded-full flex items-center justify-center transition-opacity cursor-pointer z-20">
                  <span class="text-[10px] text-white font-bold tracking-widest uppercase">更换头像</span>
                </div>
              </div>

              <div class="w-full">
                <label class="text-[11px] uppercase font-bold opacity-40 mb-1.5 block text-center">
                  Agent 名称
                </label>
                <input v-model="agentConfig.name" placeholder="为你的助手起个名字..."
                  class="bg-black/5 dark:bg-white/5 border border-black/10 dark:border-white/10 w-full rounded-lg focus:border-blue-500/50 outline-none py-2.5 px-4 text-center text-base font-bold transition-all text-primary-text" />
              </div>
            </div>
          </div>
        </section>

        <!-- 2. 系统提示词 -->
        <section class="space-y-1.5">
          <div class="flex items-center gap-1.5 px-0.5">
            <span class="w-1.5 h-1.5 rounded-full bg-blue-500"></span>
            <h3 class="text-xs font-bold text-primary-text">系统提示词</h3>
          </div>
          <div class="plate-container p-3 space-y-2">
            <textarea v-model="agentConfig.mobileSystemPrompt" placeholder="在这里输入移动端专用提示词..."
              class="w-full bg-black/5 dark:bg-white/5 border border-black/10 dark:border-white/10 rounded-lg p-3 text-sm outline-none min-h-[120px] resize-none focus:bg-black/10 transition-all leading-relaxed"></textarea>
            <div class="plate-note">
              此处编辑的提示词仅在本机生效。留空则使用默认提示词。支持 <code v-pre>{{AgentName}}</code> 占位符。
            </div>
          </div>
        </section>

        <!-- 3. 模型参数配置 -->
        <section class="space-y-1.5">
          <div class="flex items-center gap-1.5 px-0.5">
            <span class="w-1.5 h-1.5 rounded-full bg-blue-500"></span>
            <h3 class="text-xs font-bold text-primary-text">模型参数配置</h3>
          </div>

          <div class="plate-container p-4 space-y-5">
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

            <div :class="{ 'opacity-30 pointer-events-none': !agentConfig.useTemperature }" class="transition-opacity duration-200">
              <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block">Temperature (0-2):</label>
              <input type="number" v-model.number="agentConfig.temperature"
                min="0" max="2" step="0.1" :disabled="!agentConfig.useTemperature"
                class="w-full bg-black/5 dark:bg-white/5 rounded-xl px-4 py-3 text-sm outline-none font-mono" />
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

            <div class="flex justify-between items-center py-2">
              <span class="text-sm font-medium">发送温度参数</span>
              <label class="relative inline-flex items-center cursor-pointer">
                <input type="checkbox" v-model="agentConfig.useTemperature" class="sr-only peer" />
                <div
                  class="w-10 h-5 bg-black/10 dark:bg-white/10 rounded-full peer peer-checked:bg-blue-500 after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:after:translate-x-5">
                </div>
              </label>
            </div>
          </div>
        </section>

        <!-- 4. 语音设置 -->
        <AgentVoiceSettings v-if="originalConfig" :key="agentConfig.id" ref="voiceSettings" v-model="agentConfig.voice"
          :active="pageActive" :disabled="loading || isSaving || isLeaving" />

        </fieldset>

        <!-- Actions -->
        <div class="pt-4 pb-8">
          <button @click="handleDelete" :disabled="deleteBlockReason !== null"
            class="w-full py-3 bg-transparent border border-red-500/20 text-red-500/60 enabled:hover:bg-red-500/5 enabled:active:bg-red-500/10 disabled:opacity-40 disabled:cursor-not-allowed transition-colors rounded-xl font-bold uppercase tracking-widest text-[11px]">
            删除此 Agent
          </button>
          <p v-if="deleteBlockReason" class="mt-2 text-xs opacity-60">{{ deleteBlockReason }}</p>
        </div>
      </div>

      <!-- 模型选择器 (原有) -->
      <ModelSelector v-model="showModelSelector" :current-model="agentConfig.model" title="选择助手模型"
        @select="onModelSelect" />

    </div>
  </SlidePage>
  <!-- 头像裁剪器 (移出主视图以防被 Transition/v-if 干扰) -->
  <AvatarCropper v-if="isCropping" :img="cropImg" @cancel="cancelAvatarCrop" @confirm="onCropConfirm" />
</template>

<style scoped>
.agent-settings-view {
  background-color: var(--primary-bg);
  box-sizing: border-box;
  padding-bottom: var(--vcp-ime-offset, 0px);
  overflow: hidden;
}

.plate-container {
  @apply bg-black/[0.03] dark:bg-white/[0.035] border border-black/10 dark:border-white/10 rounded-lg overflow-hidden;
}

.plate-note {
  @apply border-l-2 border-black/20 dark:border-white/20 pl-2 text-[11px] leading-relaxed opacity-60;
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
