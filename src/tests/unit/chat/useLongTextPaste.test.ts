import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { effectScope, nextTick, reactive, ref, type EffectScope } from 'vue';
import { useLongTextPaste } from '@/features/chat/composables/useLongTextPaste';
import { useChatSessionStore, type ConversationKey } from '@/core/stores/chatSessionStore';
import { useAttachmentStore } from '@/core/stores/attachmentStore';
import { useNotificationStore } from '@/core/stores/notification';
import type { Attachment, AttachmentData } from '@/core/types/chat';
import { invokeMock, mockInvoke } from '@/tests/mocks/tauri';

vi.mock('@/core/stores/chatSessionStore', () => ({ useChatSessionStore: vi.fn() }));
vi.mock('@/core/stores/attachmentStore', () => ({ useAttachmentStore: vi.fn() }));
vi.mock('@/core/stores/notification', () => ({ useNotificationStore: vi.fn() }));

const MAX_BYTES = 2 * 1024 * 1024;
const longText = '长文本'.repeat(400);
const scopes: EffectScope[] = [];
const attachments = reactive({ stagedAttachments: [] as Attachment[] });
const notifications = { addNotification: vi.fn() };
const session = reactive({
  currentConversationKey: null as ConversationKey | null,
  isConversationCurrent: vi.fn((key: ConversationKey | null | undefined): boolean =>
    Boolean(key && key.epoch === session.currentConversationKey?.epoch)),
});

const savedAttachment = {
  type: 'text/plain',
  name: 'pasted.txt',
  internalPath: 'attachments/pasted.txt',
  size: 3600,
  hash: 'text-hash',
} as AttachmentData;

function setup(inputText = '') {
  const input = ref(inputText);
  const scope = effectScope();
  scopes.push(scope);
  const paste = scope.run(() => useLongTextPaste(input))!;
  return { input, scope, ...paste };
}

function pasteEvent(text: string): ClipboardEvent {
  const event = new Event('paste', { cancelable: true });
  Object.defineProperty(event, 'clipboardData', {
    value: { files: [], items: [], getData: () => text },
  });
  return event as ClipboardEvent;
}

function beforeInputEvent(text: string): InputEvent {
  return new InputEvent('beforeinput', {
    cancelable: true,
    inputType: 'insertFromPaste',
    data: text,
  });
}

function deferredSave() {
  let resolve!: (value: AttachmentData) => void;
  const promise = new Promise<AttachmentData>(done => { resolve = done; });
  mockInvoke('store_file', () => promise);
  return () => resolve(savedAttachment);
}

beforeEach(() => {
  attachments.stagedAttachments = [];
  session.currentConversationKey = {
    ownerId: 'agent-a', ownerType: 'agent', topicId: 'topic-a', epoch: 1,
  };
  vi.mocked(useChatSessionStore).mockReturnValue(session as unknown as ReturnType<typeof useChatSessionStore>);
  vi.mocked(useAttachmentStore).mockReturnValue(attachments as unknown as ReturnType<typeof useAttachmentStore>);
  vi.mocked(useNotificationStore).mockReturnValue(notifications as unknown as ReturnType<typeof useNotificationStore>);
  mockInvoke('store_file', () => savedAttachment);
});

afterEach(() => {
  for (const scope of scopes.splice(0)) scope.stop();
});

