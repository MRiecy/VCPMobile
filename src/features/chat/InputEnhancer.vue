<script setup lang="ts">
import { ref, watch, nextTick, computed, onMounted, onUnmounted, useId } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useChatHistoryStore } from '../../core/stores/chatHistoryStore';
import { useChatSessionStore } from '../../core/stores/chatSessionStore';
import { useShareStore } from '../../core/stores/shareStore';
import { useChatStreamStore } from '../../core/stores/chatStreamStore';
import { useAttachmentStore } from '../../core/stores/attachmentStore';
import { useNotificationStore } from '../../core/stores/notification';
import { useOverlayStore } from '../../core/stores/overlay';
import { Phone } from 'lucide-vue-next';
import { useTarvenStore } from '../../core/stores/tarvenStore';
import { isAndroid, isIOS } from '../../core/utils/platform';
import { useLongTextPaste } from './composables/useLongTextPaste';
import { useSpeechRecognition } from '../../core/composables/useSpeechRecognition';
import { useAudioRecorder } from '../../core/composables/useAudioRecorder';
import { useModalHistory } from '../../core/composables/useModalHistory';
import { attachMenuOpen, toggleAttachMenu } from './attachMenuController';
import StagedAttachmentPreview from './StagedAttachmentPreview.vue';
import GroupStopAllButton from './components/GroupStopAllButton.vue';
import GroupInviteBar from './components/GroupInviteBar.vue';
import VcpAvatar from '../../components/ui/VcpAvatar.vue';
import { useMentionSelector, MENTION_ALL_ID, type MentionMember } from './composables/useMentionSelector';
import { splitMentionSegments } from '../../core/utils/mention';
import type { AttachmentData } from '../../core/types/chat';

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
  (e: 'send', content: string): void;
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
const overlayStore = useOverlayStore();
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
  if (props.suspended) return;
  // 附件选择沿原输入区展开，避免 112px 菜单挤掉键盘上方的编辑/发送区域。
  collapseEditor();
  toggleAttachMenu();
};

watch(() => props.disabled, (disabled) => {
  if (disabled) collapseEditor();
});
watch(() => sessionStore.currentConversationKey?.epoch, collapseEditor);

// ----------------------------------------------------
// 语音模块：实时识别 (STT) 与 录音附件 Hook 联动
// ----------------------------------------------------
const {
  transcriptionResult,
  startListening,
  stopListening,
  cancelListening
} = useSpeechRecognition();

const {
  recordingDuration,
  startRecording,
  stopRecording,
  cancelRecording
} = useAudioRecorder();

// 状态控制器
const isAudioMode = ref(false);            // 是否切换为经典的“按住 说话”模式
const isSTTActive = ref(false);             // 语音转文字（点击切换后的按住模式）是否激活
const isLongPressRecording = ref(false);   // 直接长按语音按钮录制音频附件是否激活
const isSwipeCancel = ref(false);          // 手势是否判定为上滑取消

let touchStartY = 0;
let iconTouchStartTime = 0;
let isIconLongPress = false;
let iconLongPressTimeout: number | null = null;
let voiceInputGeneration = 0;

const prepareForVoice = () => {
  voiceInputGeneration += 1;
  if (iconLongPressTimeout !== null) window.clearTimeout(iconLongPressTimeout);
  iconLongPressTimeout = null;
  cancelListening();
  cancelRecording();
  isSTTActive.value = false;
  isLongPressRecording.value = false;
  isIconLongPress = false;
  isSwipeCancel.value = false;
  attachMenuOpen.value = false;
  collapseEditor();
  dismissMention();
  textareaRef.value?.blur();
};

defineExpose({ prepareForVoice });
watch(() => props.suspended, suspended => {
  if (suspended) prepareForVoice();
  else void nextTick(autoResize);
}, { flush: 'sync' });

const explainVoiceUnavailable = () => {
  notificationStore.addNotification({
    type: 'info',
    title: '语音输入尚未开发',
    message: '当前 iOS 版本尚未提供语音输入，可继续输入文字。',
    toastOnly: true,
  });
};

