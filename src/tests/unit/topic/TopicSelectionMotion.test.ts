import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { nextTick } from 'vue';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import TopicList from '../../../features/topic/TopicList.vue';
import { useChatSessionStore } from '../../../core/stores/chatSessionStore';
import { useLayoutStore } from '../../../core/stores/layout';
import { useTopicStore, type Topic } from '../../../core/stores/topicListManager';

const navigation = vi.hoisted(() => ({
  currentRoute: { value: { path: '/chat' } },
  push: vi.fn(),
}));

vi.mock('vue-router', () => ({ useRouter: () => navigation }));
vi.mock('../../../features/chat/voice/voiceCallStore', () => ({
  useVoiceCallStore: () => ({ active: false }),
}));
vi.mock('../../../core/composables/useModalHistory', () => ({
  useModalHistory: () => ({ registerModal: vi.fn(), unregisterModal: vi.fn() }),
}));

let pinia: Pinia;
let wrapper: VueWrapper | undefined;
let frameId: number;
let frames: Map<number, FrameRequestCallback>;
let wideWindow: boolean;

beforeEach(() => {
  localStorage.clear();
  pinia = createPinia();
  setActivePinia(pinia);
  frameId = 0;
  frames = new Map();
  wideWindow = false;
  navigation.currentRoute.value.path = '/chat';
  navigation.push.mockReset().mockImplementation(async (path: string) => {
    navigation.currentRoute.value.path = path;
  });
  vi.spyOn(window, 'requestAnimationFrame').mockImplementation(callback => {
    frames.set(++frameId, callback);
    return frameId;
  });
  vi.spyOn(window, 'cancelAnimationFrame').mockImplementation(id => {
    frames.delete(id);
  });
  vi.spyOn(window, 'matchMedia').mockImplementation(query => ({
    matches: wideWindow,
    media: query,
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(() => true),
  }));
});

afterEach(() => {
  wrapper?.unmount();
  wrapper = undefined;
  disposePinia(pinia);
});

const topic = (id: string): Topic => ({
  id,
  name: `Topic ${id}`,
  createdAt: 1,
  updatedAt: 1,
  locked: true,
  unread: false,
  unreadCount: 0,
  msgCount: 0,
  ownerId: 'agent-a',
  ownerType: 'agent',
});

async function mountTopics(drawerOpen = true) {
  const layout = useLayoutStore();
  const session = useChatSessionStore();
  const topics = useTopicStore();
  topics.topics = [topic('first'), topic('second')];
  vi.spyOn(topics, 'loadTopicList').mockResolvedValue();
  const select = vi.spyOn(session, 'selectTopicById').mockResolvedValue();
  layout.setLeftDrawer(drawerOpen);
  wrapper = mount(TopicList, {
    attachTo: document.body,
    global: { directives: { longpress: {} } },
  });
  await nextTick();
  await flushPromises();
  return { layout, session, select };
}

function clickTopic(id: string) {
  const row = wrapper!.findAll('.glass-panel').find(item => item.text().includes(`Topic ${id}`));
  expect(row, `rendered topic ${id}`).toBeDefined();
  row!.element.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, detail: 1 }));
}

async function renderFrame() {
  const callbacks = [...frames.values()];
  frames.clear();
  for (const callback of callbacks) callback(performance.now());
  await nextTick();
  await flushPromises();
}

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>(finish => { resolve = finish; });
  return { promise, resolve };
}

describe('topic selection during drawer motion', () => {
  it('closes immediately and gives the drawer a paint opportunity before routing or selecting', async () => {
    navigation.currentRoute.value.path = '/welcome';
    const { layout, select } = await mountTopics();
    const operations: string[] = [];
    navigation.push.mockImplementation(async (path: string) => {
      operations.push(`route:${layout.leftDrawerOpen}`);
      navigation.currentRoute.value.path = path;
    });
    select.mockImplementation(async () => {
      operations.push(`select:${layout.leftDrawerOpen}`);
    });

    clickTopic('first');
    expect(layout.leftDrawerOpen).toBe(false);
    expect(navigation.push).not.toHaveBeenCalled();
    expect(select).not.toHaveBeenCalled();
    await flushPromises();
    expect(select).not.toHaveBeenCalled();

    await renderFrame();
    expect(navigation.push).not.toHaveBeenCalled();
    expect(select).not.toHaveBeenCalled();
    await renderFrame();
    expect(operations).toEqual(['route:false', 'select:false']);
    expect(navigation.push).toHaveBeenCalledWith('/chat');
    expect(select).toHaveBeenCalledExactlyOnceWith('agent-a', 'agent', 'first');
    expect(wrapper!.emitted('select-topic')).toHaveLength(1);
  });

  it.each([
    ['a permanent pane', true, true],
    ['an already closed drawer', false, false],
  ] as const)('does not delay selection for %s', async (_label, wide, open) => {
    wideWindow = wide;
    const { select } = await mountTopics(open);
    clickTopic('first');
    await flushPromises();
    expect(select).toHaveBeenCalledExactlyOnceWith('agent-a', 'agent', 'first');
    expect(navigation.push).not.toHaveBeenCalled();
    expect(frames.size).toBe(0);
  });

  it('replaces a selection waiting for paint with the newer topic intent', async () => {
    const { select } = await mountTopics();
    clickTopic('first');
    await flushPromises();
    await renderFrame();
    expect(select).not.toHaveBeenCalled();

    clickTopic('second');
    await flushPromises();
    await renderFrame();
    await renderFrame();
    expect(select).toHaveBeenCalledExactlyOnceWith('agent-a', 'agent', 'second');
    expect(wrapper!.emitted('select-topic')).toHaveLength(1);
    expect(frames.size).toBe(0);
  });

  it('cancels pending paint work on unmount without starting a session selection', async () => {
    const { select } = await mountTopics();
    clickTopic('first');
    await flushPromises();
    await renderFrame();
    wrapper!.unmount();
    wrapper = undefined;
    expect(frames.size).toBe(0);
    await renderFrame();
    await renderFrame();
    expect(select).not.toHaveBeenCalled();
    expect(navigation.push).not.toHaveBeenCalled();
  });

  it('does not overwrite a conversation selected elsewhere while waiting for paint', async () => {
    const { session, select } = await mountTopics();
    clickTopic('first');
    await flushPromises();
    await renderFrame();
    session.setConversation({ id: 'agent-b', name: 'B', type: 'agent' }, 'external-topic');
    await renderFrame();
    expect(select).not.toHaveBeenCalled();
    expect(session.currentTopicId).toBe('external-topic');
    expect(wrapper!.emitted('select-topic')).toBeUndefined();
  });

  it.each(['session-change', 'unmount'] as const)('rechecks a pending route after %s', async interruption => {
    navigation.currentRoute.value.path = '/welcome';
    const route = deferred();
    navigation.push.mockImplementation(() => route.promise);
    const { session, select } = await mountTopics();
    clickTopic('first');
    await flushPromises();
    await renderFrame();
    await renderFrame();
    expect(navigation.push).toHaveBeenCalledWith('/chat');
    expect(select).not.toHaveBeenCalled();

    if (interruption === 'session-change') {
      session.setConversation({ id: 'agent-b', name: 'B', type: 'agent' }, 'external-topic');
    } else {
      wrapper!.unmount();
      wrapper = undefined;
    }
    route.resolve();
    await flushPromises();
    expect(select).not.toHaveBeenCalled();
  });
});
