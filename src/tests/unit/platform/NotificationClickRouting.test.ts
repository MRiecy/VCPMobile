import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { reactive } from 'vue';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import App from '../../../App.vue';
import { emitTauriEvent, invokeMock, listenMock, mockInvoke } from '../../mocks/tauri';

vi.mock('/vcpmobile.svg', () => ({ default: '/vcpmobile.svg' }));
const context = vi.hoisted(() => ({
  lifecycle: {} as { state: string; lifecycleSnapshot: { phase: string } | null; bootstrap: () => Promise<void> },
  selectTopic: vi.fn(async () => {}),
  notice: vi.fn(),
  voice: {} as { active: boolean },
  selectedItem: null as { id: string } | null,
  clearConversation: vi.fn(),
  closeTopModal: vi.fn(() => false),
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
  selectTopicById: context.selectTopic,
  clearConversation: context.clearConversation,
}) }));
vi.mock('../../../features/chat/voice/voiceCallStore', () => ({ useVoiceCallStore: () => context.voice }));
vi.mock('../../../core/stores/assistant', () => ({ useAssistantStore: () => ({}) }));
vi.mock('../../../core/stores/shareStore', () => ({ useShareStore: () => ({
  current: null, failures: [], selectorOpen: false, busy: false,
  refresh: async () => {}, receiveAndroid: () => {}, notify: () => {},
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
const appClick = (sequence: number) => ({ clickId: `click-${sequence}`, sequence, kind: 'app' });
const androidClick = (topicId: string) => window.dispatchEvent(new CustomEvent('vcp-notification-click', { detail: chatClick(topicId) }));
let wrapper: VueWrapper | undefined;
const mountApp = () => {
  wrapper = mount(App, { global: { stubs: {
    PermissionGate: true, BootScreen: true, AgentSidebar: true, RightSidebar: true,
    GlobalOverlayManager: true, FeatureOverlays: true, GuideOverlay: true,
    UpdatePrompt: true, ShareAgentSelector: true, RouterView: true,
  } } });
};

beforeEach(() => {
  native.ios = true;
  context.lifecycle = reactive({ state: 'BOOTING', lifecycleSnapshot: null, bootstrap: vi.fn(async () => {}) });
  context.selectTopic.mockClear();
  context.notice.mockClear();
  context.voice = reactive({ active: false });
  context.selectedItem = null;
  context.clearConversation.mockClear();
  context.closeTopModal.mockReset().mockReturnValue(false);
  mockInvoke('plugin:vcp-mobile|get_pending_notification', () => null);
  mockInvoke('get_topics', () => [{ id: 'old' }, { id: 'new' }]);
});
afterEach(() => {
  wrapper?.unmount();
  wrapper = undefined;
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

  it('keeps the current page for real iOS app-only clicks, including duplicate and late cold snapshots', async () => {
    const pending = deferred<unknown>();
    mockInvoke('plugin:vcp-mobile|get_pending_notification', () => pending.promise);
    mountApp();
    await flushPromises();
    context.lifecycle.state = 'READY';
    await flushPromises();
    emitTauriEvent('vcp-notification-click', appClick(2));
    emitTauriEvent('vcp-notification-click', appClick(2));
    pending.resolve(appClick(1));
    await flushPromises();
    // Returning to active re-reads the retained native snapshot; it is not a queue drain.
    context.lifecycle.lifecycleSnapshot = { phase: 'active' };
    await flushPromises();
    expect(context.selectTopic).not.toHaveBeenCalled();
    expect(context.notice).not.toHaveBeenCalled();
    expect(invokeMock.mock.calls.filter(([command]) => command === 'get_topics')).toHaveLength(0);
    expect(invokeMock.mock.calls.filter(([command]) => command === 'plugin:vcp-mobile|get_pending_notification')).toHaveLength(2);
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

  it('retries an iOS listener installation on active and reads the pending app-only response', async () => {
    const originalListen = listenMock.getMockImplementation()!;
    let attempts = 0;
    listenMock.mockImplementation((event, handler) => event === 'vcp-notification-click' && ++attempts === 1
      ? Promise.reject(new Error('listener not ready')) : originalListen(event, handler));
    try {
      mountApp();
      await flushPromises();
      context.lifecycle.state = 'READY';
      await flushPromises();
      mockInvoke('plugin:vcp-mobile|get_pending_notification', () => appClick(1));
      context.lifecycle.lifecycleSnapshot = { phase: 'active' };
      await flushPromises();
      expect(attempts).toBe(2);
      expect(invokeMock.mock.calls.filter(([command]) => command === 'plugin:vcp-mobile|get_pending_notification')).toHaveLength(2);
      expect(context.selectTopic).not.toHaveBeenCalled();
    } finally {
      listenMock.mockImplementation(originalListen);
    }
  });

  it('releases a notification listener whose installation finishes after unmount', async () => {
    const installed = deferred<() => void>();
    const unlisten = vi.fn();
    const originalListen = listenMock.getMockImplementation()!;
    listenMock.mockImplementation((event, handler) => event === 'vcp-notification-click'
      ? installed.promise : originalListen(event, handler));
    try {
      mountApp();
      await flushPromises();
      wrapper!.unmount();
      wrapper = undefined;
      installed.resolve(unlisten);
      await flushPromises();
      expect(unlisten).toHaveBeenCalledOnce();
      expect(invokeMock).not.toHaveBeenCalledWith('plugin:vcp-mobile|get_pending_notification');
    } finally {
      listenMock.mockImplementation(originalListen);
    }
  });
});
