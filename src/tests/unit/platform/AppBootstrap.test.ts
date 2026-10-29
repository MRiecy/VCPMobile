import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { flushPromises } from '@vue/test-utils';
import { useAppLifecycleStore } from '../../../core/stores/appLifecycle';
import { useNotificationStore } from '../../../core/stores/notification';
import { useNotificationProcessor } from '../../../core/composables/useNotificationProcessor';
import { invokeMock, mockInvoke } from '../../mocks/tauri';

const testPlatform = vi.hoisted(() => ({ value: null as 'android' | 'ios' | null }));
vi.mock('../../../core/utils/platform', () => ({
  get nativePlatform() { return testPlatform.value; },
  get isAndroid() { return testPlatform.value === 'android'; },
  get isIOS() { return testPlatform.value === 'ios'; },
}));

vi.mock('../../../core/stores/assistant', () => ({
  useAssistantStore: () => ({ fetchAgentsAndGroups: async () => {} }),
}));
vi.mock('../../../core/stores/settings', () => ({
  useSettingsStore: () => ({ fetchSettings: async () => {} }),
}));
vi.mock('../../../core/stores/theme', () => ({
  useThemeStore: () => ({ initTheme: async () => {} }),
}));
vi.mock('../../../core/stores/chatSessionStore', () => ({
  useChatSessionStore: () => ({
    currentSelectedItem: null,
    currentTopicId: null,
    reconcileCurrentConversation: () => {},
  }),
}));
vi.mock('../../../core/stores/chatHistoryStore', () => ({
  useChatHistoryStore: () => ({ preloadHistory: async () => {} }),
}));
vi.mock('../../../core/stores/topicListManager', () => ({
  useTopicStore: () => ({ loadTopicList: async () => {} }),
}));
vi.mock('../../../core/stores/avatar', () => ({
  useAvatarStore: () => ({ preloadMetadata: async () => {} }),
}));
vi.mock('../../../core/utils/bootTrace', () => ({
  bootMark: () => {},
  reportBootTrace: async () => {},
  trackBootStage: (_name: string, promise: Promise<unknown>) => promise,
}));

