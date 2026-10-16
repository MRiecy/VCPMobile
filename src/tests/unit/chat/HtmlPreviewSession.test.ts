import { mount } from '@vue/test-utils';
import { describe, expect, it, vi } from 'vitest';
import HtmlPreviewBlock from '@/features/chat/blocks/HtmlPreviewBlock.vue';
import { sandboxImageBridge } from '@/core/utils/sandboxImageBridge';

vi.mock('@/core/stores/theme', () => ({ useThemeStore: () => ({ isDarkResolved: false }) }));
vi.mock('@/core/composables/useModalHistory', () => ({ useModalHistory: () => ({ registerModal: vi.fn(), unregisterModal: vi.fn() }) }));

describe('HTML preview sessions', () => {
  it('refreshes its own iframe and invalidates the previous nonce without touching a sibling', async () => {
    const options = { props: { content: '<canvas width="200" height="100"></canvas>', messageId: 'one' }, global: { stubs: { Teleport: true, Transition: true } } };
    const first = mount(HtmlPreviewBlock, options);
    const second = mount(HtmlPreviewBlock, { ...options, props: { ...options.props, messageId: 'two' } });
    try {
      await first.findAll('button').find(button => button.text() === '预览')!.trigger('click');
      await second.findAll('button').find(button => button.text() === '预览')!.trigger('click');
      const sibling = second.get('iframe').element;
      const previous = first.get('iframe').attributes('data-vcp-image-nonce');
      const refresh = first.findAll('button').find(button => button.find('.i-ph\\:arrow-clockwise-bold').exists());
      expect(refresh).toBeDefined();
      await refresh!.trigger('click');
      expect(first.get('iframe').attributes('data-vcp-image-nonce')).not.toBe(previous);
      expect(second.get('iframe').element).toBe(sibling);
      expect(second.get('iframe').attributes('data-vcp-image-nonce')).not.toBe(previous);
    } finally { first.unmount(); second.unmount(); }
  });

  it('emits a syntactically valid sandbox script with an escaped nonce', () => {
    expect(() => new Function(sandboxImageBridge('nonce"with\\characters'))).not.toThrow();
  });
});
