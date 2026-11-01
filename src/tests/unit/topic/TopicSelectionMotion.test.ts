import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { nextTick } from 'vue';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import TopicList from '../../../features/topic/TopicList.vue';
import { mockInvoke } from '../../mocks/tauri';
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
let wideWindow: boolean;

beforeEach(() => {
  localStorage.clear();
  pinia = createPinia();
  setActivePinia(pinia);
  wideWindow = false;
  navigation.currentRoute.value.path = '/chat';
  mockInvoke('connect_chat_view', () => ({ documentId: 1 }));
  mockInvoke('register_chat_navigation_intent', () => ({ focus: { focusRevision: 0 } }));
  navigation.push.mockReset().mockImplementation(async (path: string) => {
    navigation.currentRoute.value.path = path;
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
  const select = vi.spyOn(session, 'selectTopicById').mockResolvedValue('accepted');
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


function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>(finish => { resolve = finish; });
  return { promise, resolve };
}

describe('topic selection during drawer motion', () => {
  it('keeps the drawer and route unchanged until the destination has been admitted', async () => {
    navigation.currentRoute.value.path = '/welcome';
    const pending = deferred();
    const { layout, select } = await mountTopics();
    select.mockImplementation(async () => { await pending.promise; return 'accepted'; });
    clickTopic('first');
    expect(select).toHaveBeenCalledWith('agent-a', 'agent', 'first', expect.any(Number));
    expect(layout.leftDrawerOpen).toBe(true);
    expect(navigation.push).not.toHaveBeenCalled();
    pending.resolve();
    await flushPromises();
    expect(layout.leftDrawerOpen).toBe(false);
    expect(navigation.push).toHaveBeenCalledWith('/chat');
    expect(wrapper!.emitted('select-topic')).toHaveLength(1);
  });

  it.each([false, true])('leaves the current drawer intact on call rejection (wide=%s)', async wide => {
    wideWindow = wide;
    const { layout, select, session } = await mountTopics();
    const notify = vi.spyOn(session, 'notifyNavigationResult');
    select.mockResolvedValue('call_in_progress');
    clickTopic('first');
    await flushPromises();
    expect(layout.leftDrawerOpen).toBe(true);
    expect(navigation.push).not.toHaveBeenCalled();
    expect(wrapper!.emitted('select-topic')).toBeUndefined();
    expect(notify).toHaveBeenCalledExactlyOnceWith('call_in_progress');
  });

  it.each(['new-topic', 'unmount'] as const)('ignores a late admission after %s', async interruption => {
    navigation.currentRoute.value.path = '/welcome';
    const pending = deferred();
    const { layout, select } = await mountTopics();
    select.mockImplementationOnce(async () => { await pending.promise; return 'accepted'; });
    clickTopic('first');
    if (interruption === 'new-topic') {
      select.mockResolvedValue('missing');
      clickTopic('second');
    } else {
      wrapper!.unmount();
      wrapper = undefined;
    }
    pending.resolve();
    await flushPromises();
    expect(layout.leftDrawerOpen).toBe(true);
    expect(navigation.push).not.toHaveBeenCalled();
  });
});
