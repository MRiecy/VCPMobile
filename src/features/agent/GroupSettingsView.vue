<script setup lang="ts">
defineOptions({ inheritAttrs: false });
import { computed, ref, onUnmounted, watch } from "vue";
import { ArrowDown, ArrowLeft, ArrowUp, ChevronDown, HelpCircle, Plus, Search, X } from "@lucide/vue";
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
const { registerModal, unregisterModal } = useModalHistory();

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

const allAgents = computed<Pick<AgentListItemDto, "id" | "name" | "model">[]>(() => assistantStore.agents);
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
  closeLocalSheets();
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
  closeLocalSheets();
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

type PromptSectionKey = "groupPrompt" | "invitePrompt" | "jevNavigator";
const openPrompts = ref<Record<PromptSectionKey, boolean>>({
  groupPrompt: false,
  invitePrompt: false,
  jevNavigator: false,
});
const togglePromptSection = (key: PromptSectionKey) => {
  if (disabled.value) return;
  openPrompts.value[key] = !openPrompts.value[key];
};

const ROSTER_SHEET_MODAL_ID = "GroupRosterSheet";
const MEMBER_SHEET_MODAL_ID = "GroupMemberSheet";
const showRosterSheet = ref(false);
const rosterSearchQuery = ref("");
const rosterSheetRef = ref<HTMLElement | null>(null);
const activeMemberId = ref<string | null>(null);
const memberSheetRef = ref<HTMLElement | null>(null);

const closeRosterSheet = () => {
  if (!showRosterSheet.value) return;
  showRosterSheet.value = false;
  rosterSearchQuery.value = "";
  unregisterModal(ROSTER_SHEET_MODAL_ID);
};

const openRosterSheet = () => {
  if (disabled.value || !groupConfig.value) return;
  closeMemberSheet();
  rosterSearchQuery.value = "";
  showRosterSheet.value = true;
  registerModal(
    ROSTER_SHEET_MODAL_ID,
    () => {
      showRosterSheet.value = false;
      rosterSearchQuery.value = "";
    },
    () => rosterSheetRef.value,
  );
};

const closeMemberSheet = () => {
  if (activeMemberId.value === null) return;
  activeMemberId.value = null;
  unregisterModal(MEMBER_SHEET_MODAL_ID);
};

const openMemberSheet = (agentId: string) => {
  if (disabled.value || !groupConfig.value || !groupConfig.value.members.includes(agentId)) return;
  closeRosterSheet();
  activeMemberId.value = agentId;
  registerModal(
    MEMBER_SHEET_MODAL_ID,
    () => {
      activeMemberId.value = null;
    },
    () => memberSheetRef.value,
  );
};

const closeLocalSheets = () => {
  closeRosterSheet();
  closeMemberSheet();
};

const updateEditor = (isOpen: boolean, id: string) => {
  retainDraft();
  cancelAvatarPicker();
  closeSelection();
  closeLocalSheets();
  const epoch = ++editorEpoch;
  isLeaving.value = false;
  isDeleting.value = false;
  isPickingAvatar.value = false;
  isCropping.value = false;
  showModelSelector.value = false;
  showModeHelp.value = false;
  openPrompts.value = { groupPrompt: false, invitePrompt: false, jevNavigator: false };
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
  const group = groupConfig.value;
  const currentOrder = orderedMembers.value.map(agent => agent.id);
  if (!group.members.includes(agentId)) {
    // 顺序入栈：按勾选顺序追加到当前席位栈尾
    const nextOrder = [...currentOrder.filter(id => id !== agentId), agentId];
    group.members = [...nextOrder];
    group.modeSettings.sequential.speakerOrder = [...nextOrder];
    if (!group.memberTags[agentId]) {
      const agent = allAgents.value.find(a => a.id === agentId);
      group.memberTags[agentId] = agent?.name || agentId;
    }
  } else {
    // 直接抽离：移除该成员，其余成员保持原有相对次序；保留 memberTags / memberStyles 便于重新入群恢复
    const nextOrder = currentOrder.filter(id => id !== agentId);
    group.members = group.members.filter(id => id !== agentId);
    group.modeSettings.sequential.speakerOrder = nextOrder;
    if (activeMemberId.value === agentId) {
      closeMemberSheet();
    }
  }
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
  { value: "sequential", code: "SEQ", label: "顺序发言", shortLabel: "顺序发言", desc: "成员按设定次序依次回复" },
  { value: "naturerandom", code: "RND", label: "自然随机", shortLabel: "自然随机", desc: "根据 @提及、标签和随机机会选择成员" },
  { value: "invite_only", code: "INV", label: "邀请发言", shortLabel: "邀请发言", desc: "点击成员或发送 @名字，邀请其回复" },
  { value: "jev", code: "JEV", label: "JEV 智能群聊", shortLabel: "JEV 智控", desc: "根据话题选择成员，讨论结束后自动停下" },
] satisfies { value: GroupMode; code: string; label: string; shortLabel: string; desc: string }[];

