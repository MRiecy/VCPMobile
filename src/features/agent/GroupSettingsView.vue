<script setup lang="ts">
defineOptions({ inheritAttrs: false });
import { computed, ref, onUnmounted, watch } from "vue";
import { ArrowDown, ArrowLeft, ArrowUp, HelpCircle } from "lucide-vue-next";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { deleteTempFile } from "tauri-plugin-vcp-mobile";
import { cancelFileOperation, pickFile } from "../../core/utils/nativeFilePicker";
import { useAssistantStore } from "../../core/stores/assistant";
import { useChatSessionStore } from "../../core/stores/chatSessionStore";
import { useNotificationStore } from "../../core/stores/notification";
import { useOverlayStore } from "../../core/stores/overlay";
import { useModalHistory } from "../../core/composables/useModalHistory";
import { LAYER_PAGE_BASE } from "../../core/constants/layers";
import { isAndroid, isIOS } from "../../core/utils/platform";
import SlidePage from "../../components/ui/SlidePage.vue";
import ModelSelector from "../../components/ModelSelector.vue";
import AvatarCropper from "../../components/ui/AvatarCropper.vue";
import VcpAvatar from "../../components/ui/VcpAvatar.vue";
import SettingsSection from "../../components/settings/SettingsSection.vue";
import SettingsCanvas from "../../components/settings/SettingsCanvas.vue";
import SettingsDisclosure from "../../components/settings/SettingsDisclosure.vue";
import SettingsTextField from "../../components/settings/SettingsTextField.vue";
import SettingsRow from "../../components/settings/SettingsRow.vue";
import SettingsSwitch from "../../components/settings/SettingsSwitch.vue";
import GroupModeHelpDialog from "./GroupModeHelpDialog.vue";
import GroupSettingsTextArea from "./GroupSettingsTextArea.vue";
import { allowChatMutation, chatMutationBlockReason } from "../chat/chatMutationPolicy";
import type {
  AgentListItemDto,
  GroupConfigDto,
  GroupMode,
} from "../../core/types/assistant";
import type { ContextMenuConfig, OverlayActionItem } from "../../core/types/overlay";

type EditableGroupConfig = Omit<
  GroupConfigDto,
  "memberTags" | "groupPrompt" | "invitePrompt" | "unifiedModel" | "tagMatchMode"
> & {
  memberTags: Record<string, string>;
  groupPrompt: string;
  invitePrompt: string;
  unifiedModel: string;
  tagMatchMode: string;
};

const props = withDefaults(defineProps<{
  id: string;
  isOpen?: boolean;
  zIndex?: number;
}>(), {
  isOpen: false,
  zIndex: LAYER_PAGE_BASE,
});

const emit = defineEmits(["close"]);

const assistantStore = useAssistantStore();
const sessionStore = useChatSessionStore();
const notificationStore = useNotificationStore();
const overlayStore = useOverlayStore();
const { registerModal } = useModalHistory();

const normalizeGroupConfig = (config: GroupConfigDto): EditableGroupConfig => ({
  ...config,
  memberTags: config.memberTags ?? {},
  groupPrompt: config.groupPrompt || "",
  invitePrompt: config.invitePrompt || "",
  unifiedModel: config.unifiedModel || "",
  tagMatchMode: config.tagMatchMode || "strict",
});
const groupConfig = ref<EditableGroupConfig | null>(null);
const deleteBlockReason = computed(() => groupConfig.value ? chatMutationBlockReason({
  ownerId: groupConfig.value.id, ownerType: "group",
}) : null);
let editorEpoch = 0;
let mounted = true;
const pendingSaves = new Map<string, Promise<boolean>>();
const retainedDrafts = new Map<string, { draft: EditableGroupConfig; baseline: EditableGroupConfig }>();
const isEpochCurrent = (epoch: number) => editorEpoch === epoch;
const isOpenEditorCurrent = (epoch: number, id: string) =>
  mounted && isEpochCurrent(epoch) && props.isOpen && props.id === id;

