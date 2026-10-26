import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { disposePinia, getActivePinia } from 'pinia';
import { nextTick } from 'vue';
import type { VueWrapper } from '@vue/test-utils';
import InputEnhancer from '@/features/chat/InputEnhancer.vue';
import { useModalHistory } from '@/core/composables/useModalHistory';
import { useChatHistoryStore } from '@/core/stores/chatHistoryStore';
import { useChatSessionStore } from '@/core/stores/chatSessionStore';
import { useShareStore } from '@/core/stores/shareStore';
import { useAttachmentStore } from '@/core/stores/attachmentStore';
import { mountWithPinia } from '@/tests/utils/mount';

let wrapper: VueWrapper;
let textarea: HTMLTextAreaElement;
let contentHeight = 116;
let width = 240;
let resize: () => void;
const disconnect = vi.fn();
const modals = useModalHistory();

async function settle() {
  await nextTick();
  await nextTick();
}

async function expand(text = '第一行\n第二行\n第三行\n第四行\n第五行') {
  await wrapper.get('textarea').setValue(text);
  await settle();
  await wrapper.get('[aria-label="展开输入框"]').trigger('click');
  await settle();
}

beforeEach(() => {
  contentHeight = 116;
  width = 240;
  window.history.replaceState({ vcpRoot: true, vcpMain: true }, '', '#/chat');
  vi.spyOn(window.history, 'back').mockImplementation(() => {
    window.history.replaceState({ vcpRoot: true, vcpMain: true }, '');
    window.dispatchEvent(new PopStateEvent('popstate', { state: window.history.state }));
  });
  vi.stubGlobal('ResizeObserver', class {
    constructor(callback: () => void) { resize = callback; }
    observe() {}
    unobserve() {}
    disconnect = disconnect;
  });
  wrapper = mountWithPinia(InputEnhancer, {
    attachTo: document.body,
    global: {
      directives: { longpress: {}, guide: {} },
      stubs: { GroupStopAllButton: true, GroupInviteBar: true, StagedAttachmentPreview: true, VcpAvatar: true },
    },
  });
  textarea = wrapper.get('textarea').element as HTMLTextAreaElement;
  // happy-dom 无排版：注入浏览器测得的尺寸；真实软换行/键盘几何另行观察。
  textarea.style.lineHeight = '20px';
  textarea.style.paddingTop = '8px';
  textarea.style.paddingBottom = '8px';
  Object.defineProperty(textarea, 'scrollHeight', { configurable: true, get: () => contentHeight });
  Object.defineProperty(textarea, 'clientWidth', { configurable: true, get: () => width });
});

afterEach(async () => {
  wrapper.unmount();
  await settle();
  expect(modals.modalStackLength()).toBe(0);
  const pinia = getActivePinia();
  if (pinia) disposePinia(pinia);
  vi.unstubAllGlobals();
});

