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
  it('keeps a dismissed selector closed when preparation finishes and resumes on a fresh entry', async () => {
    const prepared = { ...share(1, 'prepared'), text: 'retained shared text', revision: 1 };
    const pending = deferred<CurrentShare>();
    let reads = 0;
    mockInvoke('read_current_share', () => ++reads === 1 ? share(1, 'ready') : prepared);
    mockInvoke('prepare_current_share', () => pending.promise);
    const store = useShareStore();

    await store.refresh();
    expect(store.selectorOpen).toBe(true);
    expect(store.busy).toBe(true);
    store.closeSelector();
    pending.resolve(prepared);
    await flushPromises();

    expect(store.selectorOpen).toBe(false);
    expect(store.busy).toBe(false);
    expect(store.current).toEqual(prepared);
    expect(invokeMock.mock.calls.some(([command]) => command === 'discard_current_share')).toBe(false);
    await store.refresh();
    expect(store.selectorOpen).toBe(true);
    expect(store.current).toEqual(prepared);
  });

  it('preserves a close across an in-flight read and its already queued follow-up', async () => {
    const prepared = share(2, 'prepared');
    const pending = deferred<CurrentShare>();
    let reads = 0;
    mockInvoke('read_current_share', () => ++reads === 2 ? pending.promise : prepared);
    const store = useShareStore();
    await store.refresh();
    const reading = store.refresh();
    void store.refresh();
    store.closeSelector();
    pending.resolve(prepared);
    await reading;
    await flushPromises();

    expect(reads).toBe(3);
    expect(store.current).toEqual(prepared);
    expect(store.selectorOpen).toBe(false);
    await store.refresh();
    expect(store.selectorOpen).toBe(true);
  });

  it('opens a new native share without reviving the dismissed previous share', async () => {
    mockInvoke('read_current_share', () => share(3, 'prepared'));
    mockInvoke('accept_android_share', () => share(4, 'receiving'));
    mockInvoke('finish_android_share', () => share(4, 'ready'));
    mockInvoke('prepare_current_share', () => share(4, 'prepared'));
    const store = useShareStore();
    await store.refresh();
    store.closeSelector();
    store.receiveAndroid({ intentId: 'share-4', payloadRevision: 2, state: 'ready', text: 'new native share', files: [] });
    await flushPromises();

    expect(store.current?.deliveryId).toBe('share-4');
    expect(store.current?.state).toBe('prepared');
    expect(store.selectorOpen).toBe(true);
  });

  it('pulls a missed cold-start snapshot and coalesces repeated ready delivery', async () => {
    const pending = deferred<CurrentShare>();
    const payload = { intentId: 'share-6', payloadRevision: 2, state: 'ready', text: 'cold start', files: [] };
    mockInvoke('read_android_share', () => payload);
    mockInvoke('accept_android_share', () => share(6, 'receiving'));
    mockInvoke('finish_android_share', () => pending.promise);
    mockInvoke('prepare_current_share', () => share(6, 'prepared'));
    const store = useShareStore();
    await store.pollAndroid();
    await store.pollAndroid();
    expect(invokeMock.mock.calls.filter(([name]) => name === 'finish_android_share')).toHaveLength(1);
    expect(invokeMock.mock.calls.find(([name]) => name === 'accept_android_share')?.[1]).toMatchObject({
      deliveryId: 'share-6', payloadRevision: 2, payloadState: 'ready',
    });
    pending.resolve(share(6, 'ready'));
    await flushPromises();
    expect(store.current?.state).toBe('prepared');
  });

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