const allAgents = computed<Pick<AgentListItemDto, "id" | "name">[]>(() => assistantStore.agents);
const isLoading = ref(false);
const isSaving = ref(false);
const isDeleting = ref(false);
const isLeaving = ref(false);
const loadError = ref("");
const saveError = ref("");
const disabled = computed(() => !props.isOpen || isLoading.value || isSaving.value || isDeleting.value || isLeaving.value);

// 原始配置快照，用于判断用户是否真正修改了内容
const originalConfig = ref<EditableGroupConfig | null>(null);

onUnmounted(() => {
  mounted = false;
  editorEpoch += 1;
  cancelAvatarPicker();
  closeSelection();
  void releaseAvatarWorkingCopy();
});

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
    console.warn("Failed to delete Group avatar working copy:", error);
  }
};

const triggerFileInput = async () => {
  if (disabled.value || isPickingAvatar.value || isCropping.value) return;
  const epoch = editorEpoch;
  const groupId = props.id;
  if (!groupId || !isOpenEditorCurrent(epoch, groupId)) return;
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
    if (avatarOperationId !== operationId || !isOpenEditorCurrent(epoch, groupId)) return;
    const picked = await pickFile("avatar", { operationId });
    if (avatarOperationId !== operationId || !isOpenEditorCurrent(epoch, groupId)) {
      await deleteTempFile(picked.path);
      return;
    }
    avatarWorkingPath.value = picked.path;
    cropImg.value = convertFileSrc(picked.path);
    isCropping.value = true;
  } catch (error) {
    if (!isOpenEditorCurrent(epoch, groupId)) return;
    if (!String(error).toLowerCase().includes("cancel")) {
      console.error("Failed to prepare Group avatar image:", error);
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
  const groupId = groupConfig.value?.id;
  if (!groupId || !isOpenEditorCurrent(epoch, groupId)) {
    isCropping.value = false;
    await releaseAvatarWorkingCopy();
    return;
  }

  isCropping.value = false;
  isSaving.value = true;

  try {
    await releaseAvatarWorkingCopy();
    const arrayBuffer = await blob.arrayBuffer();
    if (!isOpenEditorCurrent(epoch, groupId)) return;
    const bytes = new Uint8Array(arrayBuffer);

    // Use assistantStore to save avatar and get notification
    await assistantStore.saveAvatar("group", groupId, blob.type, bytes);

  } catch (err) {
    console.error("Failed to save avatar:", err);
  } finally {
    if (isEpochCurrent(epoch)) isSaving.value = false;
  }
};

const fetchGroupConfig = async (epoch: number, groupId: string) => {
  if (!groupId) return;
  isLoading.value = true;
  loadError.value = "";
  try {
    const pendingSave = pendingSaves.get(groupId);
    if (pendingSave) {
      await pendingSave;
      if (!isOpenEditorCurrent(epoch, groupId)) return;
    }
    const retained = retainedDrafts.get(groupId);
    if (retained) {
      groupConfig.value = cloneConfig(retained.draft);
      originalConfig.value = cloneConfig(retained.baseline);
      saveError.value = "已恢复未保存的修改，请重试保存。";
      return;
    }
    const config = await invoke<GroupConfigDto>("read_group_config", { groupId });
    if (!isOpenEditorCurrent(epoch, groupId)) return;
    groupConfig.value = normalizeGroupConfig(config);
    originalConfig.value = cloneConfig(groupConfig.value);
  } catch (err) {
    if (!isOpenEditorCurrent(epoch, groupId)) return;
    loadError.value = `群组设置读取失败：${String(err)}`;
    console.error("Failed to load group config:", err);
  } finally {
    if (isOpenEditorCurrent(epoch, groupId)) isLoading.value = false;
  }
};

const cloneConfig = (config: EditableGroupConfig): EditableGroupConfig => JSON.parse(JSON.stringify(config));
const hasChanges = computed(() => !!groupConfig.value && !!originalConfig.value &&
  JSON.stringify(groupConfig.value) !== JSON.stringify(originalConfig.value));
const retainDraft = () => {
  if (hasChanges.value && groupConfig.value && originalConfig.value) {
    retainedDrafts.set(groupConfig.value.id, {
      draft: cloneConfig(groupConfig.value),
      baseline: cloneConfig(originalConfig.value),
    });
  }
};

const saveChanges = async (): Promise<boolean> => {
  if (isSaving.value || isLoading.value) return false;
  if (!groupConfig.value || !originalConfig.value || !hasChanges.value) return true;
  const validation = validateDraft(groupConfig.value);
  if (validation) {
    saveError.value = validation;
    return false;
  }
  const epoch = editorEpoch;
  const draft = cloneConfig(groupConfig.value);
  const baseline = cloneConfig(originalConfig.value);
  isSaving.value = true;
  saveError.value = "";
  const task = (async () => {
    try {
      const canonical = normalizeGroupConfig(await assistantStore.saveGroup(draft, baseline));
      const retained = retainedDrafts.get(draft.id);
      if (retained && JSON.stringify(retained.draft) === JSON.stringify(draft)) retainedDrafts.delete(draft.id);
      if (!isOpenEditorCurrent(epoch, draft.id)) return false;
      groupConfig.value = canonical;
      originalConfig.value = cloneConfig(canonical);
      retainedDrafts.delete(draft.id);
      return true;
    } catch (err) {
      if (isOpenEditorCurrent(epoch, draft.id)) {
        saveError.value = `保存失败，修改已保留：${String(err)}`;
        retainDraft();
      }
      return false;
    } finally {
      if (isOpenEditorCurrent(epoch, draft.id)) isSaving.value = false;
    }
  })();
  pendingSaves.set(draft.id, task);
  try {
    return await task;
  } finally {
    if (pendingSaves.get(draft.id) === task) pendingSaves.delete(draft.id);
  }
};

// Reuse OverlayStore's existing modal entry. System back stays on this page
// until the frozen draft has been saved successfully.
const requestClose = (): boolean => {
  if (!mounted) return true;
  if (!props.isOpen || isLeaving.value || isSaving.value || isDeleting.value) return false;
  const epoch = editorEpoch;
  const groupId = props.id;
  isLeaving.value = true;
  cancelAvatarPicker();
  closeSelection();
  const save = isLoading.value || !groupConfig.value ? Promise.resolve(true) : saveChanges();
  void save.then(saved => {
    if (saved && isOpenEditorCurrent(epoch, groupId)) emit("close");
  }).finally(() => {
    if (isEpochCurrent(epoch)) isLeaving.value = false;
  });
  return false;
};

const discardAndClose = async () => {
  if (disabled.value || !groupConfig.value) return;
  const epoch = editorEpoch;
  const groupId = props.id;
  if (!await overlayStore.showConfirm({ title: "放弃修改", message: "放弃本次未保存的群组设置并返回？" })) return;
  if (!isOpenEditorCurrent(epoch, groupId)) return;
  retainedDrafts.delete(groupId);
  originalConfig.value = cloneConfig(groupConfig.value);
  emit("close");
};

const updateEditor = (isOpen: boolean, id: string) => {
  retainDraft();
  cancelAvatarPicker();
  closeSelection();
  const epoch = ++editorEpoch;
  isLeaving.value = false;
  isDeleting.value = false;
  isPickingAvatar.value = false;
  isCropping.value = false;
  showModelSelector.value = false;
  showModeHelp.value = false;
  void releaseAvatarWorkingCopy();
  if (isOpen && id) {
    groupConfig.value = null;
    originalConfig.value = null;
    isSaving.value = false;
    saveError.value = "";
    registerModal(`Page:groupSettings:${id}`, requestClose, undefined, requestClose);
    void fetchGroupConfig(epoch, id);
  }
};

const toggleMember = (agentId: string) => {
  if (disabled.value || !groupConfig.value) return;
  const index = groupConfig.value.members.indexOf(agentId);
  if (index === -1) {
    groupConfig.value.members.push(agentId);
    if (!groupConfig.value.memberTags[agentId]) {
      const agent = allAgents.value.find(a => a.id === agentId);
      groupConfig.value.memberTags[agentId] = agent?.name || agentId;
    }
  } else {
    groupConfig.value.members.splice(index, 1);
    // 对齐 VChat：保留 memberTags，便于该 Agent 重新加入时恢复。
  }
  normalizeSpeakerOrder();
};

const isMember = (agentId: string) => groupConfig.value?.members.includes(agentId) ?? false;

const showModelSelector = ref(false);
const onModelSelect = (modelId: string) => {
  if (disabled.value || !groupConfig.value) return;
  groupConfig.value.unifiedModel = modelId;
};

const handleDelete = async () => {
  if (disabled.value || !groupConfig.value) return;
  const epoch = editorEpoch;
  const groupId = groupConfig.value.id;
  if (!allowChatMutation({ ownerId: groupId, ownerType: "group" })) return;
  const confirmed = await overlayStore.showConfirm({
    title: "删除群组",
    message: "确定要删除这个群组吗？所有聊天记录将被标记为删除。",
    isDanger: true
  });
  if (confirmed && groupId && isOpenEditorCurrent(epoch, groupId)) {
    isDeleting.value = true;
    try {
      if (!await assistantStore.deleteGroup(groupId)) return;
      if (!isOpenEditorCurrent(epoch, groupId)) return;
      if (
        sessionStore.currentSelectedItem?.type === "group" &&
        sessionStore.currentSelectedItem?.id === groupId
      ) {
        sessionStore.clearConversation();
      }
      // 删除成功后关闭页面不再把当前草稿自动保存到墓碑实体。
      retainedDrafts.delete(groupId);
      originalConfig.value = cloneConfig(groupConfig.value);
      emit("close");
    } catch (err: any) {
      notificationStore.addNotification({
        type: 'error',
        message: "删除失败: " + (err?.message || err),
        toastOnly: true
      });
    } finally {
      if (isEpochCurrent(epoch)) isDeleting.value = false;
    }
  }
};

const modeOptions = [
  { value: "sequential", label: "顺序发言", desc: "成员按设定次序依次回复" },
  { value: "naturerandom", label: "自然随机", desc: "根据 @提及、标签和随机机会选择成员" },
  { value: "invite_only", label: "邀请发言", desc: "点击成员或发送 @名字，邀请其回复" },
  { value: "jev", label: "JEV 智能群聊", desc: "根据话题选择成员，讨论结束后自动停下" },
] satisfies { value: GroupMode; label: string; desc: string }[];

const tagModeOptions = [
  { value: "strict", label: "严格模式" },
  { value: "natural", label: "自然模式" },
];

const modeName = (mode: string) => modeOptions.find(option => option.value === mode)?.label ?? modeOptions[0].label;
const effectiveMode = computed(() => groupConfig.value?.chatMode ?? "sequential");
const modeDescription = computed(() => modeOptions.find(option => option.value === effectiveMode.value)?.desc ?? "选择成员的发言方式。");
const availableMembers = computed(() => {
  const knownIds = new Set(allAgents.value.map(agent => agent.id));
  return [
    ...allAgents.value.map(agent => ({ ...agent, available: true })),
    ...(groupConfig.value?.members ?? []).filter(id => !knownIds.has(id)).map(id => ({ id, name: id, available: false })),
  ];
});
const selectedMembers = computed(() => (groupConfig.value?.members ?? []).map(id =>
  availableMembers.value.find(agent => agent.id === id) ?? { id, name: id, available: false },
));
const orderedMembers = computed(() => {
  const group = groupConfig.value;
  if (!group) return [];
  const members = new Set(group.members);
  const order = [...new Set([...group.modeSettings.sequential.speakerOrder, ...group.members])].filter(id => members.has(id));
  return order.map(id => selectedMembers.value.find(agent => agent.id === id)!);
});
const normalizeSpeakerOrder = () => {
  if (groupConfig.value) groupConfig.value.modeSettings.sequential.speakerOrder = orderedMembers.value.map(agent => agent.id);
};
const moveMember = (index: number, offset: number) => {
  if (disabled.value || !groupConfig.value) return;
  const next = index + offset;
  const order = orderedMembers.value.map(agent => agent.id);
  if (next < 0 || next >= order.length) return;
  [order[index], order[next]] = [order[next], order[index]];
  groupConfig.value.modeSettings.sequential.speakerOrder = order;
};

let selectionMenu: ContextMenuConfig | null = null;
const closeSelection = () => {
  if (selectionMenu && overlayStore.contextMenuConfig === selectionMenu) overlayStore.closeContextMenu();
  selectionMenu = null;
};
const openModeSelection = (event: MouseEvent) => {
  const group = groupConfig.value;
  if (disabled.value || !group) return;
  const epoch = editorEpoch;
  (event.currentTarget as HTMLElement).focus({ preventScroll: true });
  const actions: OverlayActionItem[] = modeOptions.map(option => ({
    label: option.label,
    selected: effectiveMode.value === option.value,
    handler: () => {
      if (disabled.value || !isOpenEditorCurrent(epoch, group.id) || overlayStore.contextMenuConfig?.actions !== actions) return;
      group.chatMode = option.value;
    },
  }));
  overlayStore.openContextMenu(actions, "群聊模式");
  selectionMenu = overlayStore.contextMenuConfig;
};
const openTagSelection = (event: MouseEvent) => {
  const group = groupConfig.value;
  if (disabled.value || !group) return;
  const epoch = editorEpoch;
  (event.currentTarget as HTMLElement).focus({ preventScroll: true });
  const actions: OverlayActionItem[] = tagModeOptions.map(option => ({
    label: option.label,
    selected: group.tagMatchMode === option.value,
    handler: () => {
      if (disabled.value || !isOpenEditorCurrent(epoch, group.id) || overlayStore.contextMenuConfig?.actions !== actions) return;
      group.tagMatchMode = option.value;
    },
  }));
  overlayStore.openContextMenu(actions, "标签匹配");
  selectionMenu = overlayStore.contextMenuConfig;
};

const jevNumericFields = [
  { key: "maxSpeakersPerRound", title: "每轮发言人数上限", description: "一轮最多安排多少位成员", min: 1, max: 32, step: 1 },
  { key: "maxAutonomousRounds", title: "讨论轮数上限", description: "达到上限后停止自动讨论", min: 1, max: 100, step: 1 },
  { key: "speakerThreshold", title: "发言门槛", description: "越高，参与发言的成员越少", min: 0, max: 1, step: 0.01 },
  { key: "continueThreshold", title: "继续讨论门槛", description: "越高，越容易结束讨论", min: 0, max: 1, step: 0.01 },
  { key: "stopThreshold", title: "结束讨论门槛", description: "越高，越不容易主动结束", min: 0, max: 1, step: 0.01 },
  { key: "historyWindow", title: "JEV 参考消息数", description: "选择发言成员时参考的最近消息", min: 1, max: 200, step: 1 },
  { key: "continueDebounceMs", title: "继续操作间隔（ms）", description: "避免连续点击重复发起讨论", min: 0, max: 10000, step: 100 },
] as const;
const validateDraft = (draft: EditableGroupConfig): string | null => {
  if (!draft.name.trim()) return "请填写群组名称。";
  if (draft.useUnifiedModel && !draft.unifiedModel.trim()) return "启用群组统一模型后，请选择一个模型。";
  for (const field of jevNumericFields) {
    const value = draft.modeSettings.jev[field.key];
    if (!Number.isFinite(value) || value < field.min || value > field.max || (field.step >= 1 && !Number.isInteger(value))) {
      return `${field.title}需填写 ${field.min}–${field.max} 之间的${field.step >= 1 ? "整数" : "数值"}。`;
    }
  }
  if (!Number.isInteger(draft.contextMessageWindowSize) || draft.contextMessageWindowSize < 1 || draft.contextMessageWindowSize > 10000) {
    return "回复参考消息数需填写 1–10000 之间的整数。";
  }
  return null;
};

// 「群聊模式」帮助说明弹窗
const showModeHelp = ref(false);
watch([() => props.isOpen, () => props.id], ([isOpen, id]) => updateEditor(isOpen, id), { immediate: true });
</script>

<template>
  <SlidePage v-bind="$attrs" :is-open="props.isOpen" :z-index="props.zIndex" :inert="!props.isOpen"
    :aria-hidden="!props.isOpen" aria-label="群组设置" role="dialog" :aria-modal="props.isOpen ? 'true' : undefined">
    <div class="group-settings-view flex flex-col h-full w-full min-w-0 text-primary-text">
      <header class="group-settings-header flex items-center gap-3 shrink-0 px-3 border-b border-[var(--border-color)]">
        <button type="button" class="group-icon-button" :disabled="isSaving || isDeleting || isLeaving" aria-label="返回并保存群组设置" @click="requestClose">
          <ArrowLeft :size="22" :stroke-width="1.8" aria-hidden="true" />
        </button>
        <h2 class="m-0 text-base font-semibold flex-1">群组设置</h2>
        <span v-if="isSaving || isLeaving" role="status" class="text-xs text-[var(--secondary-text)]">保存中…</span>
      </header>

      <SettingsCanvas viewport>
        <p v-if="loadError" role="alert" class="m-0 text-sm text-red-500">
          {{ loadError }}
          <button type="button" class="group-text-button" :disabled="isLoading" @click="fetchGroupConfig(editorEpoch, props.id)">重试</button>
        </p>
        <p v-else-if="isLoading" role="status" class="m-0 text-sm text-[var(--secondary-text)]">正在加载群组设置…</p>
        <div v-if="saveError" role="alert" class="flex flex-col gap-2">
          <p class="m-0 text-sm text-red-500">{{ saveError }}</p>
          <div class="flex gap-4">
            <button type="button" class="group-text-button" :disabled="disabled" @click="saveChanges">重试保存</button>
            <button type="button" class="group-text-button" :disabled="disabled" @click="discardAndClose">放弃修改并返回</button>
          </div>
        </div>

        <template v-if="groupConfig">
          <SettingsSection title="基础信息">
            <div class="flex items-center gap-4">
              <button type="button" class="group-avatar-button" :disabled="disabled || isPickingAvatar" aria-label="更换群组头像" @click="triggerFileInput">
                <VcpAvatar owner-type="group" :owner-id="props.id" :fallback-name="groupConfig.name"
                  size="w-14 h-14" rounded="rounded-full" :dominant-color="groupConfig.avatarCalculatedColor" />
                <span class="text-xs text-[var(--secondary-text)]">{{ isPickingAvatar ? '选择中…' : '更换头像' }}</span>
              </button>
              <SettingsTextField v-model="groupConfig.name" label="群组名称" placeholder="填写群组名称" :disabled="disabled" />
            </div>
          </SettingsSection>

          <SettingsSection title="群组成员" :description="`已加入 ${groupConfig.members.length} 位成员`">
            <div class="group-members-list vcp-scrollable">
              <label v-for="agent in availableMembers" :key="agent.id" class="group-member-row">
                <input type="checkbox" :checked="isMember(agent.id)" :disabled="disabled" class="w-5 h-5 shrink-0 accent-blue-500" @change="toggleMember(agent.id)" />
                <VcpAvatar owner-type="agent" :owner-id="agent.id" :fallback-name="agent.name" size="w-8 h-8" rounded="rounded-full" />
                <span class="flex-1 min-w-0 break-words text-sm" :class="{ 'font-mono': !agent.available }">{{ agent.name }}</span>
                <span v-if="!agent.available" class="text-xs text-[var(--secondary-text)]">不可用</span>
              </label>
            </div>
            <p v-if="!availableMembers.length" class="group-note">暂无助手，请先创建助手再加入群组。</p>
          </SettingsSection>

          <SettingsSection title="群聊模式">
            <template #action>
              <button type="button" class="group-icon-button" aria-label="发言模式说明" @click="showModeHelp = true">
                <HelpCircle :size="18" aria-hidden="true" />
              </button>
            </template>
            <SettingsRow title="发言方式" :value="modeName(effectiveMode)" :description="modeDescription" clickable :disabled="disabled"
              aria-label="群聊模式" aria-haspopup="dialog" @click="openModeSelection" />

            <div v-if="effectiveMode === 'sequential'">
              <p class="group-note mb-2">用上下箭头调整发言次序。</p>
              <div v-for="(agent, index) in orderedMembers" :key="agent.id" class="group-member-row">
                <span class="w-5 shrink-0 font-mono text-xs text-[var(--secondary-text)]">{{ index + 1 }}</span>
                <span class="flex-1 min-w-0 break-words text-sm" :class="{ 'font-mono': !agent.available }">{{ agent.name }}</span>
                <button type="button" class="group-icon-button" :disabled="disabled || index === 0" :aria-label="`上移 ${agent.name}`" @click="moveMember(index, -1)">
                  <ArrowUp :size="18" aria-hidden="true" />
                </button>
                <button type="button" class="group-icon-button" :disabled="disabled || index === orderedMembers.length - 1" :aria-label="`下移 ${agent.name}`" @click="moveMember(index, 1)">
                  <ArrowDown :size="18" aria-hidden="true" />
                </button>
              </div>
              <p v-if="!orderedMembers.length" class="group-note">加入成员后可调整发言次序。</p>
            </div>

            <div v-else-if="effectiveMode === 'naturerandom'" class="flex flex-col gap-3">
              <SettingsRow title="标签匹配" :value="tagModeOptions.find(option => option.value === groupConfig?.tagMatchMode)?.label"
                description="自然模式减少引用历史消息造成的重复回应。" clickable :disabled="disabled" aria-haspopup="dialog" @click="openTagSelection" />
              <SettingsTextField v-for="agent in selectedMembers" :key="agent.id" v-model="groupConfig.memberTags[agent.id]"
                :label="`${agent.name} 的触发标签`" placeholder="多个标签用逗号分隔" :disabled="disabled" />
            </div>

            <div v-else-if="effectiveMode === 'jev'" class="flex flex-col gap-4">
              <GroupSettingsTextArea v-model="groupConfig.modeSettings.jev.navigatorPrompt" label="讨论规则" :disabled="disabled" :rows="5"
                description="决定谁发言、何时结束。留空使用默认规则。" />
              <SettingsDisclosure title="成员发言风格" description="各成员适合回应的话题与时机" :disabled="disabled">
                <div class="flex flex-col gap-4">
                  <GroupSettingsTextArea v-for="agent in selectedMembers" :key="agent.id" :model-value="groupConfig.modeSettings.jev.memberStyles[agent.id] ?? ''"
                    :label="agent.name" :rows="3" :disabled="disabled" placeholder="例如：熟悉技术问题，适合补充细节。"
                    @update:model-value="groupConfig.modeSettings.jev.memberStyles[agent.id] = $event" />
                  <p v-if="!selectedMembers.length" class="group-note">加入成员后可填写发言风格。</p>
                </div>
              </SettingsDisclosure>
              <div>
                <SettingsRow v-for="field in jevNumericFields.slice(0, 2)" :key="field.key" :title="field.title" :description="field.description">
                  <template #action>
                    <input v-model.number="groupConfig.modeSettings.jev[field.key]" type="number" inputmode="numeric" :min="field.min" :max="field.max" :step="field.step"
                      :disabled="disabled" :aria-label="field.title" class="group-number-input" />
                  </template>
                </SettingsRow>
              </div>
              <SettingsDisclosure title="高级设置" description="发言门槛与参考消息范围" :disabled="disabled">
                <SettingsRow v-for="field in jevNumericFields.slice(2)" :key="field.key" :title="field.title" :description="field.description">
                  <template #action>
                    <input v-model.number="groupConfig.modeSettings.jev[field.key]" type="number" :inputmode="field.step < 1 ? 'decimal' : 'numeric'"
                      :min="field.min" :max="field.max" :step="field.step" :disabled="disabled" :aria-label="field.title" class="group-number-input" />
                  </template>
                </SettingsRow>
              </SettingsDisclosure>
              <p class="group-note">服务连接请在「JEV 设置」中配置。</p>
            </div>
          </SettingsSection>

          <SettingsSection title="成员模型">
            <div>
              <SettingsRow title="启用群组统一模型" description="所有成员使用同一模型回复。">
                <template #action><SettingsSwitch v-model="groupConfig.useUnifiedModel" :disabled="disabled" aria-label="启用群组统一模型" /></template>
              </SettingsRow>
              <SettingsRow v-if="groupConfig.useUnifiedModel" title="统一模型" :value="groupConfig.unifiedModel || '请选择'" clickable :disabled="disabled"
                @click="showModelSelector = true" />
              <SettingsRow title="限制参考消息" description="回复时只参考最近的消息，不删除聊天记录。">
                <template #action><SettingsSwitch v-model="groupConfig.enableContextMessageWindow" :disabled="disabled" aria-label="限制参考消息" /></template>
              </SettingsRow>
              <SettingsRow v-if="groupConfig.enableContextMessageWindow" title="回复参考消息数" description="成员回复时使用，与 JEV 的参考范围分开设置。">
                <template #action>
                  <input v-model.number="groupConfig.contextMessageWindowSize" type="number" inputmode="numeric" min="1" max="10000" step="1"
                    :disabled="disabled" aria-label="回复参考消息数" class="group-number-input" />
                </template>
              </SettingsRow>
            </div>
          </SettingsSection>

          <SettingsSection title="提示词">
            <GroupSettingsTextArea v-model="groupConfig.groupPrompt" label="群组提示词" placeholder="群组背景与整体对话要求" :disabled="disabled" />
            <GroupSettingsTextArea v-model="groupConfig.invitePrompt" label="邀请提示词" placeholder="轮到成员发言时发送的提示语"
              description="{{VCPChatAgentName}} 代表成员名字" :disabled="disabled" :rows="3" />
          </SettingsSection>

          <p class="group-note">返回时自动保存。</p>
          <SettingsRow title="删除此群组" :description="deleteBlockReason ?? undefined" clickable danger
            :disabled="disabled || deleteBlockReason !== null" @click="handleDelete" />
        </template>
      </SettingsCanvas>

      <ModelSelector v-model="showModelSelector" :current-model="groupConfig?.unifiedModel ?? ''" title="选择群组统一模型"
        @select="onModelSelect" />
    </div>
  </SlidePage>
  <!-- 头像裁剪器 -->
  <AvatarCropper v-if="isCropping" :img="cropImg" @cancel="cancelAvatarCrop" @confirm="onCropConfirm" />

  <!-- 群聊模式帮助说明 -->
  <GroupModeHelpDialog v-model="showModeHelp" />
</template>

<style scoped>
.group-settings-view {
  background-color: var(--primary-bg);
}
.group-settings-header { padding-top: var(--vcp-safe-top, 24px); padding-bottom: 8px; }
.group-icon-button { display: inline-flex; align-items: center; justify-content: center; min-width: 44px; min-height: 44px; padding: 8px; border: 0; border-radius: 8px; background: transparent; color: inherit; touch-action: manipulation; transition: opacity 120ms ease; }
.group-icon-button:active, .group-avatar-button:active { opacity: 0.6; }
.group-icon-button:disabled { opacity: 0.25; }
.group-avatar-button { display: flex; align-items: center; flex-direction: column; flex-shrink: 0; gap: 6px; padding: 0; border: 0; background: transparent; touch-action: manipulation; }
.group-avatar-button:disabled { opacity: 0.5; }
.group-members-list { max-height: 320px; overflow-y: auto; }
.group-member-row { display: flex; align-items: center; gap: 12px; min-height: 48px; padding-block: 4px; }
.group-member-row + .group-member-row { border-top: 1px solid var(--settings-row-divider, var(--border-color)); }
.group-note { margin: 0; font-size: 12px; line-height: 1.6; color: var(--secondary-text); }
.group-text-button { align-self: flex-start; min-height: 44px; padding: 6px 0; border: 0; background: transparent; color: var(--secondary-text); font-size: 12px; text-decoration: underline; touch-action: manipulation; }
.group-text-button:disabled { opacity: 0.5; }
.group-number-input { width: 88px; max-width: 100%; min-height: 44px; padding: 8px; border: 1px solid var(--border-color); border-radius: 8px; background: var(--input-bg, transparent); color: var(--primary-text); font-family: monospace; font-size: var(--settings-input-font-size, 16px); text-align: right; }
.group-number-input:disabled { opacity: 0.5; }
.group-number-input:focus-visible, .group-icon-button:focus-visible { outline: 2px solid var(--highlight-text, var(--primary-text)); outline-offset: 2px; }
</style>
