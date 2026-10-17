import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { effectScope, nextTick, type EffectScope } from 'vue';
import { useAutoUpdate } from '@/core/composables/useAutoUpdate';
import { useAppLifecycleStore } from '@/core/stores/appLifecycle';
import { useUpdateStore } from '@/core/stores/update';
import { invokeMock, listenMock, mockInvoke } from '@/tests/mocks/tauri';
import type { UpdateStatus } from '@/core/types/update';

const platform = vi.hoisted(() => ({ android: true }));
vi.mock('@/core/utils/platform', () => ({
  get isAndroid() { return platform.android; },
  get isIOS() { return !platform.android; },
}));
vi.mock('@/core/stores/appLifecycle', async () => {
  const { defineStore } = await import('pinia');
  const { ref } = await import('vue');
  return { useAppLifecycleStore: defineStore('appLifecycle', () => ({ state: ref('BOOTING') })) };
});

const idle: UpdateStatus = {
  state: 'idle', info: null, downloaded: 0, total: null, error: null,
};
const available: UpdateStatus = {
  ...idle,
  state: 'available',
  info: {
    hasUpdate: true,
    currentVersion: '1.1.6',
    latestVersion: '1.2.0',
    releasePageUrl: 'https://github.com/MRiecy/VCPMobile/releases/tag/v1.2.0',
    releaseNotes: 'notes',
    apkSize: 100,
    apkSha256: 'a'.repeat(64),
  },
};
const checkCalls = () => invokeMock.mock.calls.filter(([command]) => command === 'check_for_update');

describe('automatic update scheduling with the shared preference', () => {
  let pinia: Pinia;
  let scope: EffectScope;

  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-10-18T00:00:00Z'));
    localStorage.clear();
    platform.android = true;
    pinia = createPinia();
    setActivePinia(pinia);
    scope = effectScope();
    mockInvoke('get_update_status', () => idle);
    mockInvoke('check_for_update', () => available);
  });

  afterEach(() => {
    scope.stop();
    disposePinia(pinia);
    vi.useRealTimers();
  });

  const ready = async () => {
    useAppLifecycleStore().state = 'READY';
    await nextTick();
  };

  it('waits until READY plus two seconds and checks at most once in the current session', async () => {
    scope.run(() => useAutoUpdate());
    await vi.advanceTimersByTimeAsync(5000);
    expect(checkCalls()).toHaveLength(0);
    await ready();
    await vi.advanceTimersByTimeAsync(1999);
    expect(checkCalls()).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(1);
    expect(checkCalls()).toHaveLength(1);
    expect(localStorage.getItem('vcp_last_update_check')).toBe(String(Date.now()));
    expect(useUpdateStore().isPromptOpen).toBe(true);

    useAppLifecycleStore().state = 'CONNECTING';
    await nextTick();
    await ready();
    await vi.advanceTimersByTimeAsync(2000);
    expect(checkCalls()).toHaveLength(1);
  });

  it('consumes a preference change before the scheduled check without checking immediately when enabled later', async () => {
    scope.run(() => useAutoUpdate());
    await ready();
    const store = useUpdateStore();
    store.setAutoCheckEnabled(false);
    await vi.advanceTimersByTimeAsync(2000);
    expect(checkCalls()).toHaveLength(0);
    store.setAutoCheckEnabled(true);
    await vi.advanceTimersByTimeAsync(3000);
    expect(checkCalls()).toHaveLength(0);
    useAppLifecycleStore().state = 'CONNECTING';
    await nextTick();
    await ready();
    await vi.advanceTimersByTimeAsync(2000);
    expect(checkCalls()).toHaveLength(0);
  });

  it.each([
    [23 * 60 * 60 * 1000, 0],
    [25 * 60 * 60 * 1000, 1],
  ])('preserves the cooldown when the last successful check was %i milliseconds ago', async (elapsed, count) => {
    localStorage.setItem('vcp_last_update_check', String(Date.now() - elapsed));
    scope.run(() => useAutoUpdate());
    await ready();
    await vi.advanceTimersByTimeAsync(2000);
    expect(checkCalls()).toHaveLength(count);
  });

  it('keeps skipped-version suppression independent from the automatic check preference', async () => {
    localStorage.setItem('vcp_update_skipped_version', '1.2.0');
    scope.run(() => useAutoUpdate());
    await ready();
    await vi.advanceTimersByTimeAsync(2000);
    expect(checkCalls()).toHaveLength(1);
    expect(useUpdateStore().isPromptOpen).toBe(false);
    expect(useUpdateStore().autoCheckEnabled).toBe(true);
  });

  it('does not consume the cooldown after a failed check', async () => {
    mockInvoke('check_for_update', async () => { throw new Error('network offline'); });
    vi.spyOn(console, 'error').mockImplementation(() => {});
    scope.run(() => useAutoUpdate());
    await ready();
    await vi.advanceTimersByTimeAsync(2000);
    expect(checkCalls()).toHaveLength(1);
    expect(localStorage.getItem('vcp_last_update_check')).toBeNull();
    expect(useUpdateStore().isPromptOpen).toBe(false);
  });

  it('cancels a pending check when its scope is disposed', async () => {
    scope.run(() => useAutoUpdate());
    await ready();
    scope.stop();
    await vi.advanceTimersByTimeAsync(2000);
    expect(checkCalls()).toHaveLength(0);
  });

  it('never initializes the APK updater on iOS even with automatic checking enabled', async () => {
    platform.android = false;
    scope.run(() => useAutoUpdate());
    await ready();
    await vi.advanceTimersByTimeAsync(5000);
    expect(useUpdateStore().autoCheckEnabled).toBe(true);
    expect(invokeMock).not.toHaveBeenCalled();
    expect(listenMock).not.toHaveBeenCalled();
  });
});
