import { describe, expect, it, vi } from 'vitest';
import type { ContentBlock, MarkdownNode } from '@/core/types/chat';
import { createMarkdownPresentation } from '@/features/chat/utils/markdownPresentation';

const paragraph = (value: string, hash?: number): MarkdownNode => ({
  type: 'paragraph', children: [{ type: 'text', value }], ...(hash === undefined ? {} : { hash }),
});
const markdown = (...nodes: MarkdownNode[]): ContentBlock => ({ type: 'markdown', nodes });
const bubble = (blocks: ContentBlock[], id = 'message-bubble-0') => ({ id, blocks });

describe('message-local Markdown presentation', () => {
  it('reuses the same top-level units when final compilation combines transport blocks', () => {
    const projection = createMarkdownPresentation();
    const initial = projection.project('message', [bubble([
      markdown(paragraph('first', 10)), markdown(paragraph('second', 20)),
    ])]);
    const finalSource = markdown(paragraph('first', 10), paragraph('second', 20), paragraph('last', 30));
    const final = projection.project('message', [bubble([finalSource])]);
    expect(final[0].blocks).toHaveLength(3);
    expect(final[0].blocks[0]).toBe(initial[0].blocks[0]);
    expect(final[0].blocks[1]).toBe(initial[0].blocks[1]);
    expect(final[0].blocks[2].nodes?.[0]).toBe(finalSource.nodes?.[2]);
    expect(finalSource.nodes).toHaveLength(3);
    expect(final[0].blocks.every(block => block.hash === undefined)).toBe(true);
  });

  it('compares actual content even when distinct u64 hashes round to the same JS number', () => {
    const projection = createMarkdownPresentation();
    const rounded = Number(9_007_199_254_740_992n);
    expect(rounded).toBe(Number(9_007_199_254_740_993n));
    const before = projection.project('message', [bubble([markdown(paragraph('old', rounded))])]);
    const after = projection.project('message', [bubble([markdown(paragraph('authoritative', rounded))])]);
    expect(after[0].blocks[0]).not.toBe(before[0].blocks[0]);
    expect(projection.getKey(after[0].blocks[0])).not.toBe(projection.getKey(before[0].blocks[0]));
    expect(after[0].blocks[0].nodes).toEqual([paragraph('authoritative', rounded)]);
  });

  it('assigns independent identities to repeated nodes and to nodes without hashes', () => {
    const projection = createMarkdownPresentation();
    const input = () => [bubble([
      markdown(paragraph('repeat', 10)), markdown(paragraph('repeat', 10)),
      markdown({ type: 'thematic_break' }), markdown({ type: 'thematic_break' }),
    ])];
    const before = projection.project('message', input())[0].blocks;
    const after = projection.project('message', input())[0].blocks;
    expect(new Set(before.map(projection.getKey)).size).toBe(4);
    expect(after.every((block, index) => block === before[index])).toBe(true);
  });

  it('keeps list, table and code nodes atomic and isolates bubble and message identities', () => {
    const projection = createMarkdownPresentation();
    const nodes: MarkdownNode[] = [
      { type: 'list', ordered: false, items: [[paragraph('item')], [paragraph('other')]], hash: 1 },
      { type: 'table', header: [[{ type: 'text', value: 'header' }]], rows: [], wrapper_class: null, hash: 2 },
      { type: 'code_block', lang: 'js', code: '1 + 1', highlighted_html: null, theme: null, hash: 3 },
    ];
    const before = projection.project('message', [bubble([markdown(...nodes)])])[0].blocks;
    expect(before).toHaveLength(3);
    expect(before.map(block => block.nodes?.[0])).toEqual(nodes);
    const otherBubble = projection.project('message', [bubble([markdown(...nodes)], 'message-bubble-1')])[0].blocks;
    expect(otherBubble.every((block, index) => block !== before[index])).toBe(true);
    const otherMessage = projection.project('different-message', [bubble([markdown(...nodes)], 'message-bubble-1')])[0].blocks;
    expect(otherMessage.every((block, index) => block !== otherBubble[index])).toBe(true);
  });

  it('keeps raw HTML contexts, standalone styles and disabled user projections unchanged', () => {
    const projection = createMarkdownPresentation();
    const rawContainer = markdown(
      { type: 'raw_html', content: '<section>' }, paragraph('inside'), { type: 'raw_html', content: '</section>' },
    );
    const inlineContainer = markdown({ type: 'paragraph', children: [
      { type: 'raw_html_inline', content: '<span>' }, { type: 'text', value: 'inside' },
      { type: 'raw_html_inline', content: '</span>' },
    ] }, paragraph('next'));
    const style = markdown({ type: 'raw_html', content: '<style>.x{color:red}</style>' });
    const sources = [rawContainer, inlineContainer, style];
    const result = projection.project('message', [bubble(sources)])[0].blocks;
    expect(result.every((block, index) => block === sources[index])).toBe(true);
    const user = [bubble([markdown(paragraph('one'), paragraph('two'))])];
    expect(projection.project('user', user, false)).toBe(user);
  });

  it('hands reveal eligibility only to newly created views, without replaying reused nodes', () => {
    const onCreate = vi.fn();
    const projection = createMarkdownPresentation(onCreate);
    const first = markdown(paragraph('first', 1));
    const initial = projection.project('message', [bubble([first])]);
    expect(onCreate).toHaveBeenCalledWith(first, initial[0].blocks[0]);
    onCreate.mockClear();
    const next = markdown(paragraph('new', 2));
    const appended = projection.project('message', [bubble([first, next])]);
    expect(onCreate).toHaveBeenCalledTimes(1);
    expect(onCreate).toHaveBeenCalledWith(next, appended[0].blocks[1]);
  });
});
