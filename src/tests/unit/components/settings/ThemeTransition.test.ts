import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { flushPromises } from '@vue/test-utils';
import { useThemeStore } from '@/core/stores/theme';

function deferred() {
  let resolve!: () => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<void>((accept, fail) => { resolve = accept; reject = fail; });
  return { promise, resolve, reject };
}

function installTransitions() {
  const transitions: Array<{
    run: () => Promise<void>;
    ready: ReturnType<typeof deferred>;
    finished: ReturnType<typeof deferred>;
    skipTransition: ReturnType<typeof vi.fn>;
  }> = [];
  const start = vi.fn((update: () => Promise<void>) => {
    const ready = deferred();
    const updated = deferred();
    const finished = deferred();
    const skipTransition = vi.fn();
    transitions.push({
      ready, finished, skipTransition,
      async run() { await update(); updated.resolve(); ready.resolve(); },
    });
    return { ready: ready.promise, updateCallbackDone: updated.promise, finished: finished.promise, skipTransition };
  });
  Object.defineProperty(document, 'startViewTransition', { configurable: true, value: start });
  return { start, transitions };
}

describe('theme transition ownership', () => {
  let pinia: Pinia;

  beforeEach(() => {
    pinia = createPinia();
    setActivePinia(pinia);
    localStorage.clear();
    document.documentElement.className = '';
    document.documentElement.style.cssText = '';
    vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
    vi.stubGlobal('requestIdleCallback', vi.fn());
    Reflect.deleteProperty(document, 'startViewTransition');
  });

  afterEach(() => {
    disposePinia(pinia);
    Reflect.deleteProperty(document, 'startViewTransition');
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it.each(['resolve', 'reject'] as const)('retains suppression until finished settles by %s', async (end) => {
    const store = useThemeStore();
    await store.initTheme();
    const { transitions } = installTransitions();
    const applying = store.applyThemeFile('themes-simple-bw.ts');
    await transitions[0].run();
    expect((await applying).ok).toBe(true);
    expect(document.documentElement.classList.contains('theme-switching')).toBe(true);

    if (end === 'resolve') transitions[0].finished.resolve();
    else transitions[0].finished.reject(new Error('transition failed'));
    await flushPromises();
    expect(document.documentElement.classList.contains('theme-switching')).toBe(false);
  });

  it('does not let a skipped older transition release the newer switch', async () => {
    const store = useThemeStore();
    await store.initTheme();
    const { transitions } = installTransitions();
    const first = store.applyThemeFile('themes-simple-bw.ts');
    await transitions[0].run();
    await first;
    const second = store.applyThemeFile('themes-bear-holiday.ts');
    expect(transitions[0].skipTransition).toHaveBeenCalledOnce();
    await transitions[1].run();
    await second;

    transitions[0].finished.resolve();
    await flushPromises();
    expect(document.documentElement.classList.contains('theme-switching')).toBe(true);
    expect(store.currentTheme).toBe('themes-bear-holiday.ts');
    transitions[1].finished.resolve();
    await flushPromises();
    expect(document.documentElement.classList.contains('theme-switching')).toBe(false);
  });

  it('still releases a skipped transition when the next requested theme is invalid', async () => {
    const store = useThemeStore();
    await store.initTheme();
    const { transitions } = installTransitions();
    const applying = store.applyThemeFile('themes-simple-bw.ts');
    await transitions[0].run();
    await applying;

    expect((await store.applyThemeFile('../missing.ts')).ok).toBe(false);
    transitions[0].finished.resolve();
    await flushPromises();
    expect(document.documentElement.classList.contains('theme-switching')).toBe(false);
  });

  it('settles DOM styles under suppression before releasing a switch without snapshots', async () => {
    const store = useThemeStore();
    await store.initTheme();
    const observed: boolean[] = [];
    vi.spyOn(document.documentElement, 'getBoundingClientRect').mockImplementation(() => {
      observed.push(document.documentElement.classList.contains('theme-switching'));
      return new DOMRect();
    });

    const applying = store.applyThemeFile('themes-simple-bw.ts');
    expect(document.documentElement.classList.contains('theme-switching')).toBe(true);
    expect((await applying).ok).toBe(true);
    expect(observed).toEqual([true]);
    expect(document.documentElement.classList.contains('theme-switching')).toBe(false);
  });

  it('does not let a pending snapshot-free cleanup release a newer transition', async () => {
    const store = useThemeStore();
    await store.initTheme();
    const first = store.applyThemeFile('themes-simple-bw.ts');
    const { transitions } = installTransitions();
    const second = store.applyThemeFile('themes-bear-holiday.ts');
    await transitions[0].run();
    await Promise.all([first, second]);
    expect(document.documentElement.classList.contains('theme-switching')).toBe(true);
    transitions[0].finished.resolve();
    await flushPromises();
    expect(document.documentElement.classList.contains('theme-switching')).toBe(false);
  });

  it('uses snapshot-free cleanup if snapshot creation throws', async () => {
    const store = useThemeStore();
    await store.initTheme();
    const { start } = installTransitions();
    start.mockImplementation(() => { throw new Error('snapshot unavailable'); });

    expect((await store.applyThemeFile('themes-simple-bw.ts')).ok).toBe(true);
    expect(start).toHaveBeenCalledOnce();
    expect(store.currentTheme).toBe('themes-simple-bw.ts');
    expect(document.documentElement.classList.contains('theme-switching')).toBe(false);
  });

  it('restores preferences without snapshots and leaves no suppression marker', async () => {
    const store = useThemeStore();
    await store.initTheme();
    const { start } = installTransitions();
    expect((await store.restorePreferences({ themeKey: 'themes-simple-bw.ts', mode: 'dark' })).ok).toBe(true);
    expect(start).not.toHaveBeenCalled();
    expect(document.documentElement.classList.contains('dark')).toBe(true);
    expect(document.documentElement.classList.contains('theme-switching')).toBe(false);
  });

  it('releases ownership on disposal even when the browser callback is still pending', async () => {
    const store = useThemeStore();
    await store.initTheme();
    const { transitions } = installTransitions();
    const applying = store.applyThemeFile('themes-simple-bw.ts');
    store.$dispose();
    expect((await applying).ok).toBe(true);
    expect(transitions[0].skipTransition).toHaveBeenCalledOnce();
    expect(document.documentElement.classList.contains('theme-switching')).toBe(false);
    await transitions[0].run();
    transitions[0].finished.resolve();
    await flushPromises();
    expect(document.documentElement.classList.contains('theme-switching')).toBe(false);
  });
});
