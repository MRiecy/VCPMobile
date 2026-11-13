import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { flushPromises } from '@vue/test-utils';
import { reactive } from 'vue';
import { useShareStore, type CurrentShare, type ShareReadResult } from '../../../core/stores/shareStore';
import { emitTauriEvent, mockInvoke, invokeMock, listenMock } from '../../mocks/tauri';

const context = vi.hoisted(() => ({ session: {} as {
  currentTopicId: string | null;
  currentSelectedItem: { id: string; type: string } | null;
  beginNavigation: () => number;
  isNavigationCurrent: (intent: number) => boolean;
  notifyNavigationResult: (result: string) => void;
  selectTopicById: (agentId: string, ownerType: string, topicId: string, intent: number) => Promise<string>;
}, notices: vi.fn(), android: false }));
vi.mock('../../../core/stores/chatSessionStore', () => ({ useChatSessionStore: () => context.session }));
vi.mock('../../../core/stores/assistant', () => ({ useAssistantStore: () => ({ agents: [{ id: 'agent' }], fetchAgentsAndGroups: async () => {} }) }));
vi.mock('../../../core/stores/topicListManager', () => ({ useTopicStore: () => ({ loadTopicList: async () => {} }) }));
vi.mock('../../../core/stores/notification', () => ({ useNotificationStore: () => ({ addNotification: context.notices }) }));
vi.mock('../../../core/utils/platform', () => ({ get isAndroid() { return context.android; } }));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
const share = (generation: number, state: CurrentShare['state']): CurrentShare => ({
  generation, deliveryId: `share-${generation}`, state, text: `text-${generation}`,
  items: [], attachments: [], revision: 0,
});
const available = (current: CurrentShare | null, needsPreparation = current?.state === 'ready'): ShareReadResult => ({
  status: 'available', current, needsPreparation,
});
let pinia: Pinia;
beforeEach(() => {
  pinia = createPinia();
  setActivePinia(pinia);
  context.session = reactive({
    currentTopicId: 'topic', currentSelectedItem: { id: 'agent', type: 'agent' },
    beginNavigation: () => 1, isNavigationCurrent: intent => intent === 1, notifyNavigationResult: vi.fn(),
    selectTopicById: vi.fn(async () => 'accepted'),
  });
  context.notices.mockClear();
  context.android = false;
});
afterEach(() => disposePinia(pinia));

