<script setup lang="ts">
import { ref, watch, nextTick, computed, onMounted, onUnmounted, useId } from 'vue';
import { useChatHistoryStore } from '../../core/stores/chatHistoryStore';
import { useChatSessionStore } from '../../core/stores/chatSessionStore';
import { useShareStore } from '../../core/stores/shareStore';
import { useChatStreamStore } from '../../core/stores/chatStreamStore';
import { useAttachmentStore } from '../../core/stores/attachmentStore';
import { useNotificationStore } from '../../core/stores/notification';
import type { Attachment } from '../../core/types/chat';
import { useOverlayStore } from '../../core/stores/overlay';
import { Phone } from '@lucide/vue';
import { useTarvenStore } from '../../core/stores/tarvenStore';
import { isAndroid, isIOS } from '../../core/utils/platform';
import { useLongTextPaste } from './composables/useLongTextPaste';
import { useModalHistory } from '../../core/composables/useModalHistory';
import { refreshIOSWindowGeometry } from '../../core/composables/useIOSWindowGeometry';
import { attachMenuOpen, toggleAttachMenu } from './attachMenuController';
import StagedAttachmentPreview from './StagedAttachmentPreview.vue';
import GroupStopAllButton from './components/GroupStopAllButton.vue';
import GroupInviteBar from './components/GroupInviteBar.vue';
import VcpAvatar from '../../components/ui/VcpAvatar.vue';
import { useMentionSelector, MENTION_ALL_ID, type MentionMember } from './composables/useMentionSelector';
import { splitMentionSegments } from '../../core/utils/mention';
import NarrationWave from './components/NarrationWave.vue';
import { useBubbleNarration } from './voice/useBubbleNarration';

const tarvenStore = useTarvenStore();

const openTarvenSelector = () => {
  if (props.disabled || props.suspended) return;
  tarvenStore.isSelectorOpen = true;
};

const props = defineProps<{
  disabled?: boolean;
  suspended?: boolean;
}>();

const emit = defineEmits<{
  (e: 'send', content: string, onCommitted: () => void): void;
  (e: 'attach'): void;
  (e: 'toggle-menu', visible: boolean): void;
  (e: 'focus-input'): void;
  (e: 'start-call'): void;
}>();

const input = ref('');

watch(attachMenuOpen, (val) => {
  emit('toggle-menu', val);
});
const historyStore = useChatHistoryStore();
const sessionStore = useChatSessionStore();
const shareStore = useShareStore();
const streamStore = useChatStreamStore();
const attachmentStore = useAttachmentStore();
const notificationStore = useNotificationStore();
const { handlePaste, handleBeforeInput, setProgrammaticInput } = useLongTextPaste(
  input, () => !historyStore.editingMessage && !historyStore.isSubmitting,
);
let inputRevision = 0;
watch(input, () => { inputRevision++; }, { flush: 'sync' });
const overlayStore = useOverlayStore();
const bubbleNarration = useBubbleNarration();
const isNarrating = computed(() => Boolean(bubbleNarration.activeTtsId));
const textareaRef = ref<HTMLTextAreaElement | null>(null);
const rootRef = ref<HTMLElement | null>(null);
const isExpanded = ref(false);
const canExpand = ref(false);
const collapsedHeight = ref(0);
const expandedModalId = `ChatInputExpanded-${useId()}`;
const { registerModal, unregisterModal } = useModalHistory();

const collapseEditor = () => {
  if (!isExpanded.value) return;
  isExpanded.value = false;
  unregisterModal(expandedModalId);
};

const toggleEditor = () => {
  if (isExpanded.value) {
    collapseEditor();
  } else if (canExpand.value && !props.disabled) {
    collapsedHeight.value = rootRef.value?.offsetHeight ?? 0;
    attachMenuOpen.value = false;
    isExpanded.value = true;
    registerModal(expandedModalId, collapseEditor, () => rootRef.value);
  }
};

const handleAttachToggle = () => {
  if (props.suspended || historyStore.isSubmitting) return;
  // 附件选择沿原输入区展开，避免 112px 菜单挤掉键盘上方的编辑/发送区域。
  collapseEditor();
  toggleAttachMenu();
};

watch(() => props.disabled, (disabled) => {
  if (disabled) collapseEditor();
});
watch(() => sessionStore.currentConversationKey?.epoch, collapseEditor);

// 通话切换与卸载只收起瞬时交互，保留草稿和附件。
const suspendComposer = () => {
  attachMenuOpen.value = false;
  collapseEditor();
  dismissMention();
  textareaRef.value?.blur();
};

defineExpose({ suspendComposer });
watch(() => props.suspended, suspended => {
  if (suspended) suspendComposer();
  else void nextTick(autoResize);
}, { flush: 'sync' });

const handleFocus = () => {
  attachMenuOpen.value = false;
  if (!props.disabled && historyStore.currentChatHistory.length > 0) {
    emit('focus-input');
  }
};

// 自动调整输入框高度
const autoResize = () => {
  const el = textareaRef.value;
  if (!el) return;
  // 展开时由 flex/grid 分配可用空间，不能把满高编辑区当作内容行数。
  if (isExpanded.value) {
    el.style.height = '';
    syncHighlightScroll();
    return;
  }
  // 空内容时恢复默认单行高度：多行 placeholder 会在部分 WebView 中撑大
  // scrollHeight，删除文字后留下"幻影换行"
  if (!input.value) {
    el.style.height = '';
    canExpand.value = false;
    return;
  }
  const scrollTop = el.scrollTop;
  el.style.height = 'auto'; // Reset height
  const scrollHeight = el.scrollHeight;
  const metrics = getComputedStyle(el);
  const lineHeight = parseFloat(metrics.lineHeight);
  const padding = parseFloat(metrics.paddingTop) + parseFloat(metrics.paddingBottom);
  // 包括软换行；留 1px 容差给 WebView 对 scrollHeight 的整数取整。
  canExpand.value = scrollHeight - padding > lineHeight * 4 + 1;
  el.style.height = `${scrollHeight}px`;
  el.scrollTop = scrollTop;
};