const handleVoiceClick = () => {
  // touchstart.prevent 阻止触摸合成 click；独立指针、键盘和辅助技术仍可查看说明。
  if (!props.disabled && !props.suspended && isIOS) explainVoiceUnavailable();
};

// 切换语音模式与普通文本模式
const toggleAudioMode = () => {
  if (props.disabled || props.suspended || !isAndroid) return;
  collapseEditor();
  
  attachMenuOpen.value = false;
  
  // 如果当前正处于某种语音输入状态中，先彻底释放
  if (isSTTActive.value) {
    cancelListening();
    isSTTActive.value = false;
  }
  if (isLongPressRecording.value) {
    cancelRecording();
    isLongPressRecording.value = false;
  }
  
  isAudioMode.value = !isAudioMode.value;
  isSwipeCancel.value = false;
  
  if (!isAudioMode.value) {
    nextTick(() => {
      if (textareaRef.value && !props.suspended) {
        textareaRef.value.focus();
        autoResize();
      }
    });
  }
};

// ----------------------------------------------------
// 交互 A：在“按住 说话”大长条上的 STT 手势管理
// ----------------------------------------------------
const handleSTTTouchStart = async (e: TouchEvent) => {
  if (props.disabled || props.suspended || !isAndroid) return;
  const generation = voiceInputGeneration;
  if (e.cancelable) e.preventDefault(); // 阻断默认上下文菜单与滚动

  isSTTActive.value = true;
  isSwipeCancel.value = false;
  touchStartY = e.touches[0].clientY;

  try {
    await startListening((_text) => {
      // 实时流式回调更新 transcriptionResult
    });
    if (generation !== voiceInputGeneration) cancelListening();
  } catch (err: any) {
    if (generation !== voiceInputGeneration) return;
    isSTTActive.value = false;
    notificationStore.addNotification({
      type: 'warning',
      title: '语音转写启动失败',
      message: err.message || String(err),
      toastOnly: true,
    });
  }
};

const handleSTTTouchMove = (e: TouchEvent) => {
  if (!isSTTActive.value) return;
  const currentY = e.touches[0].clientY;
  const diffY = currentY - touchStartY;

  if (diffY < -60) {
    if (!isSwipeCancel.value) {
      isSwipeCancel.value = true;
    }
  } else {
    isSwipeCancel.value = false;
  }
};

const handleSTTTouchEnd = async (e: TouchEvent) => {
  if (e.cancelable) e.preventDefault();
  if (!isSTTActive.value) return;

  isSTTActive.value = false;

  if (isSwipeCancel.value) {
    cancelListening();
    notificationStore.addNotification({
      type: 'warning',
      title: '已取消转写',
      message: '上滑取消操作已完成',
      toastOnly: true,
    });
  } else {
    const generation = voiceInputGeneration;
    const recognizedText = await stopListening();
    if (generation !== voiceInputGeneration || props.suspended) return;
    if (recognizedText && !recognizedText.startsWith('[')) {
      input.value += recognizedText;
      
      // 自动切回键盘并聚焦，方便用户在键盘上微调！
      isAudioMode.value = false;
      await nextTick();
      if (textareaRef.value) {
        textareaRef.value.focus();
        autoResize();
      }
    }
  }
  isSwipeCancel.value = false;
};

const handleSTTTouchCancel = (e: TouchEvent) => {
  if (e.cancelable) e.preventDefault();
  if (isSTTActive.value) {
    cancelListening();
    isSTTActive.value = false;
    isSwipeCancel.value = false;
  }
};