describe('one current share', () => {
  it('preserves a draft and its pending edit when the share container is unavailable', async () => {
    const original = { ...share(1, 'draft'), agentId: 'agent', topicId: 'topic', revision: 2 };
    let result: ShareReadResult = available(original);
    mockInvoke('read_current_share', () => result);
    mockInvoke('update_share_draft', args => ({ ...original, text: args?.text, revision: 3 }));
    const store = useShareStore();
    await store.refresh();
    store.closeSelector();
    store.editDraft('pending local edit', []);

    result = { status: 'unavailable' };
    await store.refresh();
    await store.readCurrent();
    expect(store.activeDraft).toEqual(original);
    expect(store.selectorOpen).toBe(false);
    expect(invokeMock.mock.calls.some(([name]) => name === 'prepare_current_share')).toBe(false);
    expect(context.notices).not.toHaveBeenCalled();

    await store.flushDraft();
    expect(invokeMock).toHaveBeenCalledWith('update_share_draft', {
      generation: 1, deliveryId: 'share-1', revision: 2, text: 'pending local edit', attachments: [],
    });
    expect(store.activeDraft?.text).toBe('pending local edit');
  });

  it('restores a healthy draft without repeating attachment preparation on foreground refresh', async () => {
    const original = { ...share(2, 'draft'), agentId: 'agent', topicId: 'topic', revision: 4 };
    mockInvoke('read_current_share', () => available(original));
    const store = useShareStore();
    await store.refresh();
    await store.refresh();

    expect(store.activeDraft).toEqual(original);
    expect(store.busy).toBe(false);
    expect(invokeMock.mock.calls.some(([name]) => name === 'prepare_current_share')).toBe(false);
  });

  it('reads an empty Android inbox once after a foreground native intake', async () => {
    context.android = true;
    mockInvoke('receive_android_share', () => undefined);
    mockInvoke('read_current_share', () => available(null));
    const store = useShareStore();
    await store.refresh();

    expect(invokeMock.mock.calls).toEqual([['receive_android_share'], ['read_current_share']]);
    expect(store.current).toBeNull();
    expect(store.selectorOpen).toBe(false);
    expect(context.notices).not.toHaveBeenCalled();
  });

  it('preserves a dismissed selector across native change reads until a fresh foreground entry', async () => {
    const prepared = share(3, 'prepared');
    mockInvoke('read_current_share', () => available(prepared));
    const store = useShareStore();
    await store.refresh();
    store.closeSelector();
    await store.readCurrent();
    expect(store.selectorOpen).toBe(false);

    await store.refresh();
    expect(store.selectorOpen).toBe(true);
  });

  it('reports a real read failure instead of treating it as an unavailable container', async () => {
    mockInvoke('read_current_share', async () => { throw new Error('manifest read failed'); });
    const store = useShareStore();
    await store.refresh().catch(store.notify);

    expect(context.notices).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({
      type: 'error', title: '分享处理失败', message: 'manifest read failed',
    }));
  });

  it('keeps a dismissed selector closed when preparation finishes and resumes on a fresh entry', async () => {
    const prepared = { ...share(1, 'prepared'), text: 'retained shared text', revision: 1 };
    const pending = deferred<CurrentShare>();
    let reads = 0;
    mockInvoke('read_current_share', () => available(++reads === 1 ? share(1, 'ready') : prepared));
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
    mockInvoke('read_current_share', async () => available(++reads === 2 ? await pending.promise : prepared));
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
    let current = share(3, 'prepared');
    mockInvoke('read_current_share', () => available(current));
    mockInvoke('receive_android_share', () => {
      current = share(4, 'ready');
      emitTauriEvent('vcp-share-inbox-changed', null);
    });
    mockInvoke('prepare_current_share', () => share(4, 'prepared'));
    const store = useShareStore();
    await store.refresh();
    store.closeSelector();
    await store.pollAndroid();
    await flushPromises();

    expect(store.current?.deliveryId).toBe('share-4');
    expect(store.current?.state).toBe('prepared');
    expect(store.selectorOpen).toBe(true);
  });

  it('observes admission and file publication before a cold-start handoff returns', async () => {
    const handoff = deferred<void>();
    let current = share(6, 'receiving');
    mockInvoke('read_current_share', () => available(current));
    mockInvoke('receive_android_share', () => {
      emitTauriEvent('vcp-share-inbox-changed', null);
      return handoff.promise;
    });
    mockInvoke('prepare_current_share', () => share(6, 'prepared'));
    const store = useShareStore();
    const polling = store.pollAndroid();
    await flushPromises();
    expect(store.current?.state).toBe('receiving');
    expect(listenMock).toHaveBeenCalledExactlyOnceWith('vcp-share-inbox-changed', expect.any(Function));
    expect(invokeMock).toHaveBeenCalledWith('receive_android_share');
    expect(invokeMock.mock.calls.find(([name]) => name === 'receive_android_share')).toHaveLength(1);

    current = share(6, 'ready');
    emitTauriEvent('vcp-share-inbox-changed', null);
    await flushPromises();
    expect(store.current?.state).toBe('prepared');
    expect(store.selectorOpen).toBe(true);
    store.closeSelector();
    current = share(6, 'prepared');
    emitTauriEvent('vcp-share-inbox-changed', null);
    await flushPromises();
    expect(store.selectorOpen).toBe(false);
    handoff.resolve();
    await polling;
    expect(store.current?.state).toBe('prepared');
    expect(store.selectorOpen).toBe(false);
    expect(invokeMock.mock.calls.filter(([name]) => name === 'receive_android_share')).toHaveLength(1);
    expect(invokeMock.mock.calls.some(([name]) => ['read_android_share', 'accept_android_share', 'finish_android_share'].includes(name))).toBe(false);
  });

  it('accepts B while A is handing off files and reads B again after A returns', async () => {
    const oldHandoff = deferred<void>();
    let current = share(1, 'receiving');
    let receives = 0;
    mockInvoke('receive_android_share', () => {
      current = ++receives === 1 ? share(1, 'receiving') : share(2, 'ready');
      emitTauriEvent('vcp-share-inbox-changed', null);
      return receives === 1 ? oldHandoff.promise : undefined;
    });
    mockInvoke('read_current_share', () => available(current));
    mockInvoke('prepare_current_share', args => {
      current = share(Number(args?.generation), 'prepared');
      return current;
    });
    const store = useShareStore();
    const oldPoll = store.pollAndroid();
    await flushPromises();
    expect(store.current?.deliveryId).toBe('share-1');

    await store.pollAndroid();
    await flushPromises();
    expect(store.current).toEqual(share(2, 'prepared'));
    expect(store.selectorOpen).toBe(true);
    oldHandoff.resolve();
    await oldPoll;

    expect(store.current).toEqual(share(2, 'prepared'));
    expect(context.notices).not.toHaveBeenCalled();
    expect(listenMock).toHaveBeenCalledTimes(1);
    expect(invokeMock.mock.calls.filter(([name]) => name === 'receive_android_share')).toEqual([
      ['receive_android_share'], ['receive_android_share'],
    ]);
  });

  it('releases an installed listener and ignores notifications after store disposal', async () => {
    const install = listenMock.getMockImplementation()!;
    const stop = vi.fn();
    listenMock.mockImplementationOnce(async (event, handler) => {
      stop.mockImplementation(await install(event, handler));
      return stop;
    });
    mockInvoke('receive_android_share', () => undefined);
    mockInvoke('read_current_share', () => available(share(11, 'prepared')));
    const store = useShareStore();
    await store.pollAndroid();
    const reads = invokeMock.mock.calls.filter(([name]) => name === 'read_current_share').length;
    store.$dispose();
    expect(stop).toHaveBeenCalledTimes(1);
    emitTauriEvent('vcp-share-inbox-changed', null);
    await flushPromises();
    expect(invokeMock.mock.calls.filter(([name]) => name === 'read_current_share')).toHaveLength(reads);
  });

  it('releases a late listener installation without starting a native intake', async () => {
    const installation = deferred<() => void>();
    const stop = vi.fn();
    listenMock.mockImplementationOnce(() => installation.promise);
    const store = useShareStore();
    const polling = store.pollAndroid();
    expect(invokeMock).not.toHaveBeenCalled();
    store.$dispose();
    installation.resolve(stop);
    await polling;
    expect(stop).toHaveBeenCalledTimes(1);
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('does not prepare a read that completes after store disposal', async () => {
    const pending = deferred<CurrentShare>();
    mockInvoke('receive_android_share', () => undefined);
    mockInvoke('read_current_share', async () => available(await pending.promise));
    const store = useShareStore();
    const polling = store.pollAndroid();
    await flushPromises();
    store.$dispose();
    pending.resolve(share(12, 'ready'));
    await polling;
    expect(store.current).toBeNull();
    expect(invokeMock.mock.calls.some(([name]) => name === 'prepare_current_share')).toBe(false);
  });

  it('receives B while A imports and ignores the late A result', async () => {
    const pending = deferred<CurrentShare>();
    const store = useShareStore();
    let reads = 0;
    mockInvoke('read_current_share', () => available(++reads === 1 ? share(1, 'ready') : share(2, 'receiving')));
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
    mockInvoke('read_current_share', async () => available(++reads === 1 ? await pending.promise : share(2, 'failed')));
    const first = store.refresh();
    void store.refresh();
    pending.resolve(share(1, 'receiving'));
    await first; await flushPromises();
    expect(reads).toBe(2);
    expect(store.current?.deliveryId).toBe('share-2');
    expect(store.selectorOpen).toBe(true);
  });

  it('keeps a prepared share when an earlier empty read arrives later', async () => {
    const pending = deferred<CurrentShare | null>();
    const preparing = deferred<CurrentShare>();
    const prepared = share(10, 'prepared');
    let reads = 0;
    mockInvoke('read_current_share', async () => available(++reads === 1 ? share(10, 'ready') : await pending.promise));
    mockInvoke('prepare_current_share', () => preparing.promise);
    const store = useShareStore();
    await store.refresh();
    const reading = store.refresh();

    preparing.resolve(prepared);
    await flushPromises();
    expect(store.current).toEqual(prepared);
    pending.resolve(null);
    await reading;

    expect(store.current).toEqual(prepared);
    expect(store.selectorOpen).toBe(true);
  });

  it.each([false, true])('preserves a single share through preparation, binding and recovery (attachment=%s)', async withAttachment => {
    const attachments: CurrentShare['attachments'] = withAttachment
      ? [{ type: 'image/png', name: 'shared.png', src: 'cas/shared.png', size: 128, hash: 'image-hash', status: 'done' }]
      : [];
    let current: CurrentShare = { ...share(13, 'ready'), text: 'shared text' };
    mockInvoke('receive_android_share', () => { emitTauriEvent('vcp-share-inbox-changed', null); });
    mockInvoke('read_current_share', () => available(current));
    mockInvoke('prepare_current_share', () => {
      if (current.state === 'ready') current = { ...current, state: 'prepared', attachments };
      return current;
    });
    mockInvoke('bind_current_share', () => {
      current = { ...current, state: 'draft', agentId: 'agent', topicId: 'topic', revision: 1 };
      return current;
    });
    const store = useShareStore();
    await store.pollAndroid();
    await flushPromises();
    expect(store.current?.text).toBe('shared text');
    expect(store.current?.attachments).toEqual(attachments);
    await store.openDraft('agent');
    expect(store.activeDraft?.text).toBe('shared text');
    expect(context.session.selectTopicById).toHaveBeenCalledExactlyOnceWith('agent', 'agent', 'topic', 1);

    store.$dispose();
    const recovered = useShareStore();
    await recovered.pollAndroid();
    await flushPromises();
    expect(recovered.activeDraft).toEqual(current);
    expect(recovered.activeDraft?.attachments).toEqual(attachments);
    expect(invokeMock.mock.calls.filter(([name]) => name === 'bind_current_share')).toHaveLength(1);
    expect(context.notices).not.toHaveBeenCalled();
  });

  it('recovers the last acknowledged draft and serializes later edits by revision', async () => {
    const restored = { ...share(3, 'draft'), agentId: 'agent', topicId: 'topic', text: 'recovered edit', revision: 8 };
    mockInvoke('read_current_share', () => available(restored));
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
    mockInvoke('read_current_share', () => available(original, true));
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
    mockInvoke('read_current_share', () => available(original, true));
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
    mockInvoke('read_current_share', () => available(next));
    await store.refresh();
    expect(store.current).toEqual(next);
    expect(store.selectorOpen).toBe(true);
  });

  it('ignores a read captured before discard without reopening or preparing its old contents', async () => {
    const original = share(7, 'prepared');
    const pending = deferred<CurrentShare>();
    let reads = 0;
    mockInvoke('read_current_share', async () => available(++reads === 1 ? original : await pending.promise));
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
    mockInvoke('read_current_share', () => available(original));
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

  it('does not revive a sent share when a repeated native wake reads an older draft', async () => {
    const original = { ...share(9, 'draft'), agentId: 'agent', topicId: 'topic', revision: 1 };
    mockInvoke('read_current_share', () => available(original, true));
    mockInvoke('prepare_current_share', () => original);
    mockInvoke('commit_share_message', () => ({ message: { id: 'msg_share-9' }, persisted: { blocks: [], topicUpdatedAt: 1 }, alreadySent: false }));
    const finished = deferred<void>();
    mockInvoke('receive_android_share', () => {
      emitTauriEvent('vcp-share-inbox-changed', null);
      return finished.promise;
    });
    const store = useShareStore();
    await store.refresh();
    await flushPromises();
    await store.commit(original, { id: 'pending', role: 'user', content: 'send', timestamp: 1 });

    const polling = store.pollAndroid();
    await flushPromises();
    expect(store.current?.state).toBe('sent');
    expect(invokeMock.mock.calls.filter(([name]) => name === 'receive_android_share')).toHaveLength(1);
    finished.resolve();
    await polling;

    expect(store.current?.state).toBe('sent');
    expect(store.selectorOpen).toBe(false);
    expect(invokeMock.mock.calls.filter(([name]) => name === 'prepare_current_share')).toHaveLength(1);
  });

  it('releases the send button as soon as the message commits', async () => {
    const original = { ...share(5, 'draft'), agentId: 'agent', topicId: 'topic', revision: 1 };
    mockInvoke('read_current_share', () => available(original));
    mockInvoke('prepare_current_share', () => original);
    mockInvoke('commit_share_message', () => ({ message: { id: 'msg_share-5' }, persisted: { blocks: [], topicUpdatedAt: 1 }, alreadySent: false }));
    const store = useShareStore(); await store.refresh(); await flushPromises();
    store.sending = true;
    await store.commit({ generation: 5, deliveryId: 'share-5' }, { id: 'pending', role: 'user', content: 'send', timestamp: 1 });
    expect(store.sending).toBe(false);
    expect(store.current?.state).toBe('sent');
  });
});