watch([input, isExpanded], () => {
  nextTick(() => {
    autoResize();
    syncHighlightScroll();
  });
});
watch(isExpanded, () => {
  if (isIOS) void nextTick(refreshIOSWindowGeometry);
});

// 是否正在生成中
const isGenerating = computed(() => streamStore.isGroupGenerating || streamStore.activeStreamingIds.size > 0);
const isPreparing = computed(() => {
  const key = sessionStore.currentConversationKey;
  return !isGenerating.value && !!key && streamStore.isConversationPreparing(key.ownerId, key.ownerType, key.topicId);
});

// 是否有内容可发送
const hasContent = computed(() => input.value.trim() !== '' || attachmentStore.stagedAttachments.length > 0);
const attachmentsReady = computed(() =>
  attachmentStore.stagedAttachments.every(attachment => attachment.status === 'done'),
);
const canSend = computed(() => hasContent.value && attachmentsReady.value && !props.disabled && !props.suspended && !shareStore.sending && !historyStore.isSubmitting && !isPreparing.value);
const actionIsStopNarration = computed(() =>
  !isGenerating.value && isNarrating.value && !hasContent.value && !isPreparing.value && !isExpanded.value
);
const actionIsVoiceCall = computed(() =>
  !hasContent.value && !isGenerating.value && !isPreparing.value && !isExpanded.value && !actionIsStopNarration.value
);

// 编辑重发意图与原会话绑定；切换会话取消意图时同时清理注入内容。
let voiceResendEditor: typeof overlayStore.editorConfig = null;
let voiceResendIntent: typeof historyStore.editingMessage = null;
let inputEditingIntent: typeof historyStore.editingMessage = null;
watch(() => overlayStore.editorConfig, current => {
  if (!voiceResendEditor || current === voiceResendEditor) return;
  voiceResendEditor = null;
  if (historyStore.editingMessage === voiceResendIntent) historyStore.editingMessage = null;
  voiceResendIntent = null;
});
watch(() => historyStore.editingMessage, async (intent, previousIntent) => {
  // Editing remains available during a call without replacing the parked draft
  // or bringing the hidden composer back into focus.
  if (props.suspended) {
    if (intent) {
      // Failed sends restore the same intent. Keep the user's open editor and
      // edited text instead of replacing it with the original message again.
      if (voiceResendEditor && overlayStore.editorConfig === voiceResendEditor) return;
      overlayStore.openEditor({
        initialValue: intent.initialContent,
        onSave: async content => {
          if (historyStore.editingMessage !== intent || !sessionStore.isConversationCurrent(intent.key.conversation)) {
            throw new Error('原消息所在会话已变化，请重新选择消息。');
          }
          if (!attachmentsReady.value) throw new Error('请等待草稿附件处理完成后再重发。');
          await historyStore.sendMessage(content);
          if (historyStore.editingMessage === intent) throw new Error('消息重发未完成，请稍后重试。');
        },
      });
      voiceResendEditor = overlayStore.editorConfig;
      voiceResendIntent = intent;
    }
    return;
  }
  if (intent) {
    attachMenuOpen.value = false;
    inputEditingIntent = intent;
    setProgrammaticInput(intent.initialContent);
    await nextTick();
    if (textareaRef.value && !props.suspended) {
      textareaRef.value.focus();
      textareaRef.value.dispatchEvent(new Event('input', { bubbles: true }));
    }
  } else if (previousIntent && inputEditingIntent === previousIntent) {
    inputEditingIntent = null;
    setProgrammaticInput('');
  }
});

// Park text and attachments together so a late commit can consume the original
// draft even when a newly received share is now displayed in the composer.
let ordinaryDraft: { text: string; attachments: Attachment[]; revision: number } | null = null;
watch(() => shareStore.activeDraft?.deliveryId, async (deliveryId, previous) => {
  if (deliveryId) {
    if (!previous) {
      ordinaryDraft = {
        text: input.value,
        attachments: attachmentStore.stagedAttachments.filter(item => item.status === 'done').map(item => ({ ...item })),
        revision: inputRevision,
      };
      if (attachmentStore.stagedAttachments.some(item => item.status !== 'done')) {
        notificationStore.addNotification({ type: 'info', message: '已取消原输入中尚未完成的附件，已完成附件会在结束分享后保留。', toastOnly: true });
      }
    }
    inputEditingIntent = null;
    attachmentStore.clearStaged();
    attachmentStore.stagedAttachments = (shareStore.activeDraft?.attachments ?? []).map((item, index) => ({ ...item, id: `share_${deliveryId}_${index}` }));
    setProgrammaticInput(shareStore.activeDraft?.text ?? '');
    await nextTick();
    if (textareaRef.value && !props.suspended) {
      textareaRef.value.focus();
      textareaRef.value.dispatchEvent(new Event('input', { bubbles: true }));
    }
  } else if (previous) {
    attachmentStore.clearStaged();
    attachmentStore.stagedAttachments = ordinaryDraft?.attachments ?? [];
    setProgrammaticInput(ordinaryDraft?.text ?? '');
    ordinaryDraft = null;
  }
}, { immediate: true });
watch([input, () => attachmentStore.stagedAttachments], () => {
  shareStore.editDraft(input.value, attachmentStore.stagedAttachments);
}, { deep: true });