const tagModeOptions = [
  { value: "strict", label: "严格模式" },
  { value: "natural", label: "自然模式" },
];
const DEFAULT_INVITE_PROMPT_HINT = "{{VCPChatAgentName}} 代表成员名字";

const effectiveMode = computed(() => groupConfig.value?.chatMode ?? "sequential");
const currentModeMeta = computed(() => modeOptions.find(option => option.value === effectiveMode.value) ?? modeOptions[0]);
const modeDescription = computed(() => currentModeMeta.value.desc);
const selectMode = (mode: GroupMode) => {
  if (disabled.value || !groupConfig.value) return;
  groupConfig.value.chatMode = mode;
};
const rosterHint = computed(() => {
  switch (effectiveMode.value) {
    case "sequential":
      return "上下调整次序 · 点击成员配置";
    case "naturerandom":
      return "点击成员编辑触发标签";
    case "jev":
      return "点击成员配置发言风格";
    default:
      return "点击成员或发送 @名字 呼出";
  }
});

const availableMembers = computed(() => {
  const knownIds = new Set(allAgents.value.map(agent => agent.id));
  return [
    ...allAgents.value.map(agent => ({ ...agent, available: true })),
    ...(groupConfig.value?.members ?? []).filter(id => !knownIds.has(id)).map(id => ({ id, name: id, model: "", available: false })),
  ];
});
const filteredAvailableMembers = computed(() => {
  const q = rosterSearchQuery.value.trim().toLowerCase();
  if (!q) return availableMembers.value;
  return availableMembers.value.filter(
    agent => agent.name.toLowerCase().includes(q) || agent.id.toLowerCase().includes(q),
  );
});
const selectedMembers = computed(() => (groupConfig.value?.members ?? []).map(id =>
  availableMembers.value.find(agent => agent.id === id) ?? { id, name: id, model: "", available: false },
));
const orderedMembers = computed(() => {
  const group = groupConfig.value;
  if (!group) return [];
  const members = new Set(group.members);
  const order = [...new Set([...group.modeSettings.sequential.speakerOrder, ...group.members])].filter(id => members.has(id));
  return order.map(id => selectedMembers.value.find(agent => agent.id === id)!);
});
const memberOrderIndex = (agentId: string) =>
  orderedMembers.value.findIndex(agent => agent.id === agentId);
