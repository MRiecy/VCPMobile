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

  it('keeps a new Android share when an earlier empty read arrives later', async () => {
    const pending = deferred<CurrentShare | null>();
    const prepared = share(10, 'prepared');
    mockInvoke('read_current_share', () => pending.promise);
    mockInvoke('accept_android_share', () => share(10, 'receiving'));
    mockInvoke('finish_android_share', () => share(10, 'ready'));
    mockInvoke('prepare_current_share', () => prepared);
    const store = useShareStore();
    const reading = store.refresh();

    await store.receiveAndroid({ intentId: prepared.deliveryId, payloadRevision: 2, state: 'ready', files: [] });
    await flushPromises();
    expect(store.current).toEqual(prepared);
    pending.resolve(null);
    await reading;

    expect(store.current).toEqual(prepared);
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

  it.each(['draft', 'sent'] as const)('keeps a committed share closed when preparation returns %s', async (state) => {
    const original = { ...share(5, 'draft'), agentId: 'agent', topicId: 'topic', revision: 8 };
    const pending = deferred<CurrentShare>();
    mockInvoke('read_current_share', () => original);
    mockInvoke('prepare_current_share', () => pending.promise);
    mockInvoke('commit_share_message', () => ({ message: { id: 'msg_share-5' }, persisted: { blocks: [], topicUpdatedAt: 1 }, alreadySent: false }));
    const store = useShareStore();
    await store.refresh();
    expect(store.busy).toBe(true);

    // Foreground recovery may prepare an already bound draft while its user sends it.
    await store.commit(original, { id: 'pending', role: 'user', content: 'send', timestamp: 1 });
    expect(store.current?.state).toBe('sent');
    expect(store.selectorOpen).toBe(false);
    pending.resolve({ ...original, state, text: state === 'sent' ? '' : original.text });
    await flushPromises();

    expect(store.current?.state).toBe('sent');
    expect(store.activeDraft).toBeNull();
    expect(store.selectorOpen).toBe(false);
    expect(store.busy).toBe(false);
    expect(context.notices).not.toHaveBeenCalled();

    // A confirmed terminal belongs only to that delivery; the next share still opens.
    const next = share(6, 'prepared');
    mockInvoke('read_current_share', () => next);
    await store.refresh();
    expect(store.current).toEqual(next);
    expect(store.selectorOpen).toBe(true);
  });

  it('ignores a read captured before discard without reopening or preparing its old contents', async () => {
    const original = share(7, 'prepared');
    const pending = deferred<CurrentShare>();
    let reads = 0;
    mockInvoke('read_current_share', () => ++reads === 1 ? original : pending.promise);
    mockInvoke('discard_current_share', () => undefined);
    const store = useShareStore();
    await store.refresh();
    const reading = store.refresh();

    await store.discard();
    expect(store.current?.state).toBe('discarded');
    pending.resolve(original);
    await reading;
    await flushPromises();

    expect(store.current?.state).toBe('discarded');
    expect(store.selectorOpen).toBe(false);
    expect(invokeMock.mock.calls.some(([name]) => name === 'prepare_current_share')).toBe(false);
    expect(context.notices).not.toHaveBeenCalled();
  });

  it('keeps a sent receipt when an overlapping discard acknowledgement arrives later', async () => {
    const original = { ...share(8, 'draft'), agentId: 'agent', topicId: 'topic', revision: 1 };
    const discarded = deferred<void>();
    mockInvoke('read_current_share', () => original);
    mockInvoke('prepare_current_share', () => original);
    mockInvoke('discard_current_share', () => discarded.promise);
    mockInvoke('commit_share_message', () => ({ message: { id: 'msg_share-8' }, persisted: { blocks: [], topicUpdatedAt: 1 }, alreadySent: false }));
    const store = useShareStore();
    await store.refresh();
    await flushPromises();

    // The message transaction wins first; Rust then treats discard as a no-op.
    const discarding = store.discard();
    await store.commit(original, { id: 'pending', role: 'user', content: 'send', timestamp: 1 });
    discarded.resolve();
    await discarding;

    expect(store.current?.state).toBe('sent');
    expect(store.selectorOpen).toBe(false);
  });

  it('still acknowledges a native ready replay after its stale UI snapshot is rejected', async () => {
    const original = { ...share(9, 'draft'), agentId: 'agent', topicId: 'topic', revision: 1 };
    mockInvoke('read_current_share', () => original);
    mockInvoke('prepare_current_share', () => original);
    mockInvoke('commit_share_message', () => ({ message: { id: 'msg_share-9' }, persisted: { blocks: [], topicUpdatedAt: 1 }, alreadySent: false }));
    mockInvoke('accept_android_share', () => original);
    const finished = deferred<CurrentShare>();
    mockInvoke('finish_android_share', () => finished.promise);
    const store = useShareStore();
    await store.refresh();
    await flushPromises();
    await store.commit(original, { id: 'pending', role: 'user', content: 'send', timestamp: 1 });

    await store.receiveAndroid({ intentId: original.deliveryId, payloadRevision: 2, state: 'ready', files: [] });
    expect(store.current?.state).toBe('sent');
    expect(invokeMock.mock.calls.filter(([name]) => name === 'finish_android_share')).toHaveLength(1);
    finished.resolve({ ...original, state: 'sent', text: '' });
    await flushPromises();

    expect(store.current?.state).toBe('sent');
    expect(store.selectorOpen).toBe(false);
    expect(invokeMock.mock.calls.filter(([name]) => name === 'prepare_current_share')).toHaveLength(1);
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
