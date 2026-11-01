import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { reactive } from 'vue';
import { createPinia, disposePinia } from 'pinia';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import App from '../../../App.vue';
import { invokeMock, mockInvoke } from '../../mocks/tauri';
import * as messageScroll from '../../../core/utils/scrollToMessage';
import { useChatHistoryStore } from '../../../core/stores/chatHistoryStore';

vi.mock('/vcpmobile.svg', () => ({ default: '/vcpmobile.svg' }));
const context = vi.hoisted(() => ({
  lifecycle: {} as { state: string; lifecycleSnapshot: { phase: string } | null; bootstrap: () => Promise<void> },
  conversation: null as { ownerId: string; ownerType: string; topicId: string; epoch: number } | null,
  selectTopic: vi.fn(async (_ownerId: string, _ownerType: string, _topicId: string) => {}),
  notice: vi.fn(),
  voice: {} as { active: boolean },
  selectedItem: null as { id: string } | null,
  clearConversation: vi.fn(),
  closeTopModal: vi.fn(() => false),
  shareRefresh: vi.fn(async () => {}),
  sharePoll: vi.fn(async () => {}),
}));
const native = vi.hoisted(() => ({ ios: true }));
vi.mock('../../../core/utils/platform', () => ({
  get isAndroid() { return !native.ios; },
  get isIOS() { return native.ios; },
  get nativePlatform() { return native.ios ? 'ios' : 'android'; },
}));
vi.mock('../../../core/composables/useAppLifecycle', () => ({ useAppLifecycle: () => {} }));
vi.mock('../../../core/composables/useKeyboardInsets', () => ({ retainNativeInsetsBridge: () => () => {} }));
vi.mock('../../../core/composables/useSidebarSwipe', () => ({ useSidebarSwipe: () => {} }));
vi.mock('../../../core/composables/useEmoticonFixer', () => ({ useEmoticonFixer: () => ({ initGlobalFixer: () => {} }) }));
vi.mock('../../../core/composables/useNotificationProcessor', () => ({ useNotificationProcessor: () => ({ processPayload: () => ({ silent: true }) }) }));
vi.mock('../../../core/composables/useAutoUpdate', () => ({ useAutoUpdate: () => ({ isPromptOpen: false, updateInfo: null }) }));
vi.mock('../../../core/composables/useModalHistory', () => ({ useModalHistory: () => ({ initRootHistory: () => {}, closeTopModal: context.closeTopModal, modalStackLength: () => 0 }) }));
vi.mock('../../../core/stores/appLifecycle', () => ({ useAppLifecycleStore: () => context.lifecycle }));
vi.mock('../../../core/stores/theme', () => ({ useThemeStore: () => ({ availableThemes: [], currentThemeInfo: null, isDarkResolved: false }) }));
vi.mock('../../../core/stores/layout', () => ({ useLayoutStore: () => ({ leftDrawerOpen: false, rightDrawerOpen: false, setLeftDrawer: () => {}, setRightDrawer: () => {} }) }));
vi.mock('../../../core/stores/overlay', () => ({ useOverlayStore: () => ({}) }));
vi.mock('../../../core/stores/notification', () => ({ useNotificationStore: () => ({ addNotification: context.notice }) }));
vi.mock('../../../core/stores/chatSessionStore', () => ({ useChatSessionStore: () => ({
  get currentSelectedItem() { return context.selectedItem; },
  get currentConversationKey() { return context.conversation; },
  get sessionEpoch() { return context.conversation?.epoch ?? 0; },
  selectTopicById: context.selectTopic,
  clearConversation: context.clearConversation,
}) }));
vi.mock('../../../features/chat/voice/voiceCallStore', () => ({ useVoiceCallStore: () => context.voice }));
vi.mock('../../../core/stores/assistant', () => ({ useAssistantStore: () => ({}) }));
vi.mock('../../../core/stores/shareStore', () => ({ useShareStore: () => ({
  current: null, failures: [], selectorOpen: false, busy: false,
  refresh: context.shareRefresh, pollAndroid: context.sharePoll, notify: () => {},
}) }));
vi.mock('../../../features/cli/vcpCliStore', () => ({ useVcpCliStore: () => ({}) }));
vi.mock('vue-router', () => ({ useRouter: () => ({ isReady: async () => {}, afterEach: () => () => {} }) }));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(finish => { resolve = finish; });
  return { promise, resolve };
}
const chatClick = (topicId: string) => ({
  kind: 'chat', ownerId: 'agent', ownerType: 'agent', topicId,
});
const replyClick = (messageId: string) => ({
  kind: 'chat', ownerType: 'group', ownerId: 'group', topicId: 'topic', messageId,
});
const pendingCommand = 'plugin:vcp-mobile|get_pending_notification';
type IOSClick = ReturnType<typeof replyClick> | { kind: 'app' };
let pendingIOSClick: IOSClick | undefined;
const takePendingClick = () => {
  const click = pendingIOSClick;
  pendingIOSClick = undefined;
  return click ?? {};
};
const signalIOSClick = () => window.dispatchEvent(new Event('vcp-notification-pending'));
const iosClick = (click: IOSClick) => {
  pendingIOSClick = click;
  signalIOSClick();
};
const androidClick = (topicId: string) => window.dispatchEvent(new CustomEvent('vcp-notification-click', { detail: chatClick(topicId) }));
let wrapper: VueWrapper | undefined;
let pinia: ReturnType<typeof createPinia>;
const mountApp = () => {
  wrapper = mount(App, { global: { plugins: [pinia], stubs: {
    PermissionGate: true, BootScreen: true, AgentSidebar: true, RightSidebar: true,
    GlobalOverlayManager: true, FeatureOverlays: true, GuideOverlay: true,
    UpdatePrompt: true, ShareAgentSelector: true, RouterView: true,
  } } });
};