describe('platform-aware bootstrap', () => {
  let pinia: Pinia;
  let backgroundServiceFailures: Array<'guardian' | 'helper' | 'events'>;
  let logStatus: 'closed' | 'connecting' | 'connected' | 'error';
  let logConfigurationError: string | null;

  beforeEach(() => {
    testPlatform.value = null;
    localStorage.clear();
    pinia = createPinia();
    setActivePinia(pinia);
    backgroundServiceFailures = [];
    logStatus = 'connected';
    logConfigurationError = null;
    mockInvoke('allocate_voice_document', () => '42');
    mockInvoke('get_system_snapshot', () => ({
      lifecycle: null,
      core: 'ready',
      message: 'ready',
      log: logStatus,
      logConfigurationError,
      sync: 'disconnected',
      databaseRecovery: null,
      backgroundServiceFailures,
    }));
  });

  afterEach(() => {
    disposePinia(pinia);
    vi.useRealTimers();
  });

  it('starts voice document admission without blocking READY on physical release', async () => {
    let release!: () => void;
    mockInvoke('connect_voice_document', () => new Promise<void>(resolve => { release = resolve; }));
    const lifecycle = useAppLifecycleStore();
    const boot = lifecycle.bootstrap();
    await flushPromises();
    expect(lifecycle.state).toBe('READY');
    expect(invokeMock).toHaveBeenCalledWith('connect_voice_document', { documentId: '42' });
    release();
    await boot;
    expect(lifecycle.state).toBe('READY');
    await lifecycle.hydrateSystemStatus();
    await lifecycle.bootstrap(true);
    expect(invokeMock.mock.calls.filter(([command]) => command === 'connect_voice_document')).toHaveLength(1);
    expect(invokeMock.mock.calls.some(([command]) => command === 'connect_tts_document')).toBe(false);
  });

  it('keeps the UI available after a native release failure and retries with the same document', async () => {
    let failed = true;
    mockInvoke('connect_voice_document', () => failed ? Promise.reject(new Error('native stop failed')) : undefined);
    const lifecycle = useAppLifecycleStore();
    await lifecycle.bootstrap();
    expect(lifecycle.state).toBe('READY');
    failed = false;
    await lifecycle.bootstrap(true);
    expect(lifecycle.state).toBe('READY');
    expect(invokeMock.mock.calls.filter(([command]) => command === 'allocate_voice_document')).toHaveLength(1);
    expect(invokeMock.mock.calls.filter(([command]) => command === 'connect_voice_document')
      .map(([, args]) => args?.documentId)).toEqual(['42', '42']);
  });

  it.each(['snapshot', 'realtime'])('shows a saved connection configuration error when %s arrives first without blocking startup', async first => {
    logStatus = 'closed';
    logConfigurationError = 'WebSocket 地址必须明确使用 ws:// 或 wss://。';
    const lifecycle = useAppLifecycleStore();
    const store = useNotificationStore();
    const { processPayload } = useNotificationProcessor();
    const content = `连接已停止：${logConfigurationError} 请在设置中修正 WebSocket 地址或 Key。`;
    const deliverRealtime = () => store.addNotification(processPayload({
      type: 'vcp-log-message',
      data: {
        id: 'vcp_connection_configuration_error',
        status: 'error',
        tool_name: 'VCPLog / VCPInfo 配置错误',
        content,
        source: 'VCPLog / VCPInfo',
      },
    }));

    if (first === 'realtime') deliverRealtime();
    await lifecycle.bootstrap();

    expect(lifecycle.state).toBe('READY');
    expect(store.vcpStatus).toEqual({
      status: 'closed', message: `连接已停止：${logConfigurationError}`, source: 'VCPLog',
    });
    expect(store.historyList).toHaveLength(1);
    expect(store.activeToasts).toHaveLength(1);
    expect(store.activeToasts[0]).toMatchObject({
      id: 'vcp_connection_configuration_error', type: 'error', message: content,
    });

    if (first === 'snapshot') deliverRealtime();
    expect(store.historyList).toHaveLength(1);
    expect(store.activeToasts).toHaveLength(1);
    expect(store.unreadCount).toBe(1);
  });

  it('shows an unconfigured closed connection as not connected without inventing a startup error', async () => {
    logStatus = 'closed';
    await useAppLifecycleStore().bootstrap();

    expect(useAppLifecycleStore().state).toBe('READY');
    const store = useNotificationStore();
    expect(store.vcpStatus).toEqual({ status: 'closed', message: '未连接', source: 'VCPLog' });
    expect(store.historyList).toHaveLength(0);
    expect(store.activeToasts).toHaveLength(0);
  });

  it.each(['realtime', 'snapshot'])('accepts a retained events failure once when %s arrives first', async first => {
    vi.useFakeTimers();
    backgroundServiceFailures = ['events'];
    const lifecycle = useAppLifecycleStore();
    const store = useNotificationStore();
    const { processPayload } = useNotificationProcessor();
    const deliverRealtime = () => store.addNotification(processPayload({
      type: 'vcp-background-service-error', service: 'events',
    }));

    if (first === 'realtime') deliverRealtime();
    else await lifecycle.hydrateSystemStatus();
    const firstTimestamp = store.historyList[0].timestamp;

    vi.advanceTimersByTime(60_000);
    if (first === 'realtime') await lifecycle.hydrateSystemStatus();
    else deliverRealtime();

    expect(store.historyList).toHaveLength(1);
    expect(store.activeToasts).toHaveLength(1);
    expect(store.unreadCount).toBe(1);
    expect(store.historyList[0].timestamp).toBe(firstTimestamp);
    expect(store.activeToasts[0].timestamp).toBe(firstTimestamp);
  });

  it('does not restore a cleared events notice when the same WebView hydrates again', async () => {
    vi.useFakeTimers();
    backgroundServiceFailures = ['events'];
    const lifecycle = useAppLifecycleStore();
    const store = useNotificationStore();
    await lifecycle.hydrateSystemStatus();
    store.activeToasts = [];
    store.clearHistory();

    vi.advanceTimersByTime(60_000);
    await lifecycle.hydrateSystemStatus();

    expect(store.historyList).toHaveLength(0);
    expect(store.activeToasts).toHaveLength(0);
    expect(store.unreadCount).toBe(0);
  });

  it('replays the retained events failure into a new WebView Pinia', async () => {
    backgroundServiceFailures = ['events'];
    await useAppLifecycleStore().hydrateSystemStatus();
    const oldStore = useNotificationStore();
    expect(oldStore.historyList).toHaveLength(1);

    disposePinia(pinia);
    pinia = createPinia();
    setActivePinia(pinia);
    await useAppLifecycleStore().hydrateSystemStatus();
    const newStore = useNotificationStore();

    expect(newStore).not.toBe(oldStore);
    expect(newStore.historyList).toHaveLength(1);
    expect(newStore.activeToasts).toHaveLength(1);
    expect(newStore.unreadCount).toBe(1);
    expect(newStore.historyList[0].id).toBe('vcp_background_service_error_events');
  });

  it.each(['denied', 'authorized', 'provisional', 'unknown'])('does not block iOS chat when notification permission is %s', async authorization => {
    testPlatform.value = 'ios';
    mockInvoke('plugin:vcp-mobile|get_notification_permission', () => ({ authorization }));
    const lifecycle = useAppLifecycleStore();

    await lifecycle.bootstrap();

    expect(lifecycle.state).toBe('READY');
    expect(invokeMock).toHaveBeenCalledWith('get_system_snapshot');
    expect(invokeMock.mock.calls.filter(([command]) => command.startsWith('plugin:vcp-mobile|')).map(([command]) => command))
      .toEqual(['plugin:vcp-mobile|get_notification_permission']);
    expect(useNotificationStore().historyList).toHaveLength(0);
  });

  it('lets an iOS user skip the first optional introduction without faking OS authorization or showing it again', async () => {
    testPlatform.value = 'ios';
    mockInvoke('plugin:vcp-mobile|get_notification_permission', () => ({ authorization: 'notDetermined' }));
    const lifecycle = useAppLifecycleStore();
    await lifecycle.bootstrap();
    expect(lifecycle.state).toBe('PERMISSIONS');
    await lifecycle.continueFromPermissionGuide();
    expect(lifecycle.state).toBe('READY');
    expect(invokeMock).not.toHaveBeenCalledWith('plugin:vcp-mobile|request_notification_permission');

    disposePinia(pinia);
    pinia = createPinia();
    setActivePinia(pinia);
    await useAppLifecycleStore().bootstrap();
    expect(useAppLifecycleStore().state).toBe('READY');
    expect(invokeMock.mock.calls.filter(([command]) => command === 'plugin:vcp-mobile|get_notification_permission')).toHaveLength(1);
  });

  it('continues iOS startup if an optional permission query fails', async () => {
    testPlatform.value = 'ios';
    mockInvoke('plugin:vcp-mobile|get_notification_permission', () => Promise.reject(new Error('native unavailable')));
    await useAppLifecycleStore().bootstrap();
    expect(useAppLifecycleStore().state).toBe('READY');
    expect(useNotificationStore().historyList).toHaveLength(0);
  });

  it('keeps the Android permission gate and rechecks native authorization before entering', async () => {
    testPlatform.value = 'android';
    mockInvoke('plugin:vcp-mobile|check_all_permissions', () => ({ notification: false, battery: true }));
    mockInvoke('plugin:vcp-mobile|check_notification_listener_permission', () => ({ enabled: true }));
    const lifecycle = useAppLifecycleStore();

    await lifecycle.bootstrap();
    expect(lifecycle.state).toBe('PERMISSIONS');
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual(expect.arrayContaining([
      'allocate_voice_document',
      'connect_voice_document',
      'plugin:vcp-mobile|check_all_permissions',
      'plugin:vcp-mobile|check_notification_listener_permission',
    ]));

    mockInvoke('plugin:vcp-mobile|check_all_permissions', () => ({ notification: true, battery: true }));
    await lifecycle.bootstrap(true);
    expect(lifecycle.state).toBe('READY');
  });

  it('retains the Android foreground fallback with an explicit warning when its permission query fails', async () => {
    testPlatform.value = 'android';
    mockInvoke('plugin:vcp-mobile|check_all_permissions', async () => { throw new Error('permission query failed'); });

    await useAppLifecycleStore().bootstrap();

    expect(useAppLifecycleStore().state).toBe('READY');
    expect(useNotificationStore().historyList.some((item) => item.id === 'startup_permission_check_failed')).toBe(true);
  });
});
