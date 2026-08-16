import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia, getActivePinia } from 'pinia';
import piniaPluginPersistedstate from 'pinia-plugin-persistedstate';
import { nextTick, ref } from 'vue';
import { getVersion } from '@tauri-apps/api/app';
import {
  DEFAULT_SETTLE_MS,
  QUEUE_RESUME_DELAY_MS,
  REPLAY_PREPARE_DELAY_MS,
  useGuideStore,
} from '@/features/guide/stores/guideStore';
import { defineGuide } from '@/features/guide/registry';
import { useModalHistory } from '@/core/composables/useModalHistory';
import { useNotificationStore } from '@/core/stores/notification';
import { flushPromises } from '@/tests/utils/flush';

vi.mock('@tauri-apps/api/app', () => ({
  getVersion: vi.fn(() => Promise.resolve(undefined)),
}));

function createStore() {
  const pinia = createPinia();
  pinia.use(piniaPluginPersistedstate);
  // pinia v3 的 use() 仅把插件排队，需 install() 才注入到后续创建的 store。
  const app = {
    provide: () => undefined,
    config: { globalProperties: {} },
  } as unknown as import('vue').App;
  pinia.install(app);
  setActivePinia(pinia);
  return useGuideStore();
}

function stopActivePiniaEffects() {
  const pinia = getActivePinia() as unknown as { _e?: { stop: () => void } } | null;
  pinia?._e?.stop();
}

/**
 * 指引定义注册在全局 registry（模块级），predicates 闭包引用这些 ref。
 * 旧 store 的 watchEffect 已被 stopActivePiniaEffects 停掉，但后续测试的
 * 新 store 仍会评估所有已注册指引——因此测试结束时必须把旗标复位，
 * 否则上一个测试留下的 true 会幽灵触发后续测试的指引。
 */
const flagResets: Array<() => void> = [];

function trackedFlag(initial = false) {
  const flag = ref(initial);
  flagResets.push(() => {
    flag.value = initial;
  });
  return flag;
}

afterEach(() => {
  vi.useRealTimers();
  flagResets.splice(0).forEach((reset) => reset());
  const store = useGuideStore();
  store.pendingQueue = [];
  while (store.activeGuideId) {
    store.finish();
  }
  stopActivePiniaEffects();
});

beforeEach(() => {
  localStorage.clear();
});

describe('guideStore playback state machine', () => {
  it('walks start → next → finish and records completion once', () => {
    defineGuide({
      id: 'st-walk',
      title: 'walk',
      description: 'walk',
      steps: [
        { target: 't1', title: '一', content: 'c1' },
        { target: 't2', title: '二', content: 'c2' },
      ],
    });
    const store = createStore();
    store.start('st-walk');
    expect(store.activeGuideId).toBe('st-walk');
    expect(store.activeStepIndex).toBe(0);

    store.next();
    expect(store.activeStepIndex).toBe(1);

    store.next(); // 末步 next 即 finish
    expect(store.activeGuideId).toBeNull();
    expect(store.isCompleted('st-walk')).toBe(true);

    // 重复 finish 不重复写入
    store.completed = [];
    store.start('st-walk');
    store.finish();
    store.start('st-walk');
    store.finish();
    expect(store.completed).toEqual(['st-walk']);
  });

  it('refuses the back key while a guide is active', () => {
    defineGuide({
      id: 'st-back',
      title: 'back',
      description: 'back',
      steps: [{ target: 't1', title: '一', content: 'c1' }],
    });
    const { closeTopModal, modalStackLength } = useModalHistory();
    const store = createStore();
    store.start('st-back');

    expect(closeTopModal()).toBe(true); // 已消费一次返回手势…
    expect(store.activeGuideId).toBe('st-back'); // …但指引拒绝关闭
    expect(modalStackLength()).toBe(1);

    store.finish();
    expect(modalStackLength()).toBe(0);
  });

  it('persists only completed and lastSeenAppVersion', async () => {
    defineGuide({
      id: 'st-persist',
      title: 'persist',
      description: 'persist',
      steps: [{ target: 't1', title: '一', content: 'c1' }],
    });
    const store = createStore();
    store.start('st-persist');
    store.next();
    store.lastSeenAppVersion = '1.2.3';
    await flushPromises(); // persist 插件经 $subscribe(flush: 'pre') 微任务落盘

    const raw = localStorage.getItem('guide');
    expect(raw).toBeTruthy();
    const parsed = JSON.parse(raw as string);
    expect(parsed).toHaveProperty('completed');
    expect(parsed).toHaveProperty('lastSeenAppVersion', '1.2.3');
    expect(parsed).not.toHaveProperty('activeGuideId');
    expect(parsed).not.toHaveProperty('pendingQueue');
  });
});