beforeEach(() => {
  pinia = createPinia();
  native.ios = true;
  context.lifecycle = reactive({ state: 'BOOTING', lifecycleSnapshot: null, bootstrap: vi.fn(async () => {}) });
  context.conversation = null;
  context.selectTopic.mockReset().mockImplementation(async (ownerId, ownerType, topicId) => {
    context.conversation = { ownerId, ownerType, topicId, epoch: (context.conversation?.epoch ?? 0) + 1 };
  });
  context.notice.mockClear();
  context.voice = reactive({ active: false });
  context.selectedItem = null;
  context.clearConversation.mockClear();
  context.closeTopModal.mockReset().mockReturnValue(false);
  context.shareRefresh.mockReset().mockResolvedValue(undefined);
  context.sharePoll.mockReset().mockResolvedValue(undefined);
  pendingIOSClick = undefined;
  mockInvoke(pendingCommand, takePendingClick);
  mockInvoke('get_topics', () => [{ id: 'old' }, { id: 'new' }]);
});
afterEach(() => {
  wrapper?.unmount();
  wrapper = undefined;
  disposePinia(pinia);
  vi.restoreAllMocks();
});

describe('root share recovery', () => {
  it('renews presentation on entry without renewing it after a slow Android handoff', async () => {
    native.ios = false;
    context.lifecycle.state = 'READY';
    const handoff = deferred<void>();
    context.sharePoll.mockImplementationOnce(() => handoff.promise);
    mountApp();
    await flushPromises();
    expect(context.shareRefresh).toHaveBeenCalledTimes(1);
    expect(context.sharePoll).toHaveBeenCalledTimes(1);

    // A user may close the selector now. Completing intake must not renew the
    // entry intent and reopen that retained share a second time.
    handoff.resolve();
    await flushPromises();
    expect(context.shareRefresh).toHaveBeenCalledTimes(1);
  });
});

describe('root voice call navigation', () => {
  it('lets upper surfaces consume back and keeps the conversation until the call ends', async () => {
    context.lifecycle.state = 'READY';
    context.selectedItem = { id: 'agent' };
    context.voice.active = true;
    mountApp();
    await flushPromises();

    context.closeTopModal.mockReturnValueOnce(true);
    window.dispatchEvent(new CustomEvent('vcp-hardware-back'));
    window.dispatchEvent(new CustomEvent('vcp-exit-requested'));
    expect(context.closeTopModal).toHaveBeenCalledTimes(2);
    expect(context.clearConversation).not.toHaveBeenCalled();

    context.voice.active = false;
    window.dispatchEvent(new CustomEvent('vcp-hardware-back'));
    expect(context.clearConversation).toHaveBeenCalledOnce();
  });

  it('hides both sidebar surfaces without replacing the chat workspace', async () => {
    context.lifecycle.state = 'READY';
    mountApp();
    await flushPromises();
    const main = wrapper!.find('.vcp-workspace-main').element;
    const left = wrapper!.find('.vcp-workspace-left').element;
    const right = wrapper!.find('.vcp-workspace-right').element;

    context.voice.active = true;
    await flushPromises();
    expect(wrapper!.find('.vcp-workspace-row').classes()).toContain('is-voice-call');
    expect((left as HTMLElement).style.display).toBe('none');
    expect((right as HTMLElement).style.display).toBe('none');
    expect(wrapper!.find('.vcp-workspace-main').element).toBe(main);

    context.voice.active = false;
    await flushPromises();
    expect(wrapper!.find('.vcp-workspace-left').element).toBe(left);
    expect(wrapper!.find('.vcp-workspace-right').element).toBe(right);
    expect((left as HTMLElement).style.display).not.toBe('none');
    expect((right as HTMLElement).style.display).not.toBe('none');
  });
});