// ----------------------------------------------------
// 交互 B：直接长按左侧语音按钮，进行附件录制并松手直发
// ----------------------------------------------------
const handleIconTouchStart = (e: TouchEvent) => {
  if (props.disabled || props.suspended) return;
  if (isIOS) {
    explainVoiceUnavailable();
    return;
  }
  if (!isAndroid) return;
  
  // 核心修复：如果当前已经处于“按住说话”模式，点击此按钮是为了切回键盘，直接触发 Tap 切换！
  // 彻底解决了由于 touchstart.prevent 导致 click 无法触发的“点不回去”手势缺陷！
  if (isAudioMode.value) {
    if (e.cancelable) e.preventDefault();
    toggleAudioMode();
    return;
  }
  
  if (e.cancelable) e.preventDefault(); // 阻止 WebView 的震动和菜单弹起

  iconTouchStartTime = Date.now();
  isIconLongPress = false;
  isSwipeCancel.value = false;
  touchStartY = e.touches[0].clientY;

  iconLongPressTimeout = window.setTimeout(async () => {
    const generation = voiceInputGeneration;
    collapseEditor();
    isIconLongPress = true;
    isLongPressRecording.value = true;
    try {
      await startRecording();
      if (generation !== voiceInputGeneration) cancelRecording();
    } catch (err: any) {
      if (generation !== voiceInputGeneration) return;
      isLongPressRecording.value = false;
      isIconLongPress = false;
    }
  }, 350);
};

const handleIconTouchMove = (e: TouchEvent) => {
  if (!isIconLongPress || !isLongPressRecording.value) return;
  const currentY = e.touches[0].clientY;
  const diffY = currentY - touchStartY;

  if (diffY < -60) {
    if (!isSwipeCancel.value) {
      isSwipeCancel.value = true;
    }
  } else {
    isSwipeCancel.value = false;
  }
};

const handleIconTouchEnd = async (e: TouchEvent) => {
  if (e.cancelable) e.preventDefault();
  if (!isAndroid || props.suspended) return;
  
  if (iconLongPressTimeout) {
    clearTimeout(iconLongPressTimeout);
    iconLongPressTimeout = null;
  }

  const duration = Date.now() - iconTouchStartTime;

  if (!isIconLongPress && duration < 350) {
    // 判定为 Tap：直接切换为语音模式
    toggleAudioMode();
  } else if (isLongPressRecording.value) {
    isLongPressRecording.value = false;
    
    if (isSwipeCancel.value) {
      cancelRecording();
      notificationStore.addNotification({
        type: 'warning',
        title: '已取消录音',
        message: '上滑取消操作已完成',
        toastOnly: true,
      });
    } else {
      const generation = voiceInputGeneration;
      const voiceConversation = sessionStore.currentConversationKey;
      const result = await stopRecording();
      if (generation !== voiceInputGeneration || props.suspended) return;
      if (result) {
        try {
          const originalName = `Voice_${Date.now()}.webm`;
          // 保存音频至后端物理沙盒
          const finalData = await invoke<AttachmentData>('store_file', {
            originalName,
            fileBytes: result.bytes,
            mimeType: result.blob.type || 'audio/webm',
          });

          if (finalData) {
            if (generation !== voiceInputGeneration || props.suspended) return;
            if (!sessionStore.isConversationCurrent(voiceConversation)) {
              notificationStore.addNotification({
                type: 'warning',
                title: '语音未发送',
                message: '录音处理期间会话已切换，请在目标会话重新录制。',
                toastOnly: true,
              });
            } else {
              // 塞入 stagedAttachments
              attachmentStore.stagedAttachments.unshift({
                id: `att_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`,
                type: finalData.type,
                src: finalData.internalPath,
                name: finalData.name,
                size: finalData.size,
                hash: finalData.hash,
                status: 'done',
              });

              // 松手直接以附件形式发送
              await nextTick();
              handleSend();
            }
          }
        } catch (err: any) {
          console.error('[InputEnhancer] Direct send voice failed:', err);
          notificationStore.addNotification({
            type: 'warning',
            title: '语音发送异常',
            message: String(err),
            toastOnly: true,
          });
        }
      }
    }
  }
  isIconLongPress = false;
  isSwipeCancel.value = false;
};

const handleIconTouchCancel = (e: TouchEvent) => {
  if (e.cancelable) e.preventDefault();
  if (iconLongPressTimeout) {
    clearTimeout(iconLongPressTimeout);
    iconLongPressTimeout = null;
  }
  if (isLongPressRecording.value) {
    cancelRecording();
    isLongPressRecording.value = false;
    isSwipeCancel.value = false;
  }
  isIconLongPress = false;
};

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
const canSend = computed(() => hasContent.value && attachmentsReady.value && !props.disabled && !props.suspended && !shareStore.sending && !isPreparing.value);