const handleSend = async () => {
  if (canSend.value) {
    collapseEditor();
    if (shareStore.activeDraft) {
      await historyStore.sendMessage(input.value);
    } else {
      const revision = inputRevision;
      const conversation = sessionStore.currentConversationKey;
      const attachmentIds = new Set(attachmentStore.stagedAttachments.map(item => item.id));
      emit('send', input.value, () => {
        if (ordinaryDraft) {
          if (ordinaryDraft.revision === revision) ordinaryDraft.text = '';
          ordinaryDraft.attachments = ordinaryDraft.attachments.filter(item => !attachmentIds.has(item.id));
        } else if (inputRevision === revision && sessionStore.isConversationCurrent(conversation)) {
          setProgrammaticInput('');
        }
      });
    }
    attachMenuOpen.value = false;
  }
};

const handleAction = () => {
  if (props.disabled || props.suspended) return;
  if (actionIsStop.value) {
    if (actionIsStopNarration.value) {
      void bubbleNarration.stopBubbleNarration();
      return;
    }
    const activeIds = Array.from(streamStore.activeStreamingIds);
    const key = sessionStore.currentConversationKey;
    if (!key) return;
    if (key.ownerType === "group") {
      void streamStore.stopGroupTurn(key.ownerId, key.topicId);
      return;
    }
    activeIds.forEach(id =>
      streamStore.stopMessage(key.ownerId, key.ownerType, key.topicId, id as string),
    );
  } else {
    handleSend();
  }
};

const handlePrimaryButtonClick = () => {
  if (props.disabled || props.suspended) return;
  if (actionIsVoiceCall.value) {
    emit('start-call');
    return;
  }
  handleAction();
};

// ----------------------------------------------------
// 群聊：@提及 选择器（输入 @ 自动弹出成员浮层）
// ----------------------------------------------------
const {
  isOpen: mentionOpen,
  target: mentionTarget,
  filtered: mentionFiltered,
  members: mentionMembers,
  groupMode: mentionGroupMode,
  updateCursor: updateMentionCursor,
  dismiss: dismissMention,
} = useMentionSelector(input);

// JEV 持续会话允许在成员发言/仲裁时追加消息；无草稿时仍可一键中断全部。
const actionIsStop = computed(() => {
  if (isGenerating.value) {
    return !((streamStore.currentGroupTurn?.mode ?? mentionGroupMode.value) === 'jev'
      && hasContent.value && !historyStore.editingMessage);
  }
  return actionIsStopNarration.value;
});

// 输入法组合态（拼音上屏前）：v-model 此时尚未更新，覆盖层必须全部让位，
// 否则富文本提示/高亮背板会盖住正在组合的拼音串
const isComposing = ref(false);

const isGroupChat = computed(() => {
  const item = sessionStore.currentSelectedItem;
  return item?.type === 'group';
});

const inputPlaceholder = computed(() => {
  if (props.disabled) return '请先选择话题以开启对话';
  // 群聊使用富文本覆盖层提示（可高亮 @），原生 placeholder 留空避免双重渲染
  return isGroupChat.value ? '' : '说点什么...';
});

// 群聊富文本提示：仅纯文本输入态且内容为空、且不在输入法组合中时显示
const showMentionHint = computed(
  () =>
    isGroupChat.value &&
    !props.disabled &&
    !input.value &&
    !isComposing.value,
);

// ----------------------------------------------------
// @提及 标签化高亮：textarea 文字透明，由同版式背板提供可见文本，
// 命中群成员名的 @名字 段渲染为橙色标签（与蓝色 @ 提示组 CP）。
// 组合态下停用：背板不含正在上屏的拼音，避免双渲染/不可见。
// ----------------------------------------------------
const mentionHighlightSegments = computed(() => {
  if (!isGroupChat.value || !input.value) return null;
  const names = mentionMembers.value.map((m) => m.name);
  if (mentionGroupMode.value === 'naturerandom') names.unshift('所有人');
  return splitMentionSegments(input.value, names);
});

const highlightActive = computed(
  () => !isComposing.value && !!mentionHighlightSegments.value?.some((s) => s.mention),
);

const highlightRef = ref<HTMLElement | null>(null);
const syncHighlightScroll = () => {
  if (highlightRef.value && textareaRef.value) {
    highlightRef.value.scrollTop = textareaRef.value.scrollTop;
  }
};

// textarea 没有 DOM caret rect；仅在尺寸改变时测量当前选区端点，
// 不重设选区、不重新 focus，也不打断输入法。普通输入仍交给浏览器滚动。
const revealCursorAfterResize = () => {
  const el = textareaRef.value;
  if (!el || document.activeElement !== el || isComposing.value || !el.value) return;
  const metrics = getComputedStyle(el);
  const mirror = document.createElement('div');
  mirror.style.cssText = `position:fixed;top:0;left:0;visibility:hidden;pointer-events:none;box-sizing:border-box;width:${el.clientWidth}px;`;
  for (const property of ['font-family', 'font-size', 'font-weight', 'font-style', 'line-height', 'letter-spacing',
    'white-space', 'word-break', 'overflow-wrap', 'tab-size', 'text-indent',
    'padding-top', 'padding-right', 'padding-bottom', 'padding-left']) {
    mirror.style.setProperty(property, metrics.getPropertyValue(property));
  }
  const cursor = el.selectionDirection === 'backward' ? el.selectionStart : el.selectionEnd;
  mirror.textContent = el.value.slice(0, cursor);
  const marker = document.createElement('span');
  marker.textContent = el.value.slice(cursor) || '\u200b';
  mirror.append(marker);
  document.body.append(mirror);
  const top = marker.getBoundingClientRect().top - mirror.getBoundingClientRect().top;
  const bottom = top + parseFloat(metrics.lineHeight) + parseFloat(metrics.paddingBottom);
  mirror.remove();
  if (top < el.scrollTop) el.scrollTop = Math.max(0, top - parseFloat(metrics.paddingTop));
  else if (bottom > el.scrollTop + el.clientHeight) el.scrollTop = bottom - el.clientHeight;
  syncHighlightScroll();
};