describe('chat input expansion', () => {
  it('uses rendered height, including soft wraps, and remeasures changed width', async () => {
    contentHeight = 96; // 四行 + 16px padding
    await wrapper.get('textarea').setValue('没有换行符的连续文字');
    await settle();
    expect(wrapper.find('[aria-label="展开输入框"]').exists()).toBe(false);
    contentHeight = 116;
    resize();
    await settle();
    expect(wrapper.find('[aria-label="展开输入框"]').exists()).toBe(true);
    width = 400;
    contentHeight = 76;
    resize();
    await settle();
    expect(wrapper.find('[aria-label="展开输入框"]').exists()).toBe(false);
  });

  it('keeps the same focused editor, selection and draft through expand and collapse', async () => {
    await wrapper.get('textarea').setValue('第一行\n第二行\n第三行\n第四行\n第五行');
    await settle();
    textarea.focus();
    textarea.setSelectionRange(2, 7);
    const parent = textarea.parentElement;
    await wrapper.get('[aria-label="展开输入框"]').trigger('click');
    expect(wrapper.get('textarea').element).toBe(textarea);
    expect(textarea.parentElement).toBe(parent);
    expect(document.activeElement).toBe(textarea);
    expect([textarea.selectionStart, textarea.selectionEnd]).toEqual([2, 7]);
    expect(modals.modalStackLength()).toBe(1);
    await wrapper.get('[aria-label="缩小输入框"]').trigger('click');
    await settle();
    expect(wrapper.get('textarea').element).toBe(textarea);
    expect(textarea.value).toBe('第一行\n第二行\n第三行\n第四行\n第五行');
    expect([textarea.selectionStart, textarea.selectionEnd]).toEqual([2, 7]);
    expect(wrapper.emitted('send')).toBeUndefined();
  });

  it('does not shrink while shortening the draft or clicking inside; outside closes once', async () => {
    await expand();
    await wrapper.get('textarea').setValue('短文');
    await wrapper.get('.input-writing-area').trigger('click');
    expect(wrapper.find('[role="dialog"]').exists()).toBe(true);
    contentHeight = 36;
    await wrapper.get('.vcp-viewport-surface').trigger('click');
    await settle();
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    expect(textarea.value).toBe('短文');
    expect(window.history.back).toHaveBeenCalledTimes(1);
    expect(wrapper.find('[aria-label="展开输入框"]').exists()).toBe(false);
  });

  it.each(['hardware', 'popstate', 'escape'])('%s returns to the small editor without sending', async (kind) => {
    await expand();
    const draft = textarea.value;
    if (kind === 'hardware') modals.closeTopModal();
    else if (kind === 'escape') window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', cancelable: true }));
    else {
      window.history.replaceState({ vcpRoot: true, vcpMain: true }, '');
      window.dispatchEvent(new PopStateEvent('popstate', { state: window.history.state }));
    }
    await settle();
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    expect(wrapper.get('textarea').element).toBe(textarea);
    expect(textarea.value).toBe(draft);
    expect(wrapper.emitted('send')).toBeUndefined();
    expect(modals.modalStackLength()).toBe(0);
  });

  it('preserves a composition session on the same node and commits it normally', async () => {
    await expand();
    await wrapper.get('textarea').trigger('compositionstart');
    textarea.value += '拼音';
    textarea.dispatchEvent(new Event('input', { bubbles: true }));
    await wrapper.get('[aria-label="缩小输入框"]').trigger('click');
    expect(wrapper.get('textarea').element).toBe(textarea);
    await wrapper.get('textarea').trigger('compositionend');
    await wrapper.get('[aria-label="发送消息"]').trigger('click');
    expect(wrapper.emitted('send')?.[0]?.[0]).toContain('拼音');
  });

  it('keeps ordinary input until persistence acknowledges the send and collapses immediately', async () => {
    await expand();
    const draft = textarea.value;
    await wrapper.get('[aria-label="发送消息"]').trigger('click');
    await settle();
    expect(wrapper.emitted('send')).toEqual([[draft, expect.any(Function)]]);
    expect(textarea.value).toBe(draft);
    const acknowledge = wrapper.emitted('send')![0][1] as () => void;
    // The real composer has a selected conversation before send is enabled.
    vi.spyOn(useChatSessionStore(), 'isConversationCurrent').mockReturnValue(true);
    acknowledge();
    await settle();
    expect(textarea.value).toBe('');
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
  });

  it('keeps the existing share-draft sending path', async () => {
    const share = useShareStore();
    vi.spyOn(share, 'activeDraft', 'get').mockReturnValue({
      deliveryId: 'share-1', generation: 1, revision: 1, state: 'draft',
      text: '分享草稿', items: [], attachments: [],
    });
    const send = vi.spyOn(useChatHistoryStore(), 'sendMessage').mockResolvedValue(undefined);
    await expand();
    const draft = textarea.value;
    await wrapper.get('[aria-label="发送消息"]').trigger('click');
    expect(send).toHaveBeenCalledWith(draft);
    expect(wrapper.emitted('send')).toBeUndefined();
  });

  it('does not clear a newer input when the previous send is acknowledged', async () => {
    await wrapper.get('textarea').setValue('sent draft');
    await wrapper.get('[aria-label="发送消息"]').trigger('click');
    const acknowledge = wrapper.emitted('send')![0][1] as () => void;
    await wrapper.get('textarea').setValue('new draft');
    acknowledge();
    await settle();
    expect(textarea.value).toBe('new draft');
  });

  it('consumes a parked ordinary draft without clearing a newly received share', async () => {
    const session = useChatSessionStore();
    session.setConversation({ id: 'agent-a', type: 'agent' }, 'topic-a');
    const attachments = useAttachmentStore();
    attachments.stagedAttachments = [{ id: 'ordinary-file', name: 'ordinary.txt', type: 'text/plain', src: '', size: 4, status: 'done' }];
    await wrapper.get('textarea').setValue('ordinary draft');
    await wrapper.get('[aria-label="发送消息"]').trigger('click');
    const acknowledge = wrapper.emitted('send')![0][1] as () => void;
    const share = useShareStore();
    share.current = {
      generation: 1, deliveryId: 'incoming', revision: 1, state: 'draft',
      agentId: 'agent-a', topicId: 'topic-a', text: 'new share', items: [],
      attachments: [{ id: 'incoming-file', name: 'incoming.txt', type: 'text/plain', src: '', size: 3, status: 'done' }],
    };
    await settle();
    acknowledge();
    await settle();
    expect(textarea.value).toBe('new share');
    expect(attachments.stagedAttachments.map(item => item.name)).toEqual(['incoming.txt']);
    share.current = { ...share.current, state: 'discarded' };
    await settle();
    expect(textarea.value).toBe('');
    expect(attachments.stagedAttachments).toEqual([]);
  });

  it('keeps long edit and share refills as text without staging TXT attachments', async () => {
    const session = useChatSessionStore();
    session.setConversation({ id: 'agent-a', type: 'agent' }, 'topic-a');
    const history = useChatHistoryStore();
    const text = '长消息正文'.repeat(300);
    history.editingMessage = { key: { conversation: session.currentConversationKey!, messageId: 'old' }, initialContent: text };
    await settle();
    expect(textarea.value).toBe(text);
    expect(useAttachmentStore().stagedAttachments).toEqual([]);
    history.editingMessage = null;
    await settle();
    useShareStore().current = {
      generation: 1, deliveryId: 'incoming-long', revision: 1, state: 'draft',
      agentId: 'agent-a', topicId: 'topic-a', text, items: [], attachments: [],
    };
    await settle();
    expect(textarea.value).toBe(text);
    expect(useAttachmentStore().stagedAttachments).toEqual([]);
  });

  it('collapses on disabled or conversation changes and releases observation on unmount', async () => {
    await expand();
    await wrapper.setProps({ disabled: true });
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    await wrapper.setProps({ disabled: false });
    await expand();
    useChatSessionStore().setConversation({ id: 'agent-2', type: 'agent', name: '另一助手' }, 'topic-2');
    await settle();
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    await expand();
    expect(modals.modalStackLength()).toBe(1);
    wrapper.unmount();
    expect(modals.modalStackLength()).toBe(0);
    expect(disconnect).toHaveBeenCalledOnce();
  });

  it('makes attachments available after collapsing the editor, retaining the draft', async () => {
    await expand();
    const draft = textarea.value;
    expect(wrapper.find('[aria-label="添加附件"]').exists()).toBe(false);
    await wrapper.get('[aria-label="缩小输入框"]').trigger('click');
    await wrapper.get('[aria-label="添加附件"]').trigger('click');
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    expect(textarea.value).toBe(draft);
    expect(modals.modalStackLength()).toBe(0);
  });
});