const activeMember = computed(() =>
  activeMemberId.value ? orderedMembers.value.find(agent => agent.id === activeMemberId.value) ?? null : null,
);
const activeMemberOrderIndex = computed(() =>
  activeMember.value ? memberOrderIndex(activeMember.value.id) : -1,
);
const memberTagList = (agentId: string): string[] => {
  const raw = groupConfig.value?.memberTags[agentId] ?? "";
  return raw
    .split(/[,，]/)
    .map(tag => tag.trim())
    .filter(Boolean);
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
          <!-- 1. 紧凑群身份横幅 -->
          <section class="group-identity-banner" aria-label="基础信息">
            <button type="button" class="group-avatar-button" :disabled="disabled || isPickingAvatar" aria-label="更换群组头像" @click="triggerFileInput">
              <div class="relative">
                <VcpAvatar owner-type="group" :owner-id="props.id" :fallback-name="groupConfig.name"
                  size="w-14 h-14" rounded="rounded-xl" :dominant-color="groupConfig.avatarCalculatedColor" />
                <span class="group-avatar-badge font-mono">{{ isPickingAvatar ? '…' : '改' }}</span>
              </div>
            </button>
            <div class="flex-1 min-w-0 flex flex-col gap-1.5">
              <div class="flex items-center gap-2 flex-wrap">
                <span class="text-xs font-mono text-[var(--secondary-text)] truncate max-w-[11rem]">{{ groupConfig.id }}</span>
                <span class="group-meta-pill font-mono">{{ groupConfig.members.length }} AGENTS · {{ currentModeMeta.code }}</span>
              </div>
              <SettingsTextField v-model="groupConfig.name" label="群组名称" placeholder="填写群组名称" :disabled="disabled" />
            </div>
          </section>

          <!-- 2. 四宫格发言调度模式 -->
          <SettingsSection title="发言调度模式" :description="modeDescription">
            <template #action>
              <button type="button" class="group-icon-button" aria-label="发言模式说明" @click="showModeHelp = true">
                <HelpCircle :size="18" aria-hidden="true" />
              </button>
            </template>
            <div class="group-mode-grid" role="radiogroup" aria-label="群聊模式">
              <button
                v-for="option in modeOptions"
                :key="option.value"
                type="button"
                role="radio"
                :aria-checked="effectiveMode === option.value"
                :aria-label="option.label"
                :disabled="disabled"
                class="group-mode-pill"
                :class="{ 'is-active': effectiveMode === option.value }"
                @click="selectMode(option.value)"
              >
                <span class="group-mode-code font-mono">{{ option.code }}</span>
                <span class="group-mode-label">{{ option.shortLabel }}</span>
              </button>
            </div>
          </SettingsSection>

          <!-- 3. 随模式变形的合一成员席位 -->
          <SettingsSection title="当前群席位">
            <template #description>
              <span class="font-mono">{{ orderedMembers.length }} 位成员</span> · {{ rosterHint }}
            </template>
            <template #action>
              <button type="button" class="group-roster-action-btn" :disabled="disabled" aria-label="增减成员" @click="openRosterSheet">
                <Plus :size="14" aria-hidden="true" />
                <span>增减成员</span>
              </button>
            </template>

            <div v-if="orderedMembers.length" class="group-roster-list">
              <div v-for="(agent, index) in orderedMembers" :key="agent.id" class="group-member-row">
                <span class="w-6 shrink-0 font-mono text-xs text-[var(--secondary-text)]">{{ String(index + 1).padStart(2, '0') }}</span>
                <VcpAvatar owner-type="agent" :owner-id="agent.id" :fallback-name="agent.name" size="w-8 h-8" rounded="rounded-full" />

                <button
                  type="button"
                  class="group-member-main"
                  :disabled="disabled"
                  :aria-label="`配置 ${agent.name}`"
                  @click="openMemberSheet(agent.id)"
                >
                  <div class="flex items-center gap-2 min-w-0">
                    <span class="truncate text-sm font-medium" :class="{ 'font-mono': !agent.available }">{{ agent.name }}</span>
                    <span v-if="!agent.available" class="shrink-0 text-xs text-[var(--secondary-text)]">不可用</span>
                  </div>

                  <div v-if="effectiveMode === 'sequential'" class="group-member-sub font-mono">
                    顺位 #{{ index + 1 }}<template v-if="agent.model"> · {{ agent.model }}</template>
                  </div>
                  <div v-else-if="effectiveMode === 'naturerandom'" class="group-member-tags">
                    <template v-if="memberTagList(agent.id).length">
                      <span v-for="tag in memberTagList(agent.id)" :key="tag" class="group-tag-chip font-mono">{{ tag }}</span>
                    </template>
                    <span v-else class="group-member-sub">未设置触发标签 · 点击配置</span>
                  </div>
                  <div v-else-if="effectiveMode === 'jev'" class="group-member-sub">
                    {{ (groupConfig.modeSettings.jev.memberStyles[agent.id] || '').trim() ? `职责：${groupConfig.modeSettings.jev.memberStyles[agent.id].trim()}` : '未设置发言风格 · 点击配置' }}
                  </div>
                  <div v-else class="group-member-sub font-mono">
                    @{{ agent.name }} 触发回复
                  </div>
                </button>

                <div v-if="effectiveMode === 'sequential'" class="flex items-center gap-0.5 shrink-0">
                  <button type="button" class="group-icon-button" :disabled="disabled || index === 0" :aria-label="`上移 ${agent.name}`" @click="moveMember(index, -1)">
                    <ArrowUp :size="18" aria-hidden="true" />
                  </button>
                  <button type="button" class="group-icon-button" :disabled="disabled || index === orderedMembers.length - 1" :aria-label="`下移 ${agent.name}`" @click="moveMember(index, 1)">
                    <ArrowDown :size="18" aria-hidden="true" />
                  </button>
                </div>
                <button v-else type="button" class="group-member-config-btn font-mono" :disabled="disabled" @click="openMemberSheet(agent.id)">
                  配置 ›
                </button>
              </div>
            </div>

            <button v-else type="button" class="group-empty-roster-btn" :disabled="disabled" @click="openRosterSheet">
              {{ availableMembers.length ? '暂无入群成员 · 点击「+ 增减成员」按顺序添加助手' : '暂无可用助手，请先创建助手再加入群组' }}
            </button>

            <!-- 模式专属内联扩展区 -->
            <div v-if="effectiveMode === 'naturerandom'" class="group-mode-extension">
              <SettingsRow title="标签匹配" :value="tagModeOptions.find(option => option.value === groupConfig?.tagMatchMode)?.label"
                description="自然模式减少引用历史消息造成的重复回应。" clickable :disabled="disabled" aria-haspopup="dialog" @click="openTagSelection" />
            </div>

            <div v-else-if="effectiveMode === 'jev'" class="group-mode-extension flex flex-col gap-3">
              <div>
                <SettingsRow v-for="field in jevNumericFields.slice(0, 2)" :key="field.key" :title="field.title" :description="field.description">
                  <template #action>
                    <input v-model.number="groupConfig.modeSettings.jev[field.key]" type="number" inputmode="numeric" :min="field.min" :max="field.max" :step="field.step"
                      :disabled="disabled" :aria-label="field.title" class="group-number-input" />
                  </template>
                </SettingsRow>
                <div class="group-prompt-collapse">
                  <button type="button" class="group-prompt-trigger" :disabled="disabled" :aria-expanded="openPrompts.jevNavigator" @click="togglePromptSection('jevNavigator')">
                    <div class="min-w-0 flex-1 pr-3">
                      <div class="text-sm font-medium text-primary-text">讨论规则</div>
                      <div class="text-xs text-[var(--secondary-text)] truncate mt-0.5">
                        {{ groupConfig.modeSettings.jev.navigatorPrompt.trim() || '决定谁发言、何时结束。留空使用默认规则。' }}
                      </div>
                    </div>
                    <span class="group-prompt-action font-mono">
                      {{ openPrompts.jevNavigator ? '收起' : '编辑' }}
                      <ChevronDown :size="14" class="transition-transform" :class="{ 'rotate-180': openPrompts.jevNavigator }" aria-hidden="true" />
                    </span>
                  </button>
                  <div v-if="openPrompts.jevNavigator" class="pt-2 pb-1">
                    <GroupSettingsTextArea v-model="groupConfig.modeSettings.jev.navigatorPrompt" label="讨论规则" :disabled="disabled" :rows="5"
                      description="决定谁发言、何时结束。留空使用默认规则。" />
                  </div>
                </div>
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

          <!-- 4. 上下文与提示词 (手风琴收纳) -->
          <SettingsSection title="上下文与提示词">
            <div class="group-context-list">
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

              <div class="group-prompt-collapse">
                <button type="button" class="group-prompt-trigger" :disabled="disabled" :aria-expanded="openPrompts.groupPrompt" @click="togglePromptSection('groupPrompt')">
                  <div class="min-w-0 flex-1 pr-3">
                    <div class="text-sm font-medium text-primary-text">群组提示词</div>
                    <div class="text-xs text-[var(--secondary-text)] truncate mt-0.5">
                      {{ groupConfig.groupPrompt.trim() || '群组背景与整体对话要求' }}
                    </div>
                  </div>
                  <span class="group-prompt-action font-mono">
                    {{ openPrompts.groupPrompt ? '收起' : '编辑' }}
                    <ChevronDown :size="14" class="transition-transform" :class="{ 'rotate-180': openPrompts.groupPrompt }" aria-hidden="true" />
                  </span>
                </button>
                <div v-if="openPrompts.groupPrompt" class="pt-2 pb-1">
                  <GroupSettingsTextArea v-model="groupConfig.groupPrompt" label="群组提示词" placeholder="群组背景与整体对话要求" :disabled="disabled" />
                </div>
              </div>

              <div class="group-prompt-collapse">
                <button type="button" class="group-prompt-trigger" :disabled="disabled" :aria-expanded="openPrompts.invitePrompt" @click="togglePromptSection('invitePrompt')">
                  <div class="min-w-0 flex-1 pr-3">
                    <div class="text-sm font-medium text-primary-text">邀请提示词</div>
                    <div class="text-xs text-[var(--secondary-text)] font-mono truncate mt-0.5">
                      {{ groupConfig.invitePrompt.trim() || DEFAULT_INVITE_PROMPT_HINT }}
                    </div>
                  </div>
                  <span class="group-prompt-action font-mono">
                    {{ openPrompts.invitePrompt ? '收起' : '编辑' }}
                    <ChevronDown :size="14" class="transition-transform" :class="{ 'rotate-180': openPrompts.invitePrompt }" aria-hidden="true" />
                  </span>
                </button>
                <div v-if="openPrompts.invitePrompt" class="pt-2 pb-1">
                  <GroupSettingsTextArea v-model="groupConfig.invitePrompt" label="邀请提示词" placeholder="轮到成员发言时发送的提示语"
                    description="{{VCPChatAgentName}} 代表成员名字" :disabled="disabled" :rows="3" />
                </div>
              </div>
            </div>
          </SettingsSection>

          <p class="group-note">返回时自动保存。</p>
          <SettingsRow title="删除此群组" :description="deleteBlockReason ?? undefined" clickable danger
            :disabled="disabled || deleteBlockReason !== null" @click="handleDelete" />
        </template>
      </SettingsCanvas>

      <ModelSelector v-model="showModelSelector" :current-model="groupConfig?.unifiedModel ?? ''" title="选择群组统一模型"
        @select="onModelSelect" />

      <!-- 底部抽屉 1：增减成员（按勾选顺序入栈，取消勾选直接抽离） -->
      <Transition name="fade">
        <div
          v-if="showRosterSheet && groupConfig"
          class="group-sheet-backdrop z-sheet"
          @click.self="closeRosterSheet"
        >
          <div
            ref="rosterSheetRef"
            class="group-bottom-sheet vcp-viewport-bottom"
            role="dialog"
            aria-modal="true"
            aria-label="增减群组成员"
          >
            <div class="group-sheet-header">
              <div class="min-w-0">
                <h3 class="m-0 text-sm font-semibold text-primary-text">增减群组成员</h3>
                <p class="m-0 text-xs text-[var(--secondary-text)] mt-0.5">
                  已入群 <span class="font-mono">{{ groupConfig.members.length }}</span> 位 · 勾选顺序入栈，取消直接抽离
                </p>
              </div>
              <button type="button" class="group-sheet-done-btn" @click="closeRosterSheet">完成</button>
            </div>

            <div v-if="availableMembers.length > 4" class="px-4 pt-2 pb-1">
              <div class="group-sheet-search">
                <Search :size="15" class="text-[var(--secondary-text)] shrink-0" aria-hidden="true" />
                <input v-model="rosterSearchQuery" type="text" placeholder="搜索助手名称…" class="group-sheet-search-input" />
                <button v-if="rosterSearchQuery" type="button" class="group-icon-button !min-w-7 !min-h-7 !p-1" aria-label="清除搜索" @click="rosterSearchQuery = ''">
                  <X :size="14" aria-hidden="true" />
                </button>
              </div>
            </div>

            <div class="group-sheet-body vcp-scrollable">
              <label v-for="agent in filteredAvailableMembers" :key="agent.id" class="group-sheet-member-row">
                <input type="checkbox" :checked="isMember(agent.id)" :disabled="disabled" class="w-5 h-5 shrink-0 accent-blue-500" @change="toggleMember(agent.id)" />
                <VcpAvatar owner-type="agent" :owner-id="agent.id" :fallback-name="agent.name" size="w-8 h-8" rounded="rounded-full" />
                <div class="flex-1 min-w-0">
                  <div class="text-sm font-medium truncate" :class="{ 'font-mono': !agent.available }">{{ agent.name }}</div>
                  <div v-if="agent.model || !agent.available" class="text-xs font-mono text-[var(--secondary-text)] truncate">
                    {{ agent.available ? agent.model : '不可用' }}
                  </div>
                </div>
                <span v-if="memberOrderIndex(agent.id) !== -1" class="group-order-badge font-mono">
                  #{{ String(memberOrderIndex(agent.id) + 1).padStart(2, '0') }}
                </span>
              </label>
              <p v-if="!filteredAvailableMembers.length" class="group-note py-6 text-center">
                {{ availableMembers.length ? '未找到匹配的助手' : '暂无可用助手，请先创建助手' }}
              </p>
            </div>
          </div>
        </div>
      </Transition>

      <!-- 底部抽屉 2：单成员群内配置（触发标签 / JEV 发言风格 / 移出本群） -->
      <Transition name="fade">
        <div
          v-if="activeMember && groupConfig"
          class="group-sheet-backdrop z-sheet"
          @click.self="closeMemberSheet"
        >
          <div
            ref="memberSheetRef"
            class="group-bottom-sheet vcp-viewport-bottom"
            role="dialog"
            aria-modal="true"
            :aria-label="`${activeMember.name} 群内配置`"
          >
            <div class="group-sheet-header">
              <div class="flex items-center gap-2.5 min-w-0">
                <VcpAvatar owner-type="agent" :owner-id="activeMember.id" :fallback-name="activeMember.name" size="w-9 h-9" rounded="rounded-full" />
                <div class="min-w-0">
                  <div class="flex items-center gap-2">
                    <h3 class="m-0 text-sm font-semibold text-primary-text truncate">{{ activeMember.name }}</h3>
                    <span v-if="activeMemberOrderIndex !== -1" class="group-order-badge font-mono">
                      #{{ String(activeMemberOrderIndex + 1).padStart(2, '0') }}
                    </span>
                  </div>
                  <p class="m-0 text-xs font-mono text-[var(--secondary-text)] truncate">
                    {{ activeMember.model || activeMember.id }}
                  </p>
                </div>
              </div>
              <button type="button" class="group-sheet-done-btn" @click="closeMemberSheet">完成</button>
            </div>

            <div class="group-sheet-body vcp-scrollable flex flex-col gap-4 py-3">
              <SettingsTextField
                :model-value="groupConfig.memberTags[activeMember.id] ?? ''"
                :label="`${activeMember.name} 的触发标签`"
                placeholder="多个标签用逗号分隔"
                :disabled="disabled"
                @update:model-value="groupConfig.memberTags[activeMember.id] = $event"
              />
              <GroupSettingsTextArea
                :model-value="groupConfig.modeSettings.jev.memberStyles[activeMember.id] ?? ''"
                :label="`${activeMember.name} 的发言风格`"
                :rows="3"
                :disabled="disabled"
                placeholder="例如：熟悉技术问题，适合补充细节。"
                description="JEV 智能群聊中，决定该成员适合回应的话题与时机。"
                @update:model-value="groupConfig.modeSettings.jev.memberStyles[activeMember.id] = $event"
              />

              <div class="group-sheet-footer">
                <button type="button" class="group-remove-member-btn" :disabled="disabled" @click="toggleMember(activeMember.id)">
                  移出本群
                </button>
                <span class="text-xs font-mono text-[var(--secondary-text)]">
                  SLOT #{{ String(activeMemberOrderIndex + 1).padStart(2, '0') }}
                </span>
              </div>
            </div>
          </div>
        </div>
      </Transition>
    </div>
  </SlidePage>

  <!-- 头像裁剪器 -->
  <AvatarCropper v-if="isCropping" :img="cropImg" @cancel="cancelAvatarCrop" @confirm="onCropConfirm" />

  <!-- 群聊模式帮助说明 -->
  <GroupModeHelpDialog v-model="showModeHelp" />
</template>

<style scoped>
.group-settings-view {
  position: relative;
  background-color: var(--primary-bg);
}
.group-settings-header { padding-top: var(--vcp-safe-top, 24px); padding-bottom: 8px; }
.group-icon-button { display: inline-flex; align-items: center; justify-content: center; min-width: 44px; min-height: 44px; padding: 8px; border: 0; border-radius: 8px; background: transparent; color: inherit; touch-action: manipulation; transition: opacity 120ms ease; }
.group-icon-button:active, .group-avatar-button:active { opacity: 0.6; }
.group-icon-button:disabled { opacity: 0.25; }

/* 1. 紧凑群身份横幅 */
.group-identity-banner {
  display: flex;
  align-items: center;
  gap: 14px;
  padding-bottom: 16px;
  border-bottom: 1px solid var(--settings-row-divider, var(--border-color));
}
.group-avatar-button {
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  padding: 0;
  border: 0;
  background: transparent;
  touch-action: manipulation;
}
.group-avatar-button:disabled { opacity: 0.5; }
.group-avatar-badge {
  position: absolute;
  right: -4px;
  bottom: -4px;
  padding: 1px 5px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--secondary-bg, var(--primary-bg));
  color: var(--secondary-text);
  font-size: 12px;
  line-height: 1.3;
}
.group-meta-pill {
  padding: 1px 6px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  color: var(--secondary-text);
  font-size: 12px;
  line-height: 1.4;
}

