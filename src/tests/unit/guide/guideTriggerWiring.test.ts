/**
 * 真实指引定义的接线行为测试：
 * - 页面栈遮挡（workspace-not-occluded）阻止 sidebar/plus 在
 *   SlidePage 覆盖工作区时误触发；
 * - requires 依赖链 + 注册表顺序 + FIFO 队列的串行播放语义。
 */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia, getActivePinia } from 'pinia';
import { nextTick } from 'vue';
import '@/features/guide/guides';
import { registerTarget, unregisterTarget } from '@/features/guide/registry';
import { useGuideStore, DEFAULT_SETTLE_MS, QUEUE_RESUME_DELAY_MS } from '@/features/guide/stores/guideStore';
import { useAssistantStore } from '@/core/stores/assistant';
import { useLayoutStore } from '@/core/stores/layout';
import { useOverlayStore } from '@/core/stores/overlay';
import { useChatSessionStore } from '@/core/stores/chatSessionStore';
import { useChatHistoryStore } from '@/core/stores/chatHistoryStore';
import { useTopicStore } from '@/core/stores/topicListManager';

vi.mock('@tauri-apps/api/app', () => ({
  getVersion: vi.fn(() => Promise.resolve('1.1.4')),
}));

function createStore() {
  const pinia = createPinia();
  setActivePinia(pinia);
  return useGuideStore();
}

function stopActivePiniaEffects() {
  const pinia = getActivePinia() as unknown as { _e?: { stop: () => void } } | null;
  pinia?._e?.stop();
}

afterEach(() => {
  vi.useRealTimers();
  const store = useGuideStore();
  store.pendingQueue = [];
  while (store.activeGuideId) {
    store.finish();
  }
  useLayoutStore().setLeftDrawer(false);
  unregisterTarget('agent-row-a1', document.body);
  stopActivePiniaEffects();
});

function seedAgentsAndRow() {
  const assistantStore = useAssistantStore();
  assistantStore.agents = [
    { id: 'a1', name: 'A1', model: 'm' },
    { id: 'a2', name: 'A2', model: 'm' },
  ] as never;
  registerTarget('agent-row-a1', document.body);
}

function seedMatureTopic() {
  const sessionStore = useChatSessionStore();
  sessionStore.currentTopicId = 't1';
  sessionStore.currentSelectedItem = { id: 'a1', type: 'agent' };
  useTopicStore().topics = [{ id: 't1', name: '已总结的话题标题' }] as never;
  useChatHistoryStore().currentChatHistory = [
    { role: 'user' },
    { role: 'assistant' },
    { role: 'user' },
    { role: 'assistant' },
  ] as never;
}

describe('shipped guide trigger wiring', () => {
  it('keeps sidebar-gestures silent while a page covers the workspace, then fires when it closes', async () => {
    vi.useFakeTimers();
    const store = createStore();
    seedAgentsAndRow();
    useLayoutStore().setLeftDrawer(true);
    useOverlayStore().pageStack = [{ type: 'settings', modalId: 'Page:settings:' }] as never;

    store.init();
    await nextTick();
    vi.advanceTimersByTime(DEFAULT_SETTLE_MS * 2);
    expect(store.activeGuideId).toBeNull(); // 页面盖住工作区：保持沉默

    useOverlayStore().pageStack = [] as never;
    await nextTick();
    vi.advanceTimersByTime(DEFAULT_SETTLE_MS + 100);
    expect(store.activeGuideId).toBe('sidebar-gestures');
    expect(store.pendingQueue).toEqual([]); // 抽屉开着，plus 的 drawers-closed 不满足
  });

  it('keeps plus-longpress silent while a page covers the workspace, then fires when it closes', async () => {
    vi.useFakeTimers();
    const store = createStore();
    seedMatureTopic(); // 输入框解锁 + 抽屉关闭 + 无依赖 → 唯一阻挡是页面遮挡
    useOverlayStore().pageStack = [{ type: 'settings', modalId: 'Page:settings:' }] as never;

    store.init();
    await nextTick();
    vi.advanceTimersByTime(DEFAULT_SETTLE_MS * 2);
    expect(store.activeGuideId).toBeNull();

    useOverlayStore().pageStack = [] as never;
    await nextTick();
    vi.advanceTimersByTime(DEFAULT_SETTLE_MS + 100);
    expect(store.activeGuideId).toBe('plus-longpress');
  });

  it('chains theme-longpress only after sidebar-gestures completes (requires)', async () => {
    vi.useFakeTimers();
    const store = createStore();
    seedAgentsAndRow();
    useLayoutStore().setLeftDrawer(true);
    seedMatureTopic();
    useOverlayStore().pageStack = [] as never;

    store.init();
    await nextTick();
    vi.advanceTimersByTime(DEFAULT_SETTLE_MS + 100);
    expect(store.activeGuideId).toBe('sidebar-gestures');

    // 完成 sidebar 并关抽屉：theme 依赖链解锁；此时 theme 与 plus 同时
    // 齐备，注册表顺序 theme 在前（plus 此前被抽屉打开挡住，从未入队）。
    store.finish();
    useLayoutStore().setLeftDrawer(false);
    await nextTick();
    vi.advanceTimersByTime(QUEUE_RESUME_DELAY_MS + DEFAULT_SETTLE_MS + 100);
    expect(store.activeGuideId).toBe('theme-longpress');
    expect(store.pendingQueue).toEqual(['plus-longpress']);
    store.finish();
    await nextTick();
    vi.advanceTimersByTime(QUEUE_RESUME_DELAY_MS + 50);
    expect(store.activeGuideId).toBe('plus-longpress');
  });

  it('re-triggers sidebar-gestures after resetProgress for repeat testing', async () => {
    vi.useFakeTimers();
    const store = createStore();
    seedAgentsAndRow();
    useLayoutStore().setLeftDrawer(true);
    useOverlayStore().pageStack = [] as never;

    store.init();
    await nextTick();
    vi.advanceTimersByTime(DEFAULT_SETTLE_MS + 100);
    expect(store.activeGuideId).toBe('sidebar-gestures');

    store.finish();
    expect(store.isCompleted('sidebar-gestures')).toBe(true);

    store.resetProgress();
    await nextTick();
    vi.advanceTimersByTime(DEFAULT_SETTLE_MS + 100);
    expect(store.activeGuideId).toBe('sidebar-gestures'); // 条件仍在 → 重新自动触发
  });
});