describe('guideStore trigger evaluation', () => {
  it('starts a guide after predicates hold for settleMs', async () => {
    vi.useFakeTimers();
    const flag = trackedFlag();
    defineGuide({
      id: 'tr-settle',
      title: 'settle',
      description: 'settle',
      trigger: {
        predicates: [{ name: 'flag', check: () => flag.value }],
      },
      steps: [{ target: 't1', title: '一', content: 'c1' }],
    });
    const store = createStore();
    store.init();
    await nextTick();

    flag.value = true;
    await nextTick();
    vi.advanceTimersByTime(DEFAULT_SETTLE_MS - 100);
    expect(store.activeGuideId).toBeNull();

    vi.advanceTimersByTime(200);
    expect(store.activeGuideId).toBe('tr-settle');
  });

  it('cancels the settle timer when predicates flip back', async () => {
    vi.useFakeTimers();
    const flag = trackedFlag();
    defineGuide({
      id: 'tr-flip',
      title: 'flip',
      description: 'flip',
      trigger: {
        predicates: [{ name: 'flag', check: () => flag.value }],
      },
      steps: [{ target: 't1', title: '一', content: 'c1' }],
    });
    const store = createStore();
    store.init();
    await nextTick();

    flag.value = true;
    await nextTick();
    vi.advanceTimersByTime(200);
    flag.value = false;
    await nextTick();
    vi.advanceTimersByTime(DEFAULT_SETTLE_MS * 2);
    expect(store.activeGuideId).toBeNull();
  });

  it('blocks a guide until its requires chain is completed', async () => {
    vi.useFakeTimers();
    const flagA = trackedFlag();
    const flagB = trackedFlag();
    defineGuide({
      id: 'tr-chain-a',
      title: 'a',
      description: 'a',
      trigger: {
        predicates: [{ name: 'a', check: () => flagA.value }],
      },
      steps: [{ target: 't1', title: '一', content: 'c1' }],
    });
    defineGuide({
      id: 'tr-chain-b',
      title: 'b',
      description: 'b',
      trigger: {
        requires: ['tr-chain-a'],
        predicates: [{ name: 'b', check: () => flagB.value }],
      },
      steps: [{ target: 't1', title: '一', content: 'c1' }],
    });
    const store = createStore();
    store.init();
    await nextTick();

    flagB.value = true; // B 条件满足但依赖未完成
    await nextTick();
    vi.advanceTimersByTime(DEFAULT_SETTLE_MS * 2);
    expect(store.activeGuideId).toBeNull();

    flagA.value = true;
    await nextTick();
    vi.advanceTimersByTime(DEFAULT_SETTLE_MS + 100);
    expect(store.activeGuideId).toBe('tr-chain-a');

    store.finish();
    await nextTick();
    vi.advanceTimersByTime(DEFAULT_SETTLE_MS + 100);
    expect(store.activeGuideId).toBe('tr-chain-b');
  });

  it('plays simultaneously-ready guides serially through the queue', async () => {
    vi.useFakeTimers();
    const flagA = trackedFlag();
    const flagB = trackedFlag();
    defineGuide({
      id: 'tr-queue-a',
      title: 'a',
      description: 'a',
      trigger: {
        predicates: [{ name: 'a', check: () => flagA.value }],
      },
      steps: [{ target: 't1', title: '一', content: 'c1' }],
    });
    defineGuide({
      id: 'tr-queue-b',
      title: 'b',
      description: 'b',
      trigger: {
        predicates: [{ name: 'b', check: () => flagB.value }],
      },
      steps: [{ target: 't1', title: '一', content: 'c1' }],
    });
    const store = createStore();
    store.init();
    await nextTick();

    flagA.value = true;
    flagB.value = true;
    await nextTick();
    vi.advanceTimersByTime(DEFAULT_SETTLE_MS + 100);
    expect(store.activeGuideId).toBe('tr-queue-a');
    expect(store.pendingQueue).toEqual(['tr-queue-b']);

    store.finish();
    await nextTick();
    expect(store.activeGuideId).toBeNull();
    vi.advanceTimersByTime(QUEUE_RESUME_DELAY_MS + 50);
    expect(store.activeGuideId).toBe('tr-queue-b');
  });

  it('keeps the queue strictly FIFO: a late-settling guide queues behind instead of jumping the resume gap', async () => {
    vi.useFakeTimers();
    const flagA = trackedFlag();
    const flagB = trackedFlag();
    const flagC = trackedFlag();
    defineGuide({
      id: 'tr-fifo-a',
      title: 'a',
      description: 'a',
      trigger: {
        predicates: [{ name: 'a', check: () => flagA.value }],
      },
      steps: [{ target: 't1', title: '一', content: 'c1' }],
    });
    defineGuide({
      id: 'tr-fifo-b',
      title: 'b',
      description: 'b',
      trigger: {
        predicates: [{ name: 'b', check: () => flagB.value }],
      },
      steps: [{ target: 't1', title: '一', content: 'c1' }],
    });
    defineGuide({
      id: 'tr-fifo-c',
      title: 'c',
      description: 'c',
      trigger: {
        // 短稳定期：恰好在 A 收尾后、B 恢复前的 250ms 间隙内到点
        settleMs: 100,
        predicates: [{ name: 'c', check: () => flagC.value }],
      },
      steps: [{ target: 't1', title: '一', content: 'c1' }],
    });
    const store = createStore();
    store.init();
    await nextTick();

    flagA.value = true;
    flagB.value = true;
    await nextTick();
    vi.advanceTimersByTime(DEFAULT_SETTLE_MS + 100);
    expect(store.activeGuideId).toBe('tr-fifo-a');
    expect(store.pendingQueue).toEqual(['tr-fifo-b']);

    store.finish(); // B 在 250ms 后恢复
    await nextTick();
    flagC.value = true; // C 在间隙内到点（settleMs=100 < 250）
    await nextTick();
    vi.advanceTimersByTime(120);
    expect(store.activeGuideId).toBeNull(); // 未插队
    expect(store.pendingQueue).toEqual(['tr-fifo-b', 'tr-fifo-c']);

    vi.advanceTimersByTime(QUEUE_RESUME_DELAY_MS + 50);
    expect(store.activeGuideId).toBe('tr-fifo-b');
  });
});

