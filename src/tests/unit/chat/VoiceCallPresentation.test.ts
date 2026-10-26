import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { disposePinia, getActivePinia } from 'pinia';
import { defineComponent, nextTick, ref, type Component } from 'vue';
import { DOMWrapper, flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import InputEnhancer from '@/features/chat/InputEnhancer.vue';
import ChatView from '@/features/chat/ChatView.vue';
import GlobalOverlayManager from '@/components/GlobalOverlayManager.vue';
import FullScreenEditor from '@/components/ui/FullScreenEditor.vue';
import { attachMenuOpen } from '@/features/chat/attachMenuController';
import { useVoiceCallStore } from '@/features/chat/voice/voiceCallStore';
import type { VoiceCallSnapshot } from '@/features/chat/voice/voiceConversation';
import type { VoiceAudioEnd } from '@/features/chat/voice/microphoneMonitor';
import { useChatSessionStore } from '@/core/stores/chatSessionStore';
import { useChatHistoryStore } from '@/core/stores/chatHistoryStore';
import { useAttachmentStore } from '@/core/stores/attachmentStore';
import { useShareStore } from '@/core/stores/shareStore';
import { useTopicStore } from '@/core/stores/topicListManager';
import { useOverlayStore } from '@/core/stores/overlay';
import { useModalHistory } from '@/core/composables/useModalHistory';
import { mountWithPinia } from '@/tests/utils/mount';
import { emitTauriEvent, mockInvoke } from '@/tests/mocks/tauri';

const audio = vi.hoisted(() => ({
  startMonitor: vi.fn(), stopMonitor: vi.fn(), emitEnded: vi.fn<(event: VoiceAudioEnd) => void>(),
}));

vi.mock('@/core/utils/platform', () => ({ isAndroid: true, isIOS: false }));
vi.mock('@/features/chat/voice/microphoneMonitor', () => ({
  createMicrophoneMonitor: (_onLevel: unknown, onEnded: (event: VoiceAudioEnd) => void) => {
    audio.emitEnded.mockImplementation(onEnded);
    return { start: audio.startMonitor, stop: audio.stopMonitor, detach: vi.fn() };
  },
}));
vi.mock('@/core/composables/useKeyboardInsets', async () => {
  const { ref } = await import('vue');
  return { useKeyboardInsets: () => ({ keyboardHeight: ref(0), forceRecalculate: vi.fn() }) };
});
vi.mock('@/core/composables/useChatScroll', async () => {
  const { ref } = await import('vue');
  return { useChatScroll: () => ({
    showScrollToBottom: ref(false), scrollToBottom: vi.fn(), startAutoScroll: vi.fn(), stopAutoScroll: vi.fn(),
    checkAndLoadMore: vi.fn(), preserveViewportAcrossLayoutChange: vi.fn(async (change: () => void) => change()),
    reset: vi.fn(), dispose: vi.fn(),
  }) };
});

const MessageStub = defineComponent({
  setup() { return { expanded: ref(false) }; },
  template: '<article><button type="button" @click="expanded = !expanded">{{ expanded ? "消息已展开" : "展开消息" }}</button></article>',
});

let wrapper: VueWrapper | undefined;
let overlayWrapper: VueWrapper | undefined;
const modals = useModalHistory();

async function settle() {
  await Promise.resolve();
  await nextTick();
  await nextTick();
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(res => { resolve = res; });
  return { promise, resolve };
}

function mountComposer(component: Component = InputEnhancer) {
  wrapper = mountWithPinia(component, {
    attachTo: document.body,
    global: {
      directives: { longpress: {}, guide: {} },
      stubs: { Transition: true, GroupStopAllButton: true, GroupInviteBar: true,
        StagedAttachmentPreview: true, VcpAvatar: true, TarvenSelector: true,
        CoreStatusIndicator: true, MessageRenderer: MessageStub },
    },
  }, () => {
    const session = useChatSessionStore();
    session.setConversation({ id: 'voice-agent', type: 'agent', name: '语音助手' }, 'voice-topic');
    const history = useChatHistoryStore();
    history.loadedConversationKey = session.currentConversationKey;
    history.currentChatHistory = [{ id: 'existing-message', role: 'assistant', content: '已有消息', timestamp: 1, blocks: [] }];
    vi.spyOn(history, 'loadHistoryPaginated').mockResolvedValue({ addedCount: 0 });
    vi.spyOn(useTopicStore(), 'markTopicAsRead').mockResolvedValue(undefined);
    vi.spyOn(useShareStore(), 'editDraft').mockImplementation(() => {});
  });
  return wrapper;
}

function suspendComposer() {
  (wrapper!.getComponent(InputEnhancer).vm as unknown as { suspendComposer: () => void }).suspendComposer();
}

function mountEditorOverlay() {
  overlayWrapper = mount(GlobalOverlayManager, {
    attachTo: document.body,
    global: {
      plugins: [getActivePinia()!],
      stubs: { Transition: true, ToastManager: true, RenderedImageViewer: true },
    },
  });
  return overlayWrapper;
}

function beginEditIntent() {
  const history = useChatHistoryStore();
  const key = history.captureMessageActionKey('existing-message')!;
  history.editingMessage = { key, initialContent: '已有消息' };
  return history;
}

function editorSurface() {
  return new DOMWrapper(document.querySelector<HTMLElement>('[role="dialog"][aria-label="编辑消息"]')!);
}

beforeEach(() => {
  attachMenuOpen.value = false;
  for (const mock of Object.values(audio)) mock.mockReset();
  audio.startMonitor.mockResolvedValue(true);
  window.history.replaceState({ vcpRoot: true, vcpMain: true }, '', '#/chat');
  vi.spyOn(window.history, 'back').mockImplementation(() => {
    window.history.replaceState({ vcpRoot: true, vcpMain: true }, '');
    window.dispatchEvent(new PopStateEvent('popstate', { state: window.history.state }));
  });
});

afterEach(async () => {
  overlayWrapper?.unmount();
  overlayWrapper = undefined;
  wrapper?.unmount();
  wrapper = undefined;
  await settle();
  expect(modals.modalStackLength()).toBe(0);
  const pinia = getActivePinia();
  if (pinia) disposePinia(pinia);
  attachMenuOpen.value = false;
  vi.useRealTimers();
});

describe('voice entry from the existing composer', () => {
  it('shows the inline voice call button when empty, morphs to send with content, and keeps the attach-menu call entry', async () => {
    const view = mountComposer();
    const primaryBtn = view.get('.primary-action-btn');
    expect(primaryBtn.attributes('aria-label')).toBe('开始语音通话');
    expect(primaryBtn.classes()).toContain('is-voice');
    await primaryBtn.trigger('click');
    expect(view.emitted('start-call')).toEqual([[]]);

    await view.get('textarea').setValue('尚未发送');
    expect(primaryBtn.attributes('aria-label')).toBe('发送消息');
    expect(primaryBtn.classes()).toContain('is-send');

    await view.get('[aria-label="添加附件"]').trigger('click');
    const call = view.get('[aria-label="开始语音通话"]');
    await call.trigger('click');
    expect(view.emitted('start-call')).toEqual([[], []]);
    expect(view.emitted('send')).toBeUndefined();
    expect((view.get('textarea').element as HTMLTextAreaElement).value).toBe('尚未发送');
  });

  it('parks the same draft and attachments, closes the editor/menu and releases focus', async () => {
    const view = mountComposer();
    const textarea = view.get('textarea').element as HTMLTextAreaElement;
    Object.defineProperty(textarea, 'scrollHeight', { configurable: true, value: 160 });
    textarea.style.lineHeight = '20px';
    textarea.style.paddingTop = '8px';
    textarea.style.paddingBottom = '8px';
    await view.get('textarea').setValue('保留普通草稿\n第二行\n第三行\n第四行\n第五行');
    await settle();
    useAttachmentStore().stagedAttachments = [{ id: 'kept-file', type: 'file', src: 'cas://kept', name: 'kept.txt', size: 1, status: 'done' }];
    await view.get('[aria-label="展开输入框"]').trigger('click');
    expect(modals.modalStackLength()).toBe(1);
    textarea.focus();
    suspendComposer();
    await view.setProps({ suspended: true });
    expect(modals.modalStackLength()).toBe(0);
    expect(document.activeElement).not.toBe(textarea);
    expect(view.find('[aria-label="缩小输入框"]').exists()).toBe(false);
    await view.setProps({ suspended: false });
    expect(view.get('textarea').element).toBe(textarea);
    expect(textarea.value).toContain('保留普通草稿');
    expect(useAttachmentStore().stagedAttachments.map(item => item.id)).toEqual(['kept-file']);
    await view.get('[aria-label="添加附件"]').trigger('click');
    expect(attachMenuOpen.value).toBe(true);
    suspendComposer();
    await settle();
    expect(attachMenuOpen.value).toBe(false);
    expect(view.emitted('send')).toBeUndefined();
  });

  it('does not focus the suspended composer when a share draft arrives or is removed', async () => {
    const view = mountComposer();
    await view.get('textarea').setValue('通话前草稿');
    await view.setProps({ suspended: true });
    const textarea = view.get('textarea').element as HTMLTextAreaElement;
    const focus = vi.spyOn(textarea, 'focus');
    const share = useShareStore();
    share.current = { deliveryId: 'incoming-share', generation: 1, revision: 1, state: 'draft',
      agentId: 'voice-agent', topicId: 'voice-topic', text: '分享文字', items: [], attachments: [] };
    await settle();
    expect(textarea.value).toBe('分享文字');
    expect(focus).not.toHaveBeenCalled();
    share.current = null;
    await settle();
    expect(textarea.value).toBe('通话前草稿');
    expect(focus).not.toHaveBeenCalled();
  });

  it('keeps the parked draft and open editor when resend is blocked by an unfinished attachment', async () => {
    const view = mountComposer();
    await view.get('textarea').setValue('暂存的普通草稿');
    await view.setProps({ suspended: true });
    useAttachmentStore().stagedAttachments = [{ id: 'uploading', type: 'file', src: '', name: 'pending.txt', size: 1, status: 'loading' }];
    const history = beginEditIntent();
    const send = vi.spyOn(history, 'sendMessage').mockResolvedValue(undefined);
    await settle();
    const config = useOverlayStore().editorConfig!;
    const save = vi.spyOn(config, 'onSave');
    vi.spyOn(console, 'error').mockImplementation(() => {});
    const overlays = mountEditorOverlay();
    await settle();
    const editor = overlays.getComponent(FullScreenEditor);
    const editingTextarea = editorSurface().get('textarea').element as HTMLTextAreaElement;
    await editorSurface().get('textarea').setValue('用户在编辑器中修改的内容');
    await editorSurface().findAll('button')[1].trigger('click');
    await settle();
    await expect(save.mock.results[0].value).rejects.toThrow('请等待草稿附件处理完成后再重发');
    expect(send).not.toHaveBeenCalled();
    expect(useOverlayStore().editorConfig === config).toBe(true);
    expect(editorSurface().get('textarea').element).toBe(editingTextarea);
    expect(editingTextarea.value).toBe('用户在编辑器中修改的内容');
    expect(editor.props('saving')).toBe(false);
    expect((view.get('textarea').element as HTMLTextAreaElement).value).toBe('暂存的普通草稿');
  });

  it('keeps the edited text and same editor when a failed resend restores its intent', async () => {
    const view = mountComposer();
    await view.get('textarea').setValue('继续保留的普通草稿');
    await view.setProps({ suspended: true });
    const history = beginEditIntent();
    const intent = history.editingMessage;
    const send = vi.spyOn(history, 'sendMessage').mockImplementation(async () => {
      history.editingMessage = null;
      await nextTick();
      history.editingMessage = intent;
    });
    await settle();
    const config = useOverlayStore().editorConfig!;
    const save = vi.spyOn(config, 'onSave');
    vi.spyOn(console, 'error').mockImplementation(() => {});
    const overlays = mountEditorOverlay();
    await settle();
    const editor = overlays.getComponent(FullScreenEditor);
    const editingTextarea = editorSurface().get('textarea').element as HTMLTextAreaElement;
    await editorSurface().get('textarea').setValue('发送失败也不能丢的修改');
    await editorSurface().findAll('button')[1].trigger('click');
    await settle();
    await expect(save.mock.results[0].value).rejects.toThrow('消息重发未完成');
    expect(send).toHaveBeenCalledWith('发送失败也不能丢的修改');
    expect(history.editingMessage === intent).toBe(true);
    expect(useOverlayStore().editorConfig === config).toBe(true);
    expect(config.initialValue).toBe('已有消息');
    expect(editorSurface().get('textarea').element).toBe(editingTextarea);
    expect(editingTextarea.value).toBe('发送失败也不能丢的修改');
    expect(editor.props('saving')).toBe(false);
    expect((view.get('textarea').element as HTMLTextAreaElement).value).toBe('继续保留的普通草稿');
  });
});

describe('chat scene voice transitions', () => {
  it('keeps the native interruption visible through closed snapshots and clears it when the mic restarts', async () => {
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const snapshot: VoiceCallSnapshot = {
      callId: '', target: { ownerId: 'voice-agent', ownerType: 'agent', topicId: 'voice-topic' },
      revision: 1, active: true, micState: 'closed', micStopReason: null, inputState: 'idle',
      replyState: 'idle', ttsState: 'idle', partial: '', error: null, notice: null,
      inputSessionId: null, originUserMessageId: null, ttsId: null, lastPersistedMessageId: null,
    };
    mockInvoke('begin_voice_conversation', args => {
      snapshot.callId = args!.callId as string;
      return { ...snapshot };
    });
    const view = mountComposer(ChatView);
    const voice = useVoiceCallStore();
    voice.start();
    await flushPromises();

    const message = '音频已被其他应用接管，点击麦克风重新开启。';
    audio.emitEnded({ kind: 'stopped', sessionId: 'capture', audioEpoch: 1, reason: 'audio_focus_lost' });
    await nextTick();
    expect(view.get('.voice-control-status').text()).toBe(message);

    snapshot.micStopReason = 'audio_focus_lost';
    for (let index = 0; index < 2; index += 1) {
      snapshot.revision += 1;
      emitTauriEvent('vcp-voice-call', { ...snapshot });
      await nextTick();
      expect(voice.microphoneState).toBe('error');
      expect(view.get('.voice-control-status').text()).toBe(message);
    }
    expect(warning.mock.calls.filter(([label]) => label === '[VoiceAudio]')).toHaveLength(1);

    voice.toggleMicrophone();
    snapshot.revision += 1;
    emitTauriEvent('vcp-voice-call', { ...snapshot });
    expect(voice.microphoneState).toBe('requesting');
    expect(voice.errorMessage).toBe('');
    await flushPromises();
    snapshot.revision += 1;
    snapshot.micState = 'listening';
    snapshot.micStopReason = null;
    snapshot.inputSessionId = 'next-capture';
    emitTauriEvent('vcp-voice-call', { ...snapshot });
    await nextTick();
    expect(voice.microphoneState).toBe('on');
    expect(view.get('.voice-control-status').text()).toBe('正在聆听');
  });

  it('keeps one status node through pending capture, mute and continuing service work', async () => {
    const pending = deferred<boolean>();
    audio.startMonitor.mockReturnValueOnce(pending.promise);
    const snapshot: VoiceCallSnapshot = {
      callId: '', target: { ownerId: 'voice-agent', ownerType: 'agent', topicId: 'voice-topic' },
      revision: 1, active: true, micState: 'closed', micStopReason: null, inputState: 'idle',
      replyState: 'idle', ttsState: 'idle', partial: '', error: null, notice: null,
      inputSessionId: null, originUserMessageId: null, ttsId: null, lastPersistedMessageId: null,
    };
    mockInvoke('begin_voice_conversation', args => {
      snapshot.callId = args!.callId as string;
      return { ...snapshot };
    });
    const view = mountComposer(ChatView);
    const voice = useVoiceCallStore();
    voice.start();
    await flushPromises();
    const status = view.get('.voice-control-status');
    const line = status.get('p').element;
    const expectStatus = (text: string) => {
      expect(status.findAll('p')).toHaveLength(1);
      expect(status.get('p').element).toBe(line);
      expect(status.text()).toBe(text);
    };

    expect(voice.microphoneState).toBe('requesting');
    expectStatus('');
    pending.resolve(true);
    await flushPromises();
    expectStatus('正在聆听');
    await view.get('[aria-label="关闭麦克风"]').trigger('click');
    expectStatus('');

    const updates: Array<[Partial<VoiceCallSnapshot>, string]> = [
      [{ inputState: 'transcribing' }, '正在识别'],
      [{ inputState: 'transcribing', partial: '已录好的语音内容' }, '已录好的语音内容'],
      [{ replyState: 'generating' }, '正在回复'],
      [{ replyState: 'finished', ttsState: 'speaking' }, '正在朗读'],
      [{ inputState: 'error', error: '识别失败，请重试', partial: '旧转写' }, '识别失败，请重试'],
    ];
    for (const [update, text] of updates) {
      snapshot.revision += 1;
      emitTauriEvent('vcp-voice-call', { ...snapshot, ...update });
      await nextTick();
      expect(voice.microphoneState).toBe('off');
      expectStatus(text);
    }
    expect(status.get('p').classes()).toContain('is-error');

    audio.startMonitor.mockRejectedValueOnce('microphone_permission_denied');
    await view.get('[aria-label="开启麦克风"]').trigger('click');
    await flushPromises();
    expectStatus('麦克风权限未开启，请在系统设置中允许后重试。');
    expect(status.get('p').classes()).toContain('is-error');
  });

  it('keeps the composer and interactive message nodes through call entry and hangup', async () => {
    const view = mountComposer(ChatView);
    await settle();
    const input = view.getComponent(InputEnhancer);
    const inputInstanceId = input.vm.$.uid;
    const textarea = view.get('textarea').element as HTMLTextAreaElement;
    const message = view.get('[data-message-id="existing-message"]').element;
    await view.get('textarea').setValue('挂断后继续写');
    await view.get('[aria-label="添加附件"]').trigger('click');
    await view.get('[aria-label="开始语音通话"]').trigger('click');
    await settle();
    expect(useVoiceCallStore().active).toBe(true);
    expect(view.getComponent(InputEnhancer).vm.$.uid).toBe(inputInstanceId);
    expect(view.get('textarea').element).toBe(textarea);
    expect(input.props('suspended')).toBe(true);
    expect(view.get('[data-message-id="existing-message"]').element).toBe(message);
    await view.get('[data-message-id="existing-message"] button').trigger('click');
    expect(message.textContent).toBe('消息已展开');
    expect(attachMenuOpen.value).toBe(false);
    expect(view.find('[aria-label="风格指导"]').exists()).toBe(true);
    expect(view.find('[aria-label="语音设置"]').exists()).toBe(true);
    await view.get('[aria-label="挂断并退出语音模式"]').trigger('click');
    await settle();
    expect(useVoiceCallStore().active).toBe(false);
    expect(view.getComponent(InputEnhancer).vm.$.uid).toBe(inputInstanceId);
    expect(view.get('textarea').element).toBe(textarea);
    expect(textarea.value).toBe('挂断后继续写');
    expect(view.get('[data-message-id="existing-message"]').element).toBe(message);
    expect(message.textContent).toBe('消息已展开');
    expect(attachMenuOpen.value).toBe(false);
    expect(view.find('[aria-label="打开助手与话题侧栏"]').exists()).toBe(true);
    expect(view.find('[aria-label="打开通知与工具侧栏"]').exists()).toBe(true);
    expect(document.activeElement).not.toBe(textarea);
  });
});