const syncMentionCursor = (e: Event) => {
  const el = e.target as HTMLTextAreaElement | null;
  if (el) updateMentionCursor(el.selectionStart ?? el.value.length);
};

const handleMentionSelect = (member: MentionMember) => {
  const target = mentionTarget.value;
  if (!target) return;

  // 全模式统一插入 @名字 文本；invite_only 下发送时由 store 自动邀约被提及成员
  const insertText = `@${member.name} `;
  setProgrammaticInput(input.value.slice(0, target.start) + insertText + input.value.slice(target.end));
  dismissMention();
  nextTick(() => {
    const el = textareaRef.value;
    if (el) {
      const pos = target.start + insertText.length;
      el.focus();
      el.setSelectionRange(pos, pos);
      updateMentionCursor(pos);
      autoResize();
    }
  });
};

const handleKeydown = (e: KeyboardEvent) => {
  if (isComposing.value || e.isComposing || e.keyCode === 229) return;
  if (e.key === 'Escape' && mentionOpen.value) {
    e.preventDefault();
    dismissMention();
    return;
  }
  if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
    e.preventDefault();
    handleAction();
  }
};

const triggerFilePick = async (mode: 'camera' | 'gallery' | 'file') => {
  if (props.disabled || historyStore.isSubmitting) return;
  emit('attach');
  await attachmentStore.handleAttachment(mode);
};

const removeStagedAttachment = (index: number) => {
  if (historyStore.isSubmitting) return;
  attachmentStore.removeStaged(index);
};

const handleClickOutside = (event: MouseEvent) => {
  if (attachMenuOpen.value && rootRef.value && !rootRef.value.contains(event.target as Node)) {
    attachMenuOpen.value = false;
  }
};

// 观察实际文本宽度，覆盖软换行、侧栏布局、旋转与通话退出后的输入区恢复。
let textareaObserver: ResizeObserver | null = null;
let measuredWidth = -1;
let measuredHeight = -1;
let measuredLineHeight = '';
let measuredBoxWidth = -1;
let measuredBoxHeight = -1;
watch(textareaRef, (el, previous) => {
  if (previous) textareaObserver?.unobserve(previous);
  measuredWidth = -1;
  measuredHeight = -1;
  if (el) {
    textareaObserver?.observe(el);
    autoResize();
  }
}, { flush: 'post' });

onMounted(() => {
  document.addEventListener('click', handleClickOutside, true);
  if (typeof ResizeObserver !== 'undefined') {
    textareaObserver = new ResizeObserver(() => {
      const el = textareaRef.value;
      if (!el) return;
      if (isIOS) {
        // Observe the composer slot, not the constrained editor height. Keyboard
        // avoidance preserves this slot; attachments/expansion may resize it.
        const box = el.closest<HTMLElement>('.input-box');
        if (box && (box.clientWidth !== measuredBoxWidth || box.clientHeight !== measuredBoxHeight)) {
          measuredBoxWidth = box.clientWidth;
          measuredBoxHeight = box.clientHeight;
          refreshIOSWindowGeometry();
        }
      }
      const width = el.clientWidth;
      const height = el.clientHeight;
      const lineHeight = getComputedStyle(el).lineHeight;
      const geometryChanged = width !== measuredWidth || height !== measuredHeight || lineHeight !== measuredLineHeight;
      if (width !== measuredWidth || lineHeight !== measuredLineHeight) {
        measuredWidth = width;
        measuredLineHeight = lineHeight;
        autoResize();
      }
      measuredHeight = el.clientHeight;
      if (geometryChanged) revealCursorAfterResize();
      syncHighlightScroll();
    });
    if (textareaRef.value) textareaObserver.observe(textareaRef.value);
    if (isIOS) {
      const box = textareaRef.value?.closest('.input-box');
      if (box) textareaObserver.observe(box);
    }
  }
  autoResize();
});

onUnmounted(() => {
  suspendComposer();
  textareaObserver?.disconnect();
  unregisterModal(expandedModalId);
  document.removeEventListener('click', handleClickOutside, true);
});
</script>