// 编辑重发意图与原会话绑定；切换会话取消意图时同时清理注入内容。
let voiceResendEditor: typeof overlayStore.editorConfig = null;
let voiceResendIntent: typeof historyStore.editingMessage = null;
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
    input.value = intent.initialContent;
    isAudioMode.value = false; // 强行切回键盘输入态
    await nextTick();
    if (textareaRef.value && !props.suspended) {
      textareaRef.value.focus();
      textareaRef.value.dispatchEvent(new Event('input', { bubbles: true }));
    }
  } else if (previousIntent) {
    input.value = '';
  }
});

// Keep ordinary input separate from the one replaceable share draft.
let ordinaryInput: string | null = null;
watch(() => shareStore.activeDraft?.deliveryId, async (deliveryId, previous) => {
  if (deliveryId) {
    if (!previous) ordinaryInput = input.value;
    input.value = shareStore.activeDraft?.text ?? '';
    isAudioMode.value = false;
    await nextTick();
    if (textareaRef.value && !props.suspended) {
      textareaRef.value.focus();
      textareaRef.value.dispatchEvent(new Event('input', { bubbles: true }));
    }
  } else if (previous) {
    input.value = ordinaryInput ?? '';
    ordinaryInput = null;
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
      emit('send', input.value);
      input.value = '';
    }
    attachMenuOpen.value = false;
  }
};

const handleAction = () => {
  if (props.disabled || props.suspended) return;
  if (actionIsStop.value) {
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
const actionIsStop = computed(() => isGenerating.value
  && !((streamStore.currentGroupTurn?.mode ?? mentionGroupMode.value) === 'jev'
    && hasContent.value && !historyStore.editingMessage));

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
    !isComposing.value &&
    !isAudioMode.value &&
    !isSTTActive.value &&
    !isLongPressRecording.value,
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
  input.value = input.value.slice(0, target.start) + insertText + input.value.slice(target.end);
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
  if (props.disabled) return;
  emit('attach');
  await attachmentStore.handleAttachment(mode);
};

const { handlePaste, handleBeforeInput } = useLongTextPaste(input);

const removeStagedAttachment = (index: number) => {
  attachmentStore.removeStaged(index);
};

const handleClickOutside = (event: MouseEvent) => {
  if (attachMenuOpen.value && rootRef.value && !rootRef.value.contains(event.target as Node)) {
    attachMenuOpen.value = false;
  }
};

// 观察实际文本宽度，覆盖软换行、侧栏布局、旋转与语音切回文字。
let textareaObserver: ResizeObserver | null = null;
let measuredWidth = -1;
let measuredHeight = -1;
let measuredLineHeight = '';
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
  }
  autoResize();
});

