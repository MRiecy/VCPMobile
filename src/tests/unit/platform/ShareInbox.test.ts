import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { flushPromises } from '@vue/test-utils';
import { reactive } from 'vue';
import { useShareStore, type CurrentShare } from '../../../core/stores/shareStore';
import { mockInvoke, invokeMock } from '../../mocks/tauri';

const context = vi.hoisted(() => ({ session: {} as { currentTopicId: string | null; currentSelectedItem: { id: string } | null }, notices: vi.fn() }));
vi.mock('../../../core/stores/chatSessionStore', () => ({ useChatSessionStore: () => context.session }));
vi.mock('../../../core/stores/assistant', () => ({ useAssistantStore: () => ({ agents: [{ id: 'agent' }], fetchAgentsAndGroups: async () => {} }) }));
vi.mock('../../../core/stores/topicListManager', () => ({ useTopicStore: () => ({ loadTopicList: async () => {} }) }));
vi.mock('../../../core/stores/notification', () => ({ useNotificationStore: () => ({ addNotification: context.notices }) }));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
const share = (generation: number, state: CurrentShare['state']): CurrentShare => ({
  generation, deliveryId: `share-${generation}`, state, text: `text-${generation}`,
  items: [], attachments: [], revision: 0,
});
beforeEach(() => {
  setActivePinia(createPinia());
  context.session = reactive({ currentTopicId: 'topic', currentSelectedItem: { id: 'agent' } });
  context.notices.mockClear();
});

describe('one current share', () => {
  it('receives B while A imports and ignores the late A result', async () => {
    const pending = deferred<CurrentShare>();
    const store = useShareStore();
    let reads = 0;
    mockInvoke('read_current_share', () => ++reads === 1 ? share(1, 'ready') : share(2, 'receiving'));
    mockInvoke('prepare_current_share', () => pending.promise);
    await store.refresh();
    await store.refresh();
    expect(store.current?.deliveryId).toBe('share-2');
    pending.resolve(share(1, 'prepared'));
    await flushPromises();
    expect(store.current?.state).toBe('receiving');
    expect(store.selectorOpen).toBe(false);
    expect(context.notices).not.toHaveBeenCalled();
  });

  it('coalesces a notification during a read and still observes its newer slot', async () => {
    const pending = deferred<CurrentShare>();
    const store = useShareStore();
    let reads = 0;
    mockInvoke('read_current_share', () => ++reads === 1 ? pending.promise : share(2, 'failed'));
    const first = store.refresh();
    void store.refresh();
    pending.resolve(share(1, 'receiving'));
    await first; await flushPromises();
    expect(reads).toBe(2);
    expect(store.current?.deliveryId).toBe('share-2');
    expect(store.selectorOpen).toBe(true);
  });

  it('recovers the last acknowledged draft and serializes later edits by revision', async () => {
    const restored = { ...share(3, 'draft'), agentId: 'agent', topicId: 'topic', text: 'recovered edit', revision: 8 };
    mockInvoke('read_current_share', () => restored);
    mockInvoke('prepare_current_share', () => restored);
    const pending = deferred<CurrentShare>();
    let writes = 0;
    mockInvoke('update_share_draft', args => ++writes === 1 ? pending.promise : ({ ...restored, text: args?.text, revision: 10 }));
    const store = useShareStore();
    await store.refresh(); await flushPromises();
    expect(store.activeDraft?.text).toBe('recovered edit');
    store.editDraft('first edit', []);
    const first = store.flushDraft();
    store.editDraft('second edit', []);
    pending.resolve({ ...restored, text: 'first edit', revision: 9 });
    await first; await store.flushDraft();
    const calls = invokeMock.mock.calls.filter(([name]) => name === 'update_share_draft').map(([, args]) => args);
    expect(calls.map(args => [args?.revision, args?.text])).toEqual([[8, 'first edit'], [9, 'second edit']]);
    expect(store.activeDraft?.text).toBe('second edit');
  });

  it('does not let an older preparation reply roll back a confirmed revision', async () => {
    const original = { ...share(4, 'draft'), agentId: 'agent', topicId: 'topic', revision: 8 };
    const prepare = deferred<CurrentShare>();
    mockInvoke('read_current_share', () => original);
    mockInvoke('prepare_current_share', () => prepare.promise);
    mockInvoke('update_share_draft', () => ({ ...original, text: 'new edit', revision: 9 }));
    const store = useShareStore();
    await store.refresh();
    store.editDraft('new edit', []); await store.flushDraft();
    prepare.resolve(original); await flushPromises();
    expect(store.current?.revision).toBe(9);
    expect(store.current?.text).toBe('new edit');
  });

  it('releases the send button as soon as the message commits', async () => {
    const original = { ...share(5, 'draft'), agentId: 'agent', topicId: 'topic', revision: 1 };
    mockInvoke('read_current_share', () => original);
    mockInvoke('prepare_current_share', () => original);
    mockInvoke('commit_share_message', () => ({ message: { id: 'msg_share-5' }, persisted: { blocks: [], topicUpdatedAt: 1 }, alreadySent: false }));
    const store = useShareStore(); await store.refresh(); await flushPromises();
    store.sending = true;
    await store.commit({ generation: 5, deliveryId: 'share-5' }, { id: 'pending', role: 'user', content: 'send', timestamp: 1 });
    expect(store.sending).toBe(false);
    expect(store.current?.state).toBe('sent');
  });
});
