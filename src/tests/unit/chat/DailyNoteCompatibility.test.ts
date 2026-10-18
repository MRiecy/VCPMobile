import { beforeEach, describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import DiaryBlock from '@/features/chat/blocks/DiaryBlock.vue';
import { clearHtmlCache } from '@/core/utils/astRenderer';
import type { ContentBlock, MarkdownNode } from '@/core/types/chat';

function paragraph(value: string): MarkdownNode {
  return {
    type: 'paragraph',
    children: [{ type: 'text', value }],
  };
}

describe('DiaryBlock', () => {
  beforeEach(() => {
    clearHtmlCache();
  });

  it('keeps update target and replacement AST caches isolated', () => {
    const block: ContentBlock = {
      type: 'diary-update',
      maid: 'Sakura',
      target: 'old source',
      replace: 'new source',
      target_nodes: [paragraph('OLD AST')],
      replace_nodes: [paragraph('NEW AST')],
      hash: 21,
    };
    const wrapper = mount(DiaryBlock, {
      props: { block, messageId: 'message-update' },
    });
    const sides = wrapper.findAll('.vcp-diary-update-content');

    expect(sides).toHaveLength(2);
    expect(sides[0].text()).toContain('OLD AST');
    expect(sides[1].text()).toContain('NEW AST');
  });

});