<template>
  <div :style="isExpanded ? { height: `${collapsedHeight}px` } : undefined">
  <!-- 与编辑区保留在同一 stacking context；不搬动正在组合输入的 textarea。 -->
  <div v-if="isExpanded" class="vcp-viewport-surface fixed inset-0 z-sheet bg-black/40 no-swipe"
    aria-hidden="true" @click.stop="collapseEditor" @touchmove.prevent></div>
  <div ref="rootRef" class="input-enhancer px-1 py-1 w-full transition-opacity duration-300 no-swipe relative flex flex-col gap-1.5"
    :class="{ 'opacity-70 pointer-events-none': disabled, 'input-expanded vcp-viewport-bottom z-sheet': isExpanded }"
    :role="isExpanded ? 'dialog' : undefined" :aria-modal="isExpanded ? true : undefined"
    :aria-label="isExpanded ? '编辑消息' : undefined">
    <!-- 展开时由底部同一停止操作承接，避免重复按钮悬出编辑外壳。 -->
    <GroupStopAllButton v-if="!isExpanded" />

    <div class="input-context">
    <!-- 编辑重发状态横条 -->
    <div
      v-if="historyStore.editingMessage && !isExpanded"
      class="editing-message-bar flex items-center justify-between px-3 py-1.5 mb-2 mx-1 rounded-xl bg-blue-500/10 border border-blue-500/20 text-xs text-blue-500 dark:text-blue-400"
    >
      <div class="flex items-center gap-1.5 min-w-0">
        <span class="i-heroicons-pencil-square text-sm shrink-0"></span>
        <span class="truncate font-medium">正在编辑重发历史消息</span>
      </div>
      <button
        type="button"
        @click="historyStore.cancelEditResend()"
        class="shrink-0 ml-2 px-2 py-0.5 rounded-md hover:bg-blue-500/15 active:scale-95 transition-all text-blue-500/80 hover:text-blue-500 font-medium"
      >
        取消
      </button>
    </div>

    <!-- 群组邀请与 JEV 会话控制 -->
    <GroupInviteBar />

    <!-- 暂存附件预览区 -->
    <div v-if="attachmentStore.stagedAttachments.length > 0" class="input-attachments flex items-center gap-2 mb-2 px-2 overflow-x-auto pb-1 pt-2">
      <TransitionGroup name="list">
        <StagedAttachmentPreview 
          v-for="(file, idx) in attachmentStore.stagedAttachments" 
          :key="file.id || idx" 
          :file="file" 
          :index="idx"
          @remove="removeStagedAttachment"
        />
      </TransitionGroup>
    </div>
    </div>

    <!-- 框体主容器 -->
    <div class="input-shell flex items-end gap-2 px-1">
      <div class="input-box flex-1 min-w-0 flex items-end gap-1.5 bg-[var(--secondary-bg)] border border-[var(--border-color)] rounded-2xl px-2 py-1 shadow-sm relative overflow-visible"
      >
        <span v-if="isExpanded" class="input-editor-title text-sm text-[var(--primary-text)] opacity-60">编辑消息</span>
        <button v-if="canExpand || isExpanded"
          type="button" class="input-expand-toggle absolute right-1 top-1 z-local w-11 h-11 flex items-center justify-center rounded-lg text-[var(--primary-text)] opacity-60 active:opacity-100"
          :aria-label="isExpanded ? '缩小输入框' : '展开输入框'" :aria-expanded="isExpanded"
          @pointerdown.prevent @mousedown.prevent @click="toggleEditor">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path v-if="isExpanded" d="M4 9h5V4M9 9 3 3m17 12h-5v5m0-5 6 6" />
            <path v-else d="M15 3h6v6m0-6-7 7M9 21H3v-6m0 6 7-7" />
          </svg>
        </button>
        
        <!-- 文字输入与提及交互区 -->
        <div data-vcp-keyboard-editor class="input-writing-area flex-1 min-w-0 flex flex-col justify-end relative min-h-[36px] py-[1px] overflow-visible">

          <!-- @提及 成员选择浮层（锚定于输入框上方，z-local 局部悬浮） -->
          <Transition name="fade">
            <div
              v-if="mentionOpen"
              class="input-mention-menu absolute bottom-full left-1 right-1 mb-1.5 z-local bg-[var(--secondary-bg)] border border-[var(--border-color)] rounded-xl shadow-lg overflow-hidden"
            >
              <div class="px-3 pt-2 pb-1 text-[10px] font-bold uppercase tracking-wider text-primary-text opacity-40 select-none">
                提及成员
              </div>
              <div class="min-h-0 max-h-44 overflow-y-auto pb-1">
                <button
                  v-for="member in mentionFiltered"
                  :key="member.id"
                  class="w-full flex items-center gap-2.5 px-3 py-2 text-left active:bg-blue-500/10 transition-colors"
                  @mousedown.prevent
                  @click="handleMentionSelect(member)"
                >
                  <div
                    v-if="member.id === MENTION_ALL_ID"
                    class="w-6 h-6 shrink-0 rounded-full bg-blue-500/15 text-blue-500 flex items-center justify-center text-[12px] font-bold"
                  >@</div>
                  <VcpAvatar
                    v-else
                    owner-type="agent"
                    :owner-id="member.id"
                    :fallback-name="member.name"
                    size="w-6 h-6"
                    rounded="rounded-full"
                  />
                  <span class="text-sm font-semibold text-primary-text">{{ member.name }}</span>
                  <span
                    v-if="member.id === MENTION_ALL_ID"
                    class="ml-auto text-[10px] text-primary-text opacity-40"
                  >全员发言</span>
                </button>
              </div>
            </div>
          </Transition>

          <!-- @提及 高亮背板：与 textarea 完全同版式（字体/行高/内边距/换行规则），
               textarea 文字透明时由它提供全部可见文本，@名字 段渲染橙色标签。
               不加任何水平 padding/margin，保证与光标位置逐像素对齐 -->
          <div
            v-if="highlightActive"
            ref="highlightRef"
            aria-hidden="true"
            class="vcp-input-text-metrics absolute left-0 right-0 top-[1px] bottom-[1px] py-[8px] text-[15px] leading-[1.25] whitespace-pre-wrap break-words overflow-hidden pointer-events-none select-none text-[var(--primary-text)]"
          ><template v-for="(seg, i) in mentionHighlightSegments" :key="i"><span v-if="seg.mention" class="text-orange-500 bg-orange-500/15 rounded-sm font-semibold">{{ seg.text }}</span><template v-else>{{ seg.text }}</template></template>&#8203;</div>

          <!-- 常驻文字输入节点，保留输入法组合态与选区 -->
          <textarea
            ref="textareaRef"
            v-model="input"
            @focus="handleFocus"
            @keydown="handleKeydown"
            @input="syncMentionCursor"
            @keyup="syncMentionCursor"
            @click="syncMentionCursor"
            @scroll="syncHighlightScroll"
            @compositionstart="isComposing = true"
            @compositionend="isComposing = false"
            @paste="handlePaste"
            @beforeinput="handleBeforeInput"
            rows="1"
            class="w-full bg-transparent border-none focus:outline-none focus:ring-0 text-[15px] placeholder-opacity-40 resize-none leading-[1.25] py-[8px] scrollbar-hide vcp-textarea relative"
            :class="highlightActive ? 'text-transparent [caret-color:var(--primary-text)]' : 'text-[var(--primary-text)]'"
            aria-label="消息内容"
            :placeholder="inputPlaceholder"
            :disabled="disabled || historyStore.isSubmitting"
          ></textarea>
          
          <div v-if="!isExpanded" class="absolute top-0 left-0 right-0 h-4 pointer-events-none bg-[var(--secondary-bg)] opacity-90"
            style="mask-image: linear-gradient(to bottom, black, transparent); -webkit-mask-image: linear-gradient(to bottom, black, transparent);"></div>

          <!-- 群聊富文本占位提示：高亮 @，内容为空时显示（替代原生 placeholder） -->
          <div
            v-if="showMentionHint"
            class="absolute left-0 right-0 top-[9px] pointer-events-none select-none text-[15px] leading-[1.25] text-[var(--primary-text)] whitespace-nowrap overflow-hidden"
          >
            <span class="opacity-40">输入 </span><span class="text-blue-500 font-bold">@</span><span class="opacity-40"> 提及成员或 Tag …</span>
          </div>
        </div>

        <!-- 右侧操作区：展开编辑时仅保留发送/中止按钮。 -->
        <div class="input-actions flex items-center shrink-0 mb-0.5 relative gap-1.5">
          <!-- 展开附件按钮 -->
          <button
            v-if="!isExpanded"
            v-longpress="openTarvenSelector"
            v-guide="'chat-plus-button'"
            @click="handleAttachToggle"
            aria-label="添加附件"
            class="w-9 h-9 flex items-center justify-center rounded-full hover:bg-black/5 dark:hover:bg-white/5 text-[var(--primary-text)] opacity-80 hover:opacity-100 active:scale-90 transition-all relative"
          >
            <div class="i-heroicons-plus-circle text-2xl transition-transform duration-300 ease-out" :class="{ 'rotate-45': attachMenuOpen }"></div>
            <!-- 当有激活规则时显示绿色指示点 -->
            <div v-if="tarvenStore.rules.some(r => r.isEnabled)" 
              class="absolute top-1.5 right-1.5 w-2 h-2 bg-emerald-500 rounded-full border-2 border-[var(--secondary-bg)] shadow-[0_0_8px_rgba(16,185,129,0.5)]">
            </div>
          </button>

          <!-- 通话 / 发送 / 中止 形态切换主按钮 -->
          <button
            type="button"
            @click="handlePrimaryButtonClick"
            :aria-label="actionIsVoiceCall ? '开始语音通话' : actionIsStopNarration ? '停止朗读' : actionIsStop ? '停止生成' : isPreparing ? '正在准备消息' : '发送消息'"
            :disabled="actionIsVoiceCall ? (disabled || suspended) : (!actionIsStop && !canSend)"
            class="primary-action-btn w-8 h-8 flex items-center justify-center rounded-full shadow-sm active:scale-95 relative overflow-hidden shrink-0"
            :class="{
              'is-voice': actionIsVoiceCall,
              'is-stop': actionIsStop,
              'is-send': !actionIsVoiceCall && !actionIsStop,
              'opacity-40': actionIsVoiceCall ? (disabled || suspended) : (!actionIsStop && !canSend),
            }"
          >
            <span class="primary-action-fill" aria-hidden="true"></span>

            <!-- 语音通话声波 SVG（4 柱圆角声波） -->
            <svg
              class="primary-action-icon voice-wave-svg"
              :class="{ 'is-active': actionIsVoiceCall }"
              width="20"
              height="20"
              viewBox="0 0 24 24"
              fill="none"
              aria-hidden="true"
            >
              <rect class="voice-wave-bar bar-1" x="5.8" y="8.8" width="2.2" height="6.4" rx="1.1" fill="currentColor" />
              <rect class="voice-wave-bar bar-2" x="9.2" y="5.2" width="2.2" height="13.6" rx="1.1" fill="currentColor" />
              <rect class="voice-wave-bar bar-3" x="12.6" y="7.2" width="2.2" height="9.6" rx="1.1" fill="currentColor" />
              <rect class="voice-wave-bar bar-4" x="16.0" y="9.4" width="2.2" height="5.2" rx="1.1" fill="currentColor" />
            </svg>

            <!-- 发送 / 准备 / 停止 图标层 -->
            <span
              class="primary-action-icon send-state-icon"
              :class="{ 'is-active': !actionIsVoiceCall }"
              aria-hidden="true"
            >
              <span v-if="actionIsStop" class="i-heroicons-stop-16-solid text-lg block"></span>
              <span v-else-if="isPreparing" class="i-heroicons-ellipsis-horizontal text-lg block"></span>
              <span v-else class="i-heroicons-paper-airplane text-[15px] block -rotate-45 translate-x-0.2 -translate-y-0.2"></span>
            </span>
          </button>
        </div>

        <!-- 朗读时的彩虹音波跃动效果 -->
        <Transition name="wave-flow">
          <div
            v-if="isNarrating"
            class="absolute left-3 right-3 bottom-0 h-[5px] pointer-events-none z-10 overflow-hidden"
          >
            <NarrationWave :active="isNarrating" />
          </div>
        </Transition>
      </div>
    </div>

    <!-- 往上平滑弹起的百搭扩展面板 -->
    <div
      v-guide="'chat-attach-menu'"
      class="overflow-hidden transition-all duration-300 ease-[cubic-bezier(0.34,1.56,0.64,1)]"
      :class="attachMenuOpen ? 'opacity-100' : 'opacity-0 pointer-events-none'"
      :style="{ height: attachMenuOpen ? '112px' : '0px' }"
    >
      <div 
        class="w-full h-full border-t border-[var(--border-color)]/20 pt-3 pb-2 flex justify-around items-center transition-all duration-300"
        :class="attachMenuOpen ? 'translate-y-0 opacity-100 scale-100' : 'translate-y-4 opacity-0 scale-95'"
      >
        <!-- 拍摄按钮 -->
        <button v-if="isAndroid || isIOS" @click="triggerFilePick('camera')"
          class="group flex flex-col items-center gap-1.5 active:scale-95 transition-all outline-none"
        >
          <div class="w-13 h-13 flex items-center justify-center rounded-2xl bg-black/5 dark:bg-white/5 border border-[var(--border-color)]/30 group-hover:border-[var(--highlight-text)]/40 transition-all shadow-inner">
            <div class="i-heroicons-camera text-2xl text-blue-500/80 group-hover:text-blue-500 group-hover:scale-105 transition-all"></div>
          </div>
          <span class="text-[11px] font-semibold text-[var(--primary-text)]/70 group-hover:text-[var(--primary-text)] transition-colors">拍摄</span>
        </button>

        <!-- 相册按钮 -->
        <button @click="triggerFilePick('gallery')" 
          class="group flex flex-col items-center gap-1.5 active:scale-95 transition-all outline-none"
        >
          <div class="w-13 h-13 flex items-center justify-center rounded-2xl bg-black/5 dark:bg-white/5 border border-[var(--border-color)]/30 group-hover:border-[var(--highlight-text)]/40 transition-all shadow-inner">
            <div class="i-heroicons-photo text-2xl text-purple-500/80 group-hover:text-purple-500 group-hover:scale-105 transition-all"></div>
          </div>
          <span class="text-[11px] font-semibold text-[var(--primary-text)]/70 group-hover:text-[var(--primary-text)] transition-colors">相册</span>
        </button>

        <!-- 文件按钮 -->
        <button @click="triggerFilePick('file')" 
          class="group flex flex-col items-center gap-1.5 active:scale-95 transition-all outline-none"
        >
          <div class="w-13 h-13 flex items-center justify-center rounded-2xl bg-black/5 dark:bg-white/5 border border-[var(--border-color)]/30 group-hover:border-[var(--highlight-text)]/40 transition-all shadow-inner">
            <div class="i-heroicons-document-text text-2xl text-orange-500/80 group-hover:text-orange-500 group-hover:scale-105 transition-all"></div>
          </div>
          <span class="text-[11px] font-semibold text-[var(--primary-text)]/70 group-hover:text-[var(--primary-text)] transition-colors">文件</span>
        </button>

        <button type="button" aria-label="开始语音通话" :disabled="disabled || suspended"
          @click="emit('start-call')"
          class="group flex flex-col items-center gap-1.5 transition-opacity active:opacity-60 disabled:opacity-40">
          <span class="w-13 h-13 flex items-center justify-center rounded-2xl bg-black/5 dark:bg-white/5 border border-[var(--border-color)]/30">
            <Phone :size="24" :stroke-width="1.8" class="text-emerald-500/80 group-hover:text-emerald-500 transition-colors" aria-hidden="true" />
          </span>
          <span class="text-[11px] font-semibold text-[var(--primary-text)]/70">通话</span>
        </button>
      </div>
    </div>
  </div>
  </div>