describe('root notification navigation', () => {
  it('opens a cold iOS notification at its committed message rather than the newest group reply', async () => {
    pendingIOSClick = replyClick('db-reply-a');
    context.conversation = { ownerType: 'agent', ownerId: 'other', topicId: 'other', epoch: 1 };
    mockInvoke('load_chat_history_around', () => [
      { id: 'db-reply-a', timestamp: 1 }, { id: 'db-reply-b', timestamp: 2 },
    ]);
    const scroll = vi.spyOn(messageScroll, 'scrollToMessageById').mockReturnValue(true);
    mountApp();
    await flushPromises();
    expect(scroll).not.toHaveBeenCalled();
    context.lifecycle.state = 'READY';
    await flushPromises();
    signalIOSClick();
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith('load_chat_history_around', expect.objectContaining({
      ownerId: 'group', ownerType: 'group', topicId: 'topic', anchorMsgId: 'db-reply-a',
    }));
    expect(context.selectTopic).toHaveBeenCalledExactlyOnceWith('group', 'group', 'topic');
    expect(useChatHistoryStore(pinia).currentChatHistory.map(message => message.id)).toEqual(['db-reply-a', 'db-reply-b']);
    expect(scroll).toHaveBeenCalledExactlyOnceWith('db-reply-a', { behavior: 'instant' });
  });

  it('keeps the current screen when a newer iOS app-only click supersedes a pending message read', async () => {
    const pending = deferred<{ id: string }[]>();
    mockInvoke('load_chat_history_around', () => pending.promise);
    mountApp();
    context.lifecycle.state = 'READY';
    await flushPromises();
    iosClick(replyClick('old'));
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith('load_chat_history_around', expect.objectContaining({ anchorMsgId: 'old' }));
    iosClick({ kind: 'app' });
    await flushPromises();
    pending.resolve([{ id: 'old' }]);
    await flushPromises();
    expect(context.selectTopic).not.toHaveBeenCalled();
    expect(context.notice).not.toHaveBeenCalled();
  });

  it('anchors only the newer iOS reply when message reads finish out of order', async () => {
    const oldRead = deferred<{ id: string }[]>();
    mockInvoke('load_chat_history_around', args => args?.anchorMsgId === 'old' ? oldRead.promise : [{ id: 'new' }]);
    const scroll = vi.spyOn(messageScroll, 'scrollToMessageById').mockReturnValue(true);
    context.lifecycle.state = 'READY';
    mountApp();
    await flushPromises();
    iosClick({ ...replyClick('old'), topicId: 'older-topic' });
    await flushPromises();
    iosClick(replyClick('new'));
    await flushPromises();
    oldRead.resolve([{ id: 'old' }]);
    await flushPromises();
    expect(context.selectTopic).toHaveBeenCalledExactlyOnceWith('group', 'group', 'topic');
    expect(scroll).toHaveBeenCalledExactlyOnceWith('new', { behavior: 'instant' });
  });

  it('reports a deleted iOS message without switching the current conversation', async () => {
    mockInvoke('load_chat_history_around', () => []);
    context.lifecycle.state = 'READY';
    mountApp();
    await flushPromises();
    iosClick(replyClick('deleted'));
    await flushPromises();
    expect(context.selectTopic).not.toHaveBeenCalled();
    expect(context.notice).toHaveBeenCalledWith(expect.objectContaining({ title: '通知已过期' }));
  });

  it('keeps the existing Android latest-click intent while waiting for READY', async () => {
    native.ios = false;
    mountApp();
    await flushPromises();
    androidClick('old');
    androidClick('new');
    await flushPromises();
    expect(context.selectTopic).not.toHaveBeenCalled();
    context.lifecycle.state = 'READY';
    await flushPromises();
    expect(context.selectTopic).toHaveBeenCalledExactlyOnceWith('agent', 'agent', 'new');
  });

  it('does not let an older Android target lookup overwrite the newer click intent', async () => {
    native.ios = false;
    const oldTopics = deferred<unknown>();
    let lookups = 0;
    mockInvoke('get_topics', () => ++lookups === 1 ? oldTopics.promise : [{ id: 'new' }]);
    mountApp();
    await flushPromises();
    context.lifecycle.state = 'READY';
    await flushPromises();
    androidClick('old');
    androidClick('new');
    await flushPromises();
    oldTopics.resolve([{ id: 'old' }]);
    await flushPromises();
    expect(context.selectTopic).toHaveBeenCalledExactlyOnceWith('agent', 'agent', 'new');
  });

  it('keeps the manually selected topic after a hot iOS click and a WebView remount', async () => {
    mockInvoke('load_chat_history_around', () => [{ id: 'reply-a' }]);
    const scroll = vi.spyOn(messageScroll, 'scrollToMessageById').mockReturnValue(true);
    context.lifecycle.state = 'READY';
    mountApp();
    await flushPromises();
    iosClick(replyClick('reply-a'));
    await flushPromises();
    expect(scroll).toHaveBeenCalledExactlyOnceWith('reply-a', { behavior: 'instant' });

    await context.selectTopic('agent', 'agent', 'topic-b');
    wrapper!.unmount();
    mountApp();
    await flushPromises();
    context.lifecycle.lifecycleSnapshot = { phase: 'active' };
    await flushPromises();
    expect(context.conversation?.topicId).toBe('topic-b');
    expect(scroll).toHaveBeenCalledOnce();
  });

  it('ignores an Android target lookup and future native delivery after root unmount', async () => {
    native.ios = false;
    const topics = deferred<unknown>();
    mockInvoke('get_topics', () => topics.promise);
    mountApp();
    await flushPromises();
    context.lifecycle.state = 'READY';
    await flushPromises();
    androidClick('old');
    wrapper!.unmount();
    wrapper = undefined;
    topics.resolve([{ id: 'old' }]);
    androidClick('new');
    await flushPromises();
    expect(context.selectTopic).not.toHaveBeenCalled();
    expect(context.notice).not.toHaveBeenCalled();
    expect(invokeMock.mock.calls.filter(([command]) => command === 'get_topics')).toHaveLength(1);
  });

  it('keeps the newer iOS click when the cold read returns after its hot signal', async () => {
    const startupRead = deferred<unknown>();
    const read = vi.fn(takePendingClick).mockImplementationOnce(() => startupRead.promise);
    mockInvoke(pendingCommand, read);
    mockInvoke('load_chat_history_around', args => [{ id: args?.anchorMsgId }]);
    const scroll = vi.spyOn(messageScroll, 'scrollToMessageById').mockReturnValue(true);
    context.lifecycle.state = 'READY';
    mountApp();
    await flushPromises();
    expect(read).toHaveBeenCalledOnce();
    iosClick({ ...replyClick('new'), topicId: 'new-topic' });
    await flushPromises();
    startupRead.resolve(replyClick('old'));
    await flushPromises();
    expect(context.conversation?.topicId).toBe('new-topic');
    expect(scroll).toHaveBeenLastCalledWith('new', { behavior: 'instant' });
  });

  it('opens a pending iOS reply when returning to active without a hot signal', async () => {
    context.lifecycle.state = 'READY';
    mountApp();
    await flushPromises();
    pendingIOSClick = replyClick('missed-signal');
    mockInvoke('load_chat_history_around', () => [{ id: 'missed-signal' }]);
    const scroll = vi.spyOn(messageScroll, 'scrollToMessageById').mockReturnValue(true);
    context.lifecycle.lifecycleSnapshot = { phase: 'active' };
    await flushPromises();
    expect(scroll).toHaveBeenCalledExactlyOnceWith('missed-signal', { behavior: 'instant' });
  });

  it('ignores a late pending read and further iOS signals after unmount', async () => {
    const pending = deferred<unknown>();
    const read = vi.fn(() => pending.promise);
    mockInvoke(pendingCommand, read);
    context.lifecycle.state = 'READY';
    mountApp();
    await flushPromises();
    wrapper!.unmount();
    wrapper = undefined;
    signalIOSClick();
    pending.resolve(replyClick('old'));
    await flushPromises();
    expect(read).toHaveBeenCalledOnce();
    expect(invokeMock.mock.calls.some(([command]) => command === 'load_chat_history_around')).toBe(false);
    expect(context.notice).not.toHaveBeenCalled();
  });
});