describe('guideStore replay', () => {
  it('bypasses triggers, runs prepare, and starts after the prepare delay', async () => {
    vi.useFakeTimers();
    const prepare = vi.fn();
    defineGuide({
      id: 'rp-manual',
      title: 'replay',
      description: 'replay',
      prepare,
      steps: [{ target: 't1', title: '一', content: 'c1' }],
    });
    const store = createStore();
    store.replay('rp-manual');

    expect(store.activeGuideId).toBeNull();
    expect(prepare).toHaveBeenCalledTimes(1);

    vi.advanceTimersByTime(REPLAY_PREPARE_DELAY_MS + 50);
    expect(store.activeGuideId).toBe('rp-manual');
  });
});

describe('guideStore resetProgress', () => {
  it('restores first-run state and persists the reset', async () => {
    defineGuide({
      id: 'rs-a',
      title: 'a',
      description: 'a',
      steps: [{ target: 't1', title: '一', content: 'c1' }],
    });
    const store = createStore();
    store.completed = ['rs-a'];
    store.lastSeenAppVersion = '1.2.3';
    store.pendingQueue = ['rs-a'];

    store.resetProgress();
    expect(store.completed).toEqual([]);
    expect(store.lastSeenAppVersion).toBeNull();
    expect(store.pendingQueue).toEqual([]);

    await flushPromises(); // persist 插件微任务落盘
    const parsed = JSON.parse(localStorage.getItem('guide') as string);
    expect(parsed.completed).toEqual([]);
    expect(parsed.lastSeenAppVersion).toBeNull();
  });

  it('finishes the active guide before clearing completion', () => {
    defineGuide({
      id: 'rs-active',
      title: 'active',
      description: 'active',
      steps: [{ target: 't1', title: '一', content: 'c1' }],
    });
    const store = createStore();
    store.start('rs-active');
    expect(store.activeGuideId).toBe('rs-active');

    store.resetProgress();
    expect(store.activeGuideId).toBeNull();
    // finish 会把当前 id 写回 completed，随后被整体清空
    expect(store.completed).toEqual([]);
  });
});

describe('guideStore version gating', () => {
  it('queues introduced guides and toasts once when the app version moved past them', async () => {
    vi.mocked(getVersion).mockResolvedValue('1.2.0');
    defineGuide({
      id: 'vg-new',
      title: 'new',
      description: 'new',
      introducedIn: '1.1.0',
      steps: [{ target: 't1', title: '一', content: 'c1' }],
    });
    const store = createStore();
    store.lastSeenAppVersion = '1.0.0';
    store.init();
    await flushPromises();

    expect(store.pendingQueue).toContain('vg-new');
    expect(store.lastSeenAppVersion).toBe('1.2.0');
    const notificationStore = useNotificationStore();
    expect(notificationStore.activeToasts.length).toBeGreaterThan(0);
  });

  it('ignores guides already completed and versions already seen', async () => {
    vi.mocked(getVersion).mockResolvedValue('1.2.0');
    defineGuide({
      id: 'vg-seen',
      title: 'seen',
      description: 'seen',
      introducedIn: '1.1.0',
      steps: [{ target: 't1', title: '一', content: 'c1' }],
    });
    const store = createStore();
    store.lastSeenAppVersion = '1.1.5'; // 已见过新版本
    store.init();
    await flushPromises();

    expect(store.pendingQueue).not.toContain('vg-seen');
  });

  it('degrades silently when getVersion fails', async () => {
    vi.mocked(getVersion).mockRejectedValue(new Error('no tauri'));
    defineGuide({
      id: 'vg-fail',
      title: 'fail',
      description: 'fail',
      introducedIn: '9.9.9',
      steps: [{ target: 't1', title: '一', content: 'c1' }],
    });
    const store = createStore();
    expect(() => store.init()).not.toThrow();
    await flushPromises();
    expect(store.activeGuideId).toBeNull();
  });
});