/* 2. 四宫格模式选择器 */
.group-mode-grid {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 6px;
  padding: 4px;
  border: 1px solid var(--border-color);
  border-radius: 12px;
  background: var(--secondary-bg, transparent);
}
.group-mode-pill {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 2px;
  min-height: 52px;
  padding: 6px 4px;
  border: 1px solid transparent;
  border-radius: 8px;
  background: transparent;
  color: var(--secondary-text);
  touch-action: manipulation;
  transition: opacity 120ms ease, border-color 120ms ease, color 120ms ease;
}
.group-mode-pill:active { opacity: 0.65; }
.group-mode-pill:disabled { opacity: 0.4; }
.group-mode-pill.is-active {
  border-color: var(--highlight-text, #3b82f6);
  background: var(--primary-bg);
  color: var(--highlight-text, var(--primary-text));
  font-weight: 600;
}
.group-mode-code { font-size: 12px; line-height: 1.2; }
.group-mode-label { font-size: 12px; line-height: 1.3; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 100%; }

/* 3. 合一成员席位 */
.group-roster-action-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  min-height: 36px;
  padding: 4px 10px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: transparent;
  color: var(--highlight-text, var(--primary-text));
  font-size: 12px;
  font-weight: 500;
  touch-action: manipulation;
  transition: opacity 120ms ease;
}
.group-roster-action-btn:active { opacity: 0.65; }
.group-roster-action-btn:disabled { opacity: 0.4; }

