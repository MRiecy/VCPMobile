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

  it.each([
    [null, true],
    ['on', true],
    ['off', false],
    ['legacy-value', true],
  ] as const)('restores the local auto-check preference %s without initializing OTA', (stored, enabled) => {
    if (stored !== null) localStorage.setItem('vcp_auto_update_check', stored);
    const store = useUpdateStore();
    expect(store.autoCheckEnabled).toBe(enabled);
    expect(store.status).toEqual(idle);
    expect(invokeMock).not.toHaveBeenCalled();
    expect(listenMock).not.toHaveBeenCalled();
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

  it('uses the default when reading storage is unavailable without breaking the OTA owner', () => {
    const read = vi.fn(() => {
      throw new Error('storage unavailable');
    });
    vi.stubGlobal('localStorage', { getItem: read });
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    const store = useUpdateStore();
    expect(store.autoCheckEnabled).toBe(true);
    expect(store.status).toEqual(idle);
    expect(read).toHaveBeenCalledWith('vcp_auto_update_check');
    expect(console.warn).toHaveBeenCalledWith(
      '[UpdateStore] Failed to read auto-check preference:', 'Error: storage unavailable',
    );
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

  it('command results are applied to the mirrored status', async () => {
    mockInvoke('get_update_status', () => idle);
    mockInvoke('check_for_update', () => available);
    mockInvoke('start_update_download', () => ({
      ...available,
      state: 'readyToInstall',
      downloaded: available.info!.apkSize!,
    }));
    mockInvoke('install_update', () => ({ ...available, state: 'idle' as const }));

    const store = useUpdateStore();
    await store.init();

    const checked = await store.check();
    expect(checked.state).toBe('available');
    expect(store.info?.latestVersion).toBe('1.2.0');

    const downloaded = await store.startDownload();
    expect(downloaded.state).toBe('readyToInstall');
    expect(store.state).toBe('readyToInstall');

    const installed = await store.install();
    expect(installed.state).toBe('idle');
  });

  it('surfaces structured errors from the state machine', async () => {
    mockInvoke('get_update_status', () => idle);
    mockInvoke('start_update_download', () => ({
      ...available,
      state: 'failed' as const,
      error: { stage: 'download', message: '网络停滞超过 30 秒', retryable: true },
    }));

    const store = useUpdateStore();
    await store.init();
    const result = await store.startDownload();

    expect(result.state).toBe('failed');
    expect(store.error?.stage).toBe('download');
    expect(store.error?.retryable).toBe(true);
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
