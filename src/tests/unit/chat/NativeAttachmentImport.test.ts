import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useAttachmentStore } from '@/core/stores/attachmentStore';
import { useChatSessionStore } from '@/core/stores/chatSessionStore';
import { useNotificationStore } from '@/core/stores/notification';
import { invokeMock, mockInvoke } from '@/tests/mocks/tauri';
import { flushPromises } from '@/tests/utils/flush';

vi.mock('@/core/utils/platform', () => ({ isAndroid: false, isIOS: true }));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

const picked = { path: '/cache/picked-image.jpg', name: 'image.jpg', mime: 'image/jpeg', size: 10, hash: 'hash' };
const batch = { files: [picked], errors: [] as { name: string; message: string }[] };
const file = (name: string) => ({ ...picked, name, path: `/cache/${name}`, thumbnailPath: `/cache/${name}.thumb` });
const registered = (name: string) => ({ name, internalPath: `/cas/${name}`, type: 'image/jpeg', size: 10 });

describe('native attachment ownership', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mockInvoke('plugin:vcp-mobile|prepare_file_operation', () => ({ reservationToken: 'test-reservation' }));
  });

  it('cancels the matching native copy and ignores a late picker result after removal', async () => {
    const selection = deferred<typeof batch>();
    let operationId = '';
    mockInvoke('plugin:vcp-mobile|pick_file', (args) => {
      operationId = String(args?.operationId);
      return selection.promise;
    });
    const store = useAttachmentStore();
    const importing = store.handleAttachment('gallery');
    await flushPromises();
    const start = (id: string) => window.dispatchEvent(new CustomEvent('vcp-mobile-file-start', {
      detail: { operationId: id, name: 'image.jpg', mime: 'image/jpeg', size: 10 },
    }));
    start('another-picker');
    expect(store.stagedAttachments).toHaveLength(1);
    expect(store.stagedAttachments[0].name).toBe('正在选择文件');
    start(operationId);
    expect(store.stagedAttachments).toHaveLength(1);
    store.removeStaged(0);
    selection.resolve(batch);
    await importing;
    expect(store.stagedAttachments).toHaveLength(0);
    expect(invokeMock).toHaveBeenCalledWith('cancel_attachment_import', { operationId });
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|cancel_file_operation', { operationId });
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|delete_temp_file', { filePath: picked.path });
    expect(invokeMock.mock.calls.some(([command]) => command === 'register_local_file')).toBe(false);
    store.$dispose();
  });

  it('invalidates acquisition when the user moves to a different conversation', async () => {
    const selection = deferred<typeof batch>();
    mockInvoke('plugin:vcp-mobile|pick_file', () => selection.promise);
    const session = useChatSessionStore();
    session.setConversation({ id: 'agent-a', name: 'Agent', type: 'agent' }, 'topic-a');
    const store = useAttachmentStore();
    const importing = store.handleAttachment();
    await flushPromises();
    session.setConversation({ id: 'agent-a', name: 'Agent', type: 'agent' }, 'topic-b');
    selection.resolve(batch);
    await importing;
    expect(store.stagedAttachments).toEqual([]);
    expect(invokeMock.mock.calls.some(([command]) => command === 'register_local_file')).toBe(false);
    store.$dispose();
  });

  it.each(['file', 'gallery'] as const)('imports %s selections sequentially and keeps successes across provider and registration failures', async (mode) => {
    const first = deferred<ReturnType<typeof registered>>();
    const files = ['first.jpg', 'bad.jpg', 'last.jpg'].map(file);
    const registrations: string[] = [];
    mockInvoke('plugin:vcp-mobile|pick_file', () => ({ files, errors: [{ name: 'cloud.jpg', message: 'provider offline' }] }));
    mockInvoke('register_local_file', (args) => {
      const name = String(args?.originalName);
      registrations.push(name);
      if (name === 'first.jpg') return first.promise;
      if (name === 'bad.jpg') return Promise.reject('cannot read image');
      return registered(name);
    });
    const store = useAttachmentStore();
    const importing = store.handleAttachment(mode);
    await vi.waitFor(() => expect(registrations).toEqual(['first.jpg']));
    expect(store.stagedAttachments.map(a => a.name)).toEqual(files.map(f => f.name));
    first.resolve(registered('first.jpg'));
    await importing;
    expect(registrations).toEqual(files.map(f => f.name));
    expect(store.stagedAttachments.map(a => [a.name, a.status])).toEqual([['first.jpg', 'done'], ['last.jpg', 'done']]);
    for (const value of files) {
      expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|delete_temp_file', { filePath: value.path });
      expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|delete_temp_file', { filePath: value.thumbnailPath });
    }
    const notice = useNotificationStore().historyList[0];
    expect(notice.title).toBe('部分附件未能导入');
    expect(notice.message).toContain('已导入 2 项');
    expect(notice.message).toContain('cloud.jpg: provider offline');
    expect(notice.message).toContain('bad.jpg: cannot read image');
    store.$dispose();
  });

  it('removes a queued item without registering it or cancelling its siblings', async () => {
    const first = deferred<ReturnType<typeof registered>>();
    const files = ['first.jpg', 'skip.jpg', 'last.jpg'].map(file);
    mockInvoke('plugin:vcp-mobile|pick_file', () => ({ files, errors: [] }));
    mockInvoke('register_local_file', (args) => args?.originalName === 'first.jpg' ? first.promise : registered(String(args?.originalName)));
    const store = useAttachmentStore();
    const importing = store.handleAttachment();
    await vi.waitFor(() => expect(invokeMock.mock.calls.some(([command]) => command === 'register_local_file')).toBe(true));
    store.removeStaged(1);
    first.resolve(registered('first.jpg'));
    await importing;
    expect(store.stagedAttachments.map(a => a.name)).toEqual(['first.jpg', 'last.jpg']);
    expect(invokeMock.mock.calls.filter(([command]) => command === 'register_local_file').map(([, args]) => args?.originalName)).toEqual(['first.jpg', 'last.jpg']);
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|delete_temp_file', { filePath: files[1].path });
    expect(useNotificationStore().activeToasts).toEqual([]);
    store.$dispose();
  });

  it('ignores a removed in-flight item and continues importing the next item', async () => {
    const first = deferred<ReturnType<typeof registered>>();
    mockInvoke('plugin:vcp-mobile|pick_file', () => ({ files: [file('removed.jpg'), file('kept.jpg')], errors: [] }));
    mockInvoke('register_local_file', (args) => args?.originalName === 'removed.jpg' ? first.promise : registered('kept.jpg'));
    const store = useAttachmentStore();
    const importing = store.handleAttachment();
    await vi.waitFor(() => expect(invokeMock.mock.calls.some(([command]) => command === 'register_local_file')).toBe(true));
    store.removeStaged(0);
    first.resolve(registered('removed.jpg'));
    await importing;
    expect(store.stagedAttachments.map(a => a.name)).toEqual(['kept.jpg']);
    expect(invokeMock).not.toHaveBeenCalledWith('plugin:vcp-mobile|delete_temp_file', { filePath: '/cas/removed.jpg' });
    store.$dispose();
  });

  it.each(['clear', 'conversation', 'dispose'] as const)('stops the entire queued batch on %s and cleans late native files', async (action) => {
    const first = deferred<ReturnType<typeof registered>>();
    const files = ['first.jpg', 'second.jpg', 'third.jpg'].map(file);
    mockInvoke('plugin:vcp-mobile|pick_file', () => ({ files, errors: [] }));
    mockInvoke('register_local_file', () => first.promise);
    const store = useAttachmentStore();
    const importing = store.handleAttachment();
    await vi.waitFor(() => expect(invokeMock.mock.calls.some(([command]) => command === 'register_local_file')).toBe(true));
    if (action === 'clear') store.clearStaged();
    else if (action === 'dispose') store.$dispose();
    else useChatSessionStore().setConversation({ id: 'agent', name: 'Agent', type: 'agent' }, 'another-topic');
    first.resolve(registered('first.jpg'));
    await importing;
    expect(store.stagedAttachments).toEqual([]);
    expect(invokeMock.mock.calls.filter(([command]) => command === 'register_local_file')).toHaveLength(1);
    expect(invokeMock).not.toHaveBeenCalledWith('plugin:vcp-mobile|delete_temp_file', { filePath: '/cas/first.jpg' });
    for (const value of files) {
      expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|delete_temp_file', { filePath: value.path });
      expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|delete_temp_file', { filePath: value.thumbnailPath });
    }
    expect(useNotificationStore().activeToasts).toEqual([]);
    store.$dispose();
  });

  it('silently settles system cancellation and permits another picker', async () => {
    let calls = 0;
    mockInvoke('plugin:vcp-mobile|pick_file', () => ++calls === 1 ? Promise.reject('cancelled: file picker') : batch);
    mockInvoke('register_local_file', () => registered(picked.name));
    const store = useAttachmentStore();
    await store.handleAttachment();
    expect(store.stagedAttachments).toEqual([]);
    expect(useNotificationStore().activeToasts).toEqual([]);
    await store.handleAttachment();
    expect(store.stagedAttachments[0].status).toBe('done');
    store.$dispose();
  });

  it('reports an entirely failed selection and cleans unsupported files before continuing', async () => {
    mockInvoke('plugin:vcp-mobile|pick_file', () => ({ files: [file('unsupported.bin')], errors: [{ name: 'cloud.jpg', message: 'offline' }] }));
    mockInvoke('check_attachment_support', () => Promise.reject('unsupported format'));
    const store = useAttachmentStore();
    await store.handleAttachment();
    expect(store.stagedAttachments).toEqual([]);
    expect(useNotificationStore().historyList[0].title).toBe('附件导入失败');
    expect(invokeMock.mock.calls.some(([command]) => command === 'register_local_file')).toBe(false);
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|delete_temp_file', { filePath: '/cache/unsupported.bin' });
    store.$dispose();
  });

  it('resets native progress for each selection item and ignores late native events after handoff', async () => {
    const selection = deferred<typeof batch>();
    const registration = deferred<ReturnType<typeof registered>>();
    mockInvoke('plugin:vcp-mobile|pick_file', () => selection.promise);
    mockInvoke('register_local_file', () => registration.promise);
    const store = useAttachmentStore();
    const importing = store.handleAttachment('gallery');
    await flushPromises();
    const operationId = store.stagedAttachments[0].id;
    const detail = { operationId, name: 'image.jpg', mime: 'image/jpeg', selectionCount: 2 };
    window.dispatchEvent(new CustomEvent('vcp-mobile-file-progress', { detail: { ...detail, selectionIndex: 0, loaded: 10, total: 10, progress: 100 } }));
    window.dispatchEvent(new CustomEvent('vcp-mobile-file-start', { detail: { ...detail, selectionIndex: 1, size: 0 } }));
    expect(store.stagedAttachments[0]).toMatchObject({ name: '（2/2）image.jpg', size: 0, progress: undefined, transferredBytes: undefined });
    selection.resolve(batch);
    await vi.waitFor(() => expect(invokeMock.mock.calls.some(([command]) => command === 'register_local_file')).toBe(true));
    window.dispatchEvent(new CustomEvent('vcp-mobile-file-start', { detail: { ...detail, selectionIndex: 0, size: 10 } }));
    expect(store.stagedAttachments).toHaveLength(1);
    expect(store.stagedAttachments[0].id).not.toBe(operationId);
    registration.resolve(registered(picked.name));
    await importing;
    store.$dispose();
  });

});