</template>

<style scoped>
/* 朗读彩虹音波淡入淡出 */
.wave-flow-enter-active,
.wave-flow-leave-active {
  transition: opacity 0.28s cubic-bezier(0.22, 1, 0.36, 1), transform 0.28s cubic-bezier(0.22, 1, 0.36, 1);
}

.wave-flow-enter-from,
.wave-flow-leave-to {
  opacity: 0;
  transform: translateY(2px) scaleY(0.7);
}

/* 隐藏滚动条 */
.overflow-x-auto {
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.overflow-x-auto::-webkit-scrollbar {
  display: none;
}

.scrollbar-hide::-webkit-scrollbar {
  display: none;
}
.scrollbar-hide {
  -ms-overflow-style: none;
  scrollbar-width: none;
}

/* 优化 Android WebView 中 textarea 的点击与 focus 行为 */
.vcp-textarea {
  max-height: 114px;
  touch-action: manipulation;
  -webkit-tap-highlight-color: transparent;
  cursor: text;
}

/* 同一编辑节点原地展开；Android IME 净增量与 iOS 可用矩形只各消费一次。 */
.input-expanded {
  position: fixed;
  left: 0;
  right: 0;
  bottom: var(--vcp-ime-offset, 0px);
  height: min(80vh, calc(var(--vcp-viewport-height, 100%) - var(--vcp-safe-top, 24px) - var(--vcp-ime-offset, 0px) - 12px));
  min-height: 0;
  box-sizing: border-box;
  padding: 8px calc(var(--vcp-safe-right, 0px) + 20px) calc(var(--vcp-safe-bottom, 48px) + 10px) calc(var(--vcp-safe-left, 0px) + 20px);
  border-top: 1px solid var(--border-color);
  border-radius: 24px 24px 0 0;
  background: var(--secondary-bg);
}

.input-enhancer.input-expanded {
  overflow: hidden;
}

.input-expanded > :not(.input-shell):not(.input-context) {
  flex-shrink: 0;
}

.input-context {
  display: contents;
}

.input-expanded .input-context {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-height: 0;
  max-height: 25%;
  overflow-y: auto;
}

.input-expanded .input-context > * {
  flex-shrink: 0;
}

.input-expanded .input-context:empty {
  display: none;
}

.input-expanded .input-shell {
  flex: 1;
  min-height: 0;
}

.input-expanded .input-box {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  grid-template-rows: 44px minmax(0, 1fr) 44px;
  height: 100%;
  min-height: 0;
  width: 100%;
  border-color: transparent;
  border-radius: 0;
  box-shadow: none;
}

.input-editor-title {
  grid-area: 1 / 1;
  align-self: center;
}

.input-expanded .input-writing-area {
  grid-area: 2 / 1 / 3 / -1;
  height: 100%;
  min-height: 0;
}

.input-expanded .vcp-textarea {
  height: 100%;
  max-height: none;
  min-height: 0;
  overflow-y: auto;
  overscroll-behavior: contain;
}

/* The geometry owner keeps before + height + after equal to this grid/flex
 * slot. Text, selection, highlight and editor decorations move together. */
.input-writing-area[data-vcp-floating-editor] {
  box-sizing: border-box;
  height: var(--vcp-editor-height);
  min-height: 0;
  max-height: var(--vcp-editor-height);
  margin-top: var(--vcp-editor-before);
  margin-bottom: var(--vcp-editor-after);
  justify-content: flex-start;
}

.input-writing-area[data-vcp-floating-editor] .vcp-textarea {
  flex: 1;
  height: 100%;
  min-height: 0;
  max-height: 100%;
}

.input-expanded .input-actions {
  grid-area: 3 / 2;
  justify-self: end;
}

.input-expanded .input-mention-menu {
  bottom: 0;
  max-height: 100%;
  display: flex;
  flex-direction: column;
}

/* 矮窗口给正文让出标题行；收起按钮仍在右上，不覆盖文字。 */
@media (max-height: 500px) {
  .input-expanded .input-box {
    grid-template-rows: minmax(0, 1fr) 44px;
  }
  .input-expanded .input-editor-title {
    display: none;
  }
  .input-expanded .input-writing-area {
    grid-area: 1 / 1 / 2 / -1;
    margin-right: 44px;
  }
  .input-expanded .input-actions {
    grid-row: 2;
  }
}

/* 通话 / 发送 / 停止 常驻主按钮：高对比底色 + 同心圆充能过渡 */
.primary-action-btn {
  background-color: var(--primary-text);
  color: var(--secondary-bg);
  transition: transform 150ms cubic-bezier(0.22, 1, 0.36, 1), opacity 180ms ease;
}

.primary-action-fill {
  position: absolute;
  inset: 0;
  border-radius: 9999px;
  background-color: #3b82f6;
  transform: scale(0.2);
  opacity: 0;
  pointer-events: none;
  transition:
    transform 200ms cubic-bezier(0.22, 1, 0.36, 1),
    opacity 150ms ease,
    background-color 180ms ease;
}

.primary-action-btn.is-send .primary-action-fill {
  transform: scale(1);
  opacity: 1;
  background-color: #3b82f6;
}

.primary-action-btn.is-stop .primary-action-fill {
  transform: scale(1);
  opacity: 1;
  background-color: #ef4444;
}

.primary-action-icon {
  position: relative;
  z-index: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  transition:
    transform 180ms cubic-bezier(0.22, 1, 0.36, 1),
    opacity 140ms ease;
}

.voice-wave-svg {
  color: var(--secondary-bg);
}

.voice-wave-svg:not(.is-active) {
  opacity: 0;
  transform: scale(0.6);
  pointer-events: none;
}

.voice-wave-svg.is-active {
  opacity: 1;
  transform: scale(1);
}

.voice-wave-bar {
  transform-origin: center 12px;
  transition: transform 220ms cubic-bezier(0.34, 1.56, 0.64, 1);
}

.voice-wave-svg:not(.is-active) .voice-wave-bar {
  transform: scaleY(0.28);
}

.voice-wave-svg.is-active .bar-1 { transition-delay: 0ms; }
.voice-wave-svg.is-active .bar-2 { transition-delay: 18ms; }
.voice-wave-svg.is-active .bar-3 { transition-delay: 36ms; }
.voice-wave-svg.is-active .bar-4 { transition-delay: 54ms; }

.send-state-icon {
  position: absolute;
  inset: 0;
  color: #ffffff;
}

.send-state-icon:not(.is-active) {
  opacity: 0;
  transform: translate(-2px, 2px) scale(0.55);
  pointer-events: none;
}

.send-state-icon.is-active {
  opacity: 1;
  transform: translate(0, 0) scale(1);
}

@media (prefers-reduced-motion: reduce) {
  .primary-action-btn,
  .primary-action-fill,
  .primary-action-icon,
  .voice-wave-bar {
    transition-duration: 0ms !important;
    transition-delay: 0ms !important;
  }
}

</style>
