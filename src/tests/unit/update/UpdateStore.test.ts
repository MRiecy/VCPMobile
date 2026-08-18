import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useUpdateStore } from '../../../core/stores/update';
import { emitTauriEvent, invokeMock, mockInvoke } from '../../mocks/tauri';
import { UPDATE_STATUS_EVENT, type UpdateStatus } from '../../../core/types/update';

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
  beforeEach(() => {
    setActivePinia(createPinia());
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
});
