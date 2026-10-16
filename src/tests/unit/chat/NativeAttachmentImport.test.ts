import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useAttachmentStore } from '@/core/stores/attachmentStore';
import { useChatSessionStore } from '@/core/stores/chatSessionStore';
import { invokeMock, mockInvoke } from '@/tests/mocks/tauri';
import { flushPromises } from '@/tests/utils/flush';

vi.mock('@/core/utils/platform', () => ({ isAndroid: false, isIOS: true }));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

const picked = { path: '/cache/picked-image.jpg', name: 'image.jpg', mime: 'image/jpeg', size: 10, hash: 'hash' };

describe('native attachment ownership', () => {
  beforeEach(() => setActivePinia(createPinia()));

  it('cancels the matching native copy and ignores a late picker result after removal', async () => {
    const selection = deferred<typeof picked>();
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
    selection.resolve(picked);
    await importing;
    expect(store.stagedAttachments).toHaveLength(0);
    expect(invokeMock).toHaveBeenCalledWith('cancel_attachment_import', { operationId });
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|cancel_file_operation', { operationId });
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|delete_temp_file', { filePath: picked.path });
    expect(invokeMock.mock.calls.some(([command]) => command === 'register_local_file')).toBe(false);
    store.$dispose();
  });

  it('does not reinsert a cleared card when CAS registration has already committed', async () => {
    const registration = deferred<Record<string, unknown>>();
    mockInvoke('plugin:vcp-mobile|pick_file', () => picked);
    mockInvoke('register_local_file', () => registration.promise);
    const store = useAttachmentStore();
    const importing = store.handleAttachment();
    await flushPromises();
    expect(store.stagedAttachments).toHaveLength(1);
    store.clearStaged();
    registration.resolve({ name: 'image.jpg', internalPath: '/cas/hash.jpg', hash: 'hash', type: 'image/jpeg', size: 10 });
    await importing;
    expect(store.stagedAttachments).toHaveLength(0);
    expect(invokeMock).not.toHaveBeenCalledWith('plugin:vcp-mobile|delete_temp_file', { filePath: '/cas/hash.jpg' });
    store.$dispose();
  });

  it('invalidates acquisition when the user moves to a different conversation', async () => {
    const selection = deferred<typeof picked>();
    mockInvoke('plugin:vcp-mobile|pick_file', () => selection.promise);
    const session = useChatSessionStore();
    session.setConversation({ id: 'agent-a', name: 'Agent', type: 'agent' }, 'topic-a');
    const store = useAttachmentStore();
    const importing = store.handleAttachment();
    await flushPromises();
    session.setConversation({ id: 'agent-a', name: 'Agent', type: 'agent' }, 'topic-b');
    selection.resolve(picked);
    await importing;
    expect(store.stagedAttachments).toEqual([]);
    expect(invokeMock.mock.calls.some(([command]) => command === 'register_local_file')).toBe(false);
    store.$dispose();
  });
});