onUnmounted(() => {
  prepareForVoice();
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
        :class="{ 
          'ring-1 ring-blue-500/30 border-blue-500/50': isSTTActive || isLongPressRecording, 
          'ring-1 ring-red-500/30 border-red-500/50': isSwipeCancel 
        }"
      >
        <span v-if="isExpanded" class="input-editor-title text-sm text-[var(--primary-text)] opacity-60">编辑消息</span>
        <button v-if="(canExpand || isExpanded) && !isAudioMode && !isLongPressRecording"
          type="button" class="input-expand-toggle absolute right-1 top-1 z-local w-11 h-11 flex items-center justify-center rounded-lg text-[var(--primary-text)] opacity-60 active:opacity-100"
          :aria-label="isExpanded ? '缩小输入框' : '展开输入框'" :aria-expanded="isExpanded"
          @pointerdown.prevent @mousedown.prevent @click="toggleEditor">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path v-if="isExpanded" d="M4 9h5V4M9 9 3 3m17 12h-5v5m0-5 6 6" />
            <path v-else d="M15 3h6v6m0-6-7 7M9 21H3v-6m0 6 7-7" />
          </svg>
        </button>
        
        <!-- 左侧：语音/键盘切换及长按附件录制按钮 -->
        <button
          v-if="!isExpanded && (isAndroid || isIOS)"
          @touchstart.prevent="handleIconTouchStart"
          @touchmove="handleIconTouchMove"
          @touchend="handleIconTouchEnd"
          @touchcancel="handleIconTouchCancel"
          @click="handleVoiceClick"
          :aria-label="isIOS ? '语音输入（尚未开发）' : isAudioMode ? '切换文字输入' : '语音输入'"
          class="input-voice w-9 h-9 mb-0.5 flex items-center justify-center shrink-0 rounded-full hover:bg-black/5 dark:hover:bg-white/5 text-[var(--primary-text)] opacity-90 active:scale-90 transition-all relative select-none touch-none"
          :class="{ 
            'bg-blue-500/10 text-blue-500': isAudioMode || isLongPressRecording,
            'bg-red-500/10 text-red-500': isSwipeCancel && isLongPressRecording
          }"
        >
          <!-- 键盘图标 (纯 inline SVG，100% 精美、零依赖，规避 UnoCSS 找不到图标的问题) -->
          <svg v-if="isAudioMode" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" class="w-6 h-6 shrink-0 animate-fade-in">
            <rect x="2" y="4" width="20" height="16" rx="2" ry="2"></rect>
            <line x1="6" y1="8" x2="6" y2="8"></line>
            <line x1="10" y1="8" x2="10" y2="8"></line>
            <line x1="14" y1="8" x2="14" y2="8"></line>
            <line x1="18" y1="8" x2="18" y2="8"></line>
            <line x1="6" y1="12" x2="6" y2="12"></line>
            <line x1="10" y1="12" x2="10" y2="12"></line>
            <line x1="14" y1="12" x2="14" y2="12"></line>
            <line x1="18" y1="12" x2="18" y2="12"></line>
            <line x1="7" y1="16" x2="17" y2="16"></line>
          </svg>
          
          <!-- 麦克风图标 -->
          <svg v-else width="26" height="26" viewBox="0 0 48 48" fill="none" xmlns="http://www.w3.org/2000/svg" class="shrink-0" :class="{ 'animate-pulse text-blue-500': isLongPressRecording }">
            <circle cx="24" cy="24" r="19.5" stroke="currentColor" stroke-width="3.5" fill="none"/>
            <circle cx="17.5" cy="24" r="3" fill="currentColor"/>
            <path d="M 21.5 18 A 6.5 6.5 0 0 1 21.5 30" stroke="currentColor" stroke-width="3" stroke-linecap="round" fill="none"/>
            <path d="M 26 13.5 A 12 12 0 0 1 26 34.5" stroke="currentColor" stroke-width="3" stroke-linecap="round" fill="none"/>
          </svg>
        </button>

        <!-- 核心输入/按住说话极简交互区 (极致自然：仅原 textarea 区域变化，右侧按钮完全正常静止显示) -->
        <div class="input-writing-area flex-1 min-w-0 flex flex-col justify-end relative min-h-[36px] py-[1px] overflow-visible">

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
            v-if="highlightActive && !isAudioMode && !isLongPressRecording"
            ref="highlightRef"
            aria-hidden="true"
            class="vcp-input-text-metrics absolute left-0 right-0 top-[1px] bottom-[1px] py-[8px] text-[15px] leading-[1.25] whitespace-pre-wrap break-words overflow-hidden pointer-events-none select-none text-[var(--primary-text)]"
          ><template v-for="(seg, i) in mentionHighlightSegments" :key="i"><span v-if="seg.mention" class="text-orange-500 bg-orange-500/15 rounded-sm font-semibold">{{ seg.text }}</span><template v-else>{{ seg.text }}</template></template>&#8203;</div>

          <!-- 情况 1：普通文本输入键盘状态 -->
          <textarea
            v-if="!isAudioMode && !isLongPressRecording"
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
            :disabled="disabled"
          ></textarea>
          
          <!-- 情况 2：语音模式 - “按住 说话” 大条状态 (仅静默填入原本 textarea 的位置，右侧加号/发送正常显示) -->
          <div 
            v-else-if="isAudioMode && !isSTTActive" 
            @touchstart.prevent="handleSTTTouchStart"
            @touchmove="handleSTTTouchMove"
            @touchend="handleSTTTouchEnd"
            @touchcancel="handleSTTTouchCancel"
            class="w-full h-[36px] flex items-center justify-center rounded-xl bg-black/5 dark:bg-white/5 border border-black/10 dark:border-white/10 active:bg-black/15 dark:active:bg-white/15 select-none touch-none cursor-pointer transform active:scale-[0.98] transition-all duration-75"
          >
            <span class="text-[13px] font-semibold text-[var(--primary-text)] opacity-85 tracking-wider">按住 说话</span>
          </div>

          <!-- 情况 3：正在按压“按住说话”大条进行实时流式转文字中 (展示倾听与临时出字) -->
          <div v-else-if="isSTTActive" class="w-full flex flex-col justify-center min-h-[36px] py-1 px-1 select-none animate-fade-in">
            <div class="flex items-center gap-1.5 text-xs font-semibold text-blue-500" :class="{ 'text-red-500': isSwipeCancel }">
              <span class="w-2.5 h-2.5 rounded-full" :class="isSwipeCancel ? 'bg-red-500 animate-pulse' : 'bg-blue-500 animate-ping'"></span>
              <span>{{ isSwipeCancel ? '松手取消转写' : '正在识别流式文字... (上滑取消)' }}</span>
            </div>
            <div class="text-[14px] text-[var(--primary-text)] min-h-[1.25rem] leading-[1.25] break-all opacity-85 italic font-medium mt-0.5">
              {{ transcriptionResult || '请开始说话...' }}
            </div>
          </div>

          <!-- 情况 4：在非语音模式下，直接长按语音图标录制音频附件进行中 (经典的“倾听中”说明，松手直发) -->
          <div v-else-if="isLongPressRecording" class="w-full flex flex-col justify-center min-h-[36px] py-1 px-1 select-none animate-fade-in">
            <div class="flex items-center gap-1.5 text-xs font-semibold text-blue-500" :class="{ 'text-red-500': isSwipeCancel }">
              <span class="w-2.5 h-2.5 rounded-full" :class="isSwipeCancel ? 'bg-red-500 animate-pulse' : 'bg-blue-500 animate-ping'"></span>
              <span>{{ isSwipeCancel ? '松手取消发送' : '倾听中... (上滑取消)' }}</span>
            </div>
            <div class="text-[14px] text-[var(--primary-text)] font-mono opacity-85 mt-0.5">
              录音时长: {{ recordingDuration }} 秒
            </div>
          </div>
          
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

          <Transition name="pop-slide">
            <!-- 发送/中止按钮 -->
            <button v-if="hasContent || isGenerating || isPreparing || isExpanded"
              @click="handleAction"
              :aria-label="actionIsStop ? '停止生成' : isPreparing ? '正在准备消息' : '发送消息'"
              :disabled="!actionIsStop && !canSend"
              class="w-8 h-8 flex items-center justify-center rounded-full shadow-sm active:scale-95 transition-all bg-blue-500 text-white"
              :class="{ 'bg-red-500': actionIsStop, 'opacity-40': !actionIsStop && !canSend }"
            >
              <div v-if="actionIsStop" class="i-heroicons-stop-16-solid text-lg"></div>
              <div v-else-if="isPreparing" class="i-heroicons-ellipsis-horizontal text-lg"></div>
              <div v-else class="i-heroicons-paper-airplane text-[15px] -rotate-45 translate-x-0.2 -translate-y-0.2"></div>
            </button>
          </Transition>
        </div>
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

/* 气泡弹出/切换动画 */
.pop-slide-enter-active,
.pop-slide-leave-active {
  transition: all 0.3s cubic-bezier(0.34, 1.56, 0.64, 1);
  overflow: hidden;
  white-space: nowrap;
}

.pop-slide-enter-from {
  opacity: 0;
  transform: scale(0.4) translateX(20px);
  width: 0;
}

.pop-slide-leave-to {
  opacity: 0;
  transform: scale(0.4) translateX(10px);
  width: 0;
  margin-left: -6px;
}

.animate-fade-in {
  animation: fadeIn 0.25s cubic-bezier(0.16, 1, 0.3, 1);
}

@keyframes fadeIn {
  from { opacity: 0; transform: translateY(4px); }
  to { opacity: 1; transform: translateY(0); }
}
</style>