describe('long text paste resource and input ownership', () => {
  it.each([
    ['ASCII', 'a'.repeat(MAX_BYTES)],
    ['Chinese', '中'.repeat(Math.floor(MAX_BYTES / 3)) + 'ab'],
    ['surrogate pairs', '😀'.repeat(MAX_BYTES / 4)],
    ['unpaired surrogates', '\ud800'.repeat(Math.floor(MAX_BYTES / 3)) + 'ab'],
  ])('accepts exactly 2 MiB of UTF-8 for %s', async (_label, text) => {
    const { handlePaste } = setup();
    const event = pasteEvent(text);
    await handlePaste(event);

    expect(event.defaultPrevented).toBe(true);
    expect(invokeMock).toHaveBeenCalledTimes(1);
    const args = invokeMock.mock.calls[0][1];
    expect((args?.fileBytes as Uint8Array).byteLength).toBe(MAX_BYTES);
    expect(attachments.stagedAttachments[0].status).toBe('done');
  });

  it.each([
    ['ASCII', 'a'.repeat(MAX_BYTES + 1)],
    ['Chinese', '中'.repeat(Math.floor(MAX_BYTES / 3) + 1)],
    ['surrogate pairs', '😀'.repeat(MAX_BYTES / 4) + 'a'],
    ['unpaired surrogates', '\ud800'.repeat(Math.floor(MAX_BYTES / 3) + 1)],
  ])('rejects over-budget %s before encoding and leaves pasted input intact', async (_label, text) => {
    const encode = vi.spyOn(TextEncoder.prototype, 'encode');
    const { input, handlePaste, handleBeforeInput } = setup('existing ');
    const event = pasteEvent(text);
    await handlePaste(event);
    const beforeInput = beforeInputEvent(text);
    handleBeforeInput(beforeInput);
    // Simulate the browser insertion that must remain allowed after rejection.
    input.value += text;
    await nextTick();

    expect(event.defaultPrevented).toBe(false);
    expect(beforeInput.defaultPrevented).toBe(false);
    expect(input.value).toBe('existing ' + text);
    expect(encode).not.toHaveBeenCalled();
    expect(invokeMock).not.toHaveBeenCalled();
    expect(attachments.stagedAttachments).toEqual([]);
    expect(notifications.addNotification).toHaveBeenCalledWith(expect.objectContaining({
      type: 'warning', message: expect.stringContaining('2 MiB'),
    }));
  });

  it.each(['paste', 'beforeinput', 'watch'] as const)(
    'holds one operation across all input entry points when %s starts first',
    async (first) => {
      const finish = deferredSave();
      const paste = setup();
      if (first === 'paste') void paste.handlePaste(pasteEvent(longText));
      else if (first === 'beforeinput') paste.handleBeforeInput(beforeInputEvent(longText));
      else paste.input.value = longText;
      await nextTick();
      await nextTick();
      expect(paste.isProcessing.value).toBe(true);

      void paste.handlePaste(pasteEvent(longText));
      paste.handleBeforeInput(beforeInputEvent(longText));
      paste.input.value = 'keep this new text ' + longText;
      await nextTick();
      expect(invokeMock).toHaveBeenCalledTimes(1);
      expect(attachments.stagedAttachments).toHaveLength(1);
      expect(paste.input.value).toBe('keep this new text ' + longText);

      finish();
      await nextTick();
      await nextTick();
      expect(paste.isProcessing.value).toBe(false);
      expect(attachments.stagedAttachments[0].status).toBe('done');
    },
  );

  it('restores consumed text on a write failure without starting a second write', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    mockInvoke('store_file', () => Promise.reject(new Error('disk full')));
    const { input, handlePaste, isProcessing } = setup('existing ');
    await handlePaste(pasteEvent(longText));
    await nextTick();

    expect(input.value).toBe('existing ' + longText);
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(isProcessing.value).toBe(false);
    expect(attachments.stagedAttachments).toEqual([]);
    expect(notifications.addNotification).toHaveBeenCalledWith(expect.objectContaining({
      type: 'error', message: 'disk full',
    }));
  });

  it('lets the watcher own non-cancelable input instead of converting it twice', async () => {
    const { input, handleBeforeInput } = setup();
    handleBeforeInput(new InputEvent('beforeinput', {
      cancelable: false, inputType: 'insertFromPaste', data: longText,
    }));
    expect(attachments.stagedAttachments).toEqual([]);
    input.value = longText;
    await nextTick();
    await nextTick();
    await nextTick();

    expect(input.value).toBe('');
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(attachments.stagedAttachments).toHaveLength(1);
    expect(attachments.stagedAttachments[0].status).toBe('done');
  });

  it('restores the exact watcher input after a failed conversion', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    mockInvoke('store_file', () => Promise.reject(new Error('disk full')));
    const { input } = setup('before after');
    const inserted = 'before ' + longText + ' after';
    input.value = inserted;
    await nextTick();
    await nextTick();
    await nextTick();

    expect(input.value).toBe(inserted);
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(attachments.stagedAttachments).toEqual([]);
  });

  it('discards a late result after switching away and back to the same topic', async () => {
    const finish = deferredSave();
    const { handlePaste, input, isProcessing } = setup();
    const pending = handlePaste(pasteEvent(longText));
    await nextTick();
    expect(attachments.stagedAttachments).toHaveLength(1);
    session.currentConversationKey = {
      ownerId: 'agent-a', ownerType: 'agent', topicId: 'topic-b', epoch: 2,
    };
    session.currentConversationKey = {
      ownerId: 'agent-a', ownerType: 'agent', topicId: 'topic-a', epoch: 3,
    };
    expect(attachments.stagedAttachments).toEqual([]);
    finish();
    await pending;

    expect(isProcessing.value).toBe(false);
    expect(input.value).toBe('');
    expect(attachments.stagedAttachments).toEqual([]);
    expect(notifications.addNotification).not.toHaveBeenCalled();
  });

  it('removes its placeholder on disposal and ignores the completed write', async () => {
    const finish = deferredSave();
    const { handlePaste, scope } = setup();
    const pending = handlePaste(pasteEvent(longText));
    await nextTick();
    scope.stop();
    expect(attachments.stagedAttachments).toEqual([]);
    finish();
    await pending;

    expect(attachments.stagedAttachments).toEqual([]);
    expect(notifications.addNotification).not.toHaveBeenCalled();
  });
});