.group-roster-list {
  border-top: 1px solid var(--settings-row-divider, var(--border-color));
  border-bottom: 1px solid var(--settings-row-divider, var(--border-color));
}
.group-member-row {
  display: flex;
  align-items: center;
  gap: 10px;
  min-height: 54px;
  padding-block: 6px;
}
.group-member-row + .group-member-row {
  border-top: 1px solid var(--settings-row-divider, var(--border-color));
}
.group-member-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 2px;
  padding: 4px 0;
  border: 0;
  background: transparent;
  color: inherit;
  text-align: left;
  touch-action: manipulation;
  transition: opacity 120ms ease;
}
.group-member-main:active { opacity: 0.65; }
.group-member-sub {
  width: 100%;
  color: var(--secondary-text);
  font-size: 12px;
  line-height: 1.4;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.group-member-tags {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  margin-top: 2px;
}
.group-tag-chip {
  padding: 1px 6px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  background: var(--secondary-bg, transparent);
  color: var(--secondary-text);
  font-size: 12px;
  line-height: 1.4;
}
.group-member-config-btn {
  flex-shrink: 0;
  min-height: 36px;
  padding: 4px 8px;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: var(--secondary-text);
  font-size: 12px;
  touch-action: manipulation;
}
.group-member-config-btn:active { opacity: 0.65; }
.group-empty-roster-btn {
  width: 100%;
  min-height: 52px;
  padding: 12px;
  border: 1px dashed var(--border-color);
  border-radius: 10px;
  background: transparent;
  color: var(--secondary-text);
  font-size: 12px;
  text-align: center;
  touch-action: manipulation;
}
.group-empty-roster-btn:active { opacity: 0.65; }
.group-mode-extension {
  margin-top: 4px;
}

/* 4. 手风琴折叠提示词行 */
.group-prompt-collapse {
  border-top: 1px solid var(--settings-row-divider, var(--border-color));
}
.group-context-list > .group-prompt-collapse:first-child {
  border-top: 0;
}
.group-prompt-trigger {
  width: 100%;
  min-height: 52px;
  padding: 10px 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  border: 0;
  background: transparent;
  color: inherit;
  text-align: left;
  touch-action: manipulation;
  transition: opacity 120ms ease;
}
.group-prompt-trigger:active { opacity: 0.65; }
.group-prompt-trigger:disabled { opacity: 0.4; }
.group-prompt-action {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: var(--secondary-text);
  font-size: 12px;
}

/* 5. 底部弹出抽屉 (Sheets) */
.group-sheet-backdrop {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  justify-content: flex-end;
  background: rgba(0, 0, 0, 0.5);
}
.group-bottom-sheet {
  width: 100%;
  max-width: 48rem;
  max-height: 80vh;
  margin-inline: auto;
  display: flex;
  flex-direction: column;
  border-top: 1px solid var(--border-color);
  border-radius: 16px 16px 0 0;
  background: var(--primary-bg);
  color: var(--primary-text);
  padding-right: var(--vcp-safe-right, 0px);
  padding-bottom: calc(var(--vcp-safe-bottom, 24px) + 8px);
  padding-left: var(--vcp-safe-left, 0px);
  overflow: hidden;
}
.group-sheet-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 14px 16px;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}
.group-sheet-done-btn {
  min-height: 36px;
  padding: 6px 12px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: transparent;
  color: var(--primary-text);
  font-size: 12px;
  font-weight: 500;
  touch-action: manipulation;
}
.group-sheet-done-btn:active { opacity: 0.65; }
.group-sheet-search {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 40px;
  padding: 0 12px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--secondary-bg, transparent);
}
.group-sheet-search-input {
  flex: 1;
  min-width: 0;
  border: 0;
  background: transparent;
  color: var(--primary-text);
  font-size: 14px;
  outline: none;
}
.group-sheet-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 4px 16px 12px;
}
.group-sheet-member-row {
  display: flex;
  align-items: center;
  gap: 12px;
  min-height: 52px;
  padding-block: 6px;
  cursor: pointer;
}
.group-sheet-member-row + .group-sheet-member-row {
  border-top: 1px solid var(--settings-row-divider, var(--border-color));
}
.group-order-badge {
  flex-shrink: 0;
  padding: 2px 6px;
  border: 1px solid var(--highlight-text, #3b82f6);
  border-radius: 6px;
  color: var(--highlight-text, #3b82f6);
  font-size: 12px;
  line-height: 1.2;
}
.group-sheet-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding-top: 12px;
  border-top: 1px solid var(--settings-row-divider, var(--border-color));
}
.group-remove-member-btn {
  min-height: 36px;
  padding: 6px 0;
  border: 0;
  background: transparent;
  color: #ef4444;
  font-size: 12px;
  font-weight: 500;
  touch-action: manipulation;
}
.group-remove-member-btn:active { opacity: 0.65; }

.group-note { margin: 0; font-size: 12px; line-height: 1.6; color: var(--secondary-text); }
.group-text-button { align-self: flex-start; min-height: 44px; padding: 6px 0; border: 0; background: transparent; color: var(--secondary-text); font-size: 12px; text-decoration: underline; touch-action: manipulation; }
.group-text-button:disabled { opacity: 0.5; }
.group-number-input { width: 88px; max-width: 100%; min-height: 44px; padding: 8px; border: 1px solid var(--border-color); border-radius: 8px; background: var(--input-bg, transparent); color: var(--primary-text); font-family: monospace; font-size: var(--settings-input-font-size, 16px); text-align: right; }
.group-number-input:disabled { opacity: 0.5; }
.group-number-input:focus-visible, .group-icon-button:focus-visible, .group-mode-pill:focus-visible, .group-member-main:focus-visible, .group-prompt-trigger:focus-visible { outline: 2px solid var(--highlight-text, var(--primary-text)); outline-offset: 2px; }
</style>
