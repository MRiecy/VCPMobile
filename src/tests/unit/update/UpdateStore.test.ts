import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { useUpdateStore } from '../../../core/stores/update';
import { emitTauriEvent, invokeMock, listenMock, mockInvoke } from '../../mocks/tauri';
import { UPDATE_STATUS_EVENT, type UpdateStatus } from '../../../core/types/update';

vi.mock('../../../core/utils/platform', () => ({
  nativePlatform: 'android', isAndroid: true, isIOS: false,
}));

const idle: UpdateStatus = {
  state: 'idle',
  info: null,
  downloaded: 0,
  total: null,
  error: null,
};

const available: UpdateStatus = {
  ...idle,
  state: 'available',
  info: {
    hasUpdate: true,
    currentVersion: '1.1.4',
    latestVersion: '1.2.0',
    releasePageUrl: 'https://github.com/MRiecy/VCPMobile/releases/tag/v1.2.0',
    releaseNotes: 'notes',
    apkSize: 100 * 1024 * 1024,
    apkSha256: 'a'.repeat(64),
  },
  total: 100 * 1024 * 1024,
};

describe('update store', () => {
  let pinia: Pinia;
  beforeEach(() => {
    localStorage.clear();
    pinia = createPinia();
    setActivePinia(pinia);
  });

  afterEach(() => {
    disposePinia(pinia);
    vi.unstubAllGlobals();
  });


  it('publishes the local preference after saving and leaves it unchanged on a failed write', () => {
    const store = useUpdateStore();
    expect(store.setAutoCheckEnabled(false)).toEqual({ ok: true, changed: true, enabled: false });
    expect(localStorage.getItem('vcp_auto_update_check')).toBe('off');
    expect(store.autoCheckEnabled).toBe(false);

    vi.stubGlobal('localStorage', {
      getItem: localStorage.getItem.bind(localStorage),
      setItem: () => { throw new Error('quota exceeded'); },
    });
    expect(store.setAutoCheckEnabled(true)).toEqual({
      ok: false, changed: false, enabled: false, error: 'Error: quota exceeded',
    });
    expect(store.autoCheckEnabled).toBe(false);
    expect(store.status).toEqual(idle);
    expect(localStorage.getItem('vcp_auto_update_check')).toBe('off');

    vi.unstubAllGlobals();
    expect(store.setAutoCheckEnabled(true)).toEqual({ ok: true, changed: true, enabled: true });
    expect(localStorage.getItem('vcp_auto_update_check')).toBe('on');
    expect(store.autoCheckEnabled).toBe(true);
  });


  it('init pulls the current snapshot once and then follows broadcast events', async () => {
    mockInvoke('get_update_status', () => idle);
    const store = useUpdateStore();

    await store.init();
    await store.init();
    expect(store.state).toBe('idle');
    expect(
      invokeMock.mock.calls.filter(([command]) => command === 'get_update_status'),
    ).toHaveLength(1);

    emitTauriEvent(UPDATE_STATUS_EVENT, {
      ...available,
      state: 'downloading',
      downloaded: 50 * 1024 * 1024,
    });
    expect(store.state).toBe('downloading');
    expect(store.progressPercent).toBe(50);
    expect(store.isBusy).toBe(true);
  });



  it('retries a failed subscription and stops consuming events when disposed', async () => {
    mockInvoke('get_update_status', () => idle);
    listenMock.mockRejectedValueOnce(new Error('listener unavailable'));
    const store = useUpdateStore();

    await expect(store.init()).rejects.toThrow('listener unavailable');
    await store.init();
    expect(listenMock).toHaveBeenCalledTimes(2);
    emitTauriEvent(UPDATE_STATUS_EVENT, available);
    expect(store.state).toBe('available');

    store.$dispose();
    emitTauriEvent(UPDATE_STATUS_EVENT, idle);
    expect(store.state).toBe('available');
  });

  it('releases a failed snapshot subscription and permits a later retry', async () => {
    mockInvoke('get_update_status', async () => { throw new Error('snapshot unavailable'); });
    const store = useUpdateStore();

    await expect(store.init()).rejects.toThrow('snapshot unavailable');
    emitTauriEvent(UPDATE_STATUS_EVENT, available);
    expect(store.state).toBe('idle');

    mockInvoke('get_update_status', () => available);
    await store.init();
    expect(store.state).toBe('available');
    expect(listenMock).toHaveBeenCalledTimes(2);
  });
});
