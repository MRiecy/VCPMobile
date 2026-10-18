import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useDeveloperSettingsStore } from '@/features/settings/developer/store';

describe('device-local developer settings', () => {
  beforeEach(() => {
    const storage = new Map<string, string>();
    vi.stubGlobal('localStorage', {
      getItem: vi.fn((key: string) => storage.get(key) ?? null),
      setItem: vi.fn((key: string, value: string) => { storage.set(key, value); }),
    });
    setActivePinia(createPinia());
  });
  afterEach(() => vi.unstubAllGlobals());

  it('starts locked and restores only the persisted enabled value in a new store', () => {
    const store = useDeveloperSettingsStore();
    expect(store.enabled).toBe(false);
    expect(store.setEnabled(true)).toEqual({ ok: true });
    setActivePinia(createPinia());
    const reopened = useDeveloperSettingsStore();
    expect(reopened.enabled).toBe(true);
    expect(reopened.setEnabled(false)).toEqual({ ok: true });
    setActivePinia(createPinia());
    expect(useDeveloperSettingsStore().enabled).toBe(false);
  });

  it('does not unlock on an unreadable or malformed stored preference', () => {
    localStorage.setItem('vcp-developer-mode', 'enabled');
    expect(useDeveloperSettingsStore().enabled).toBe(false);
    setActivePinia(createPinia());
    vi.spyOn(localStorage, 'getItem').mockImplementation(() => { throw new Error('storage unavailable'); });
    expect(useDeveloperSettingsStore().enabled).toBe(false);
  });

  it.each([false, true])('preserves the acknowledged %s state when its next write fails', (enabled) => {
    const store = useDeveloperSettingsStore();
    expect(store.setEnabled(enabled).ok).toBe(true);
    vi.spyOn(localStorage, 'setItem').mockImplementation(() => { throw new Error('quota exceeded'); });
    expect(store.setEnabled(!enabled)).toEqual({ ok: false, error: 'Error: quota exceeded' });
    expect(store.enabled).toBe(enabled);
    expect(localStorage.getItem('vcp-developer-mode')).toBe(String(enabled));
  });
});
