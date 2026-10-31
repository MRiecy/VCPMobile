import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { InlineNode, MarkdownNode } from '@/core/types/chat';
import {
  clearHtmlCache,
  filterTrustedRichHtml,
  renderMarkdownNodes,
} from '@/core/utils/astRenderer';

function parseHtml(html: string): HTMLDivElement {
  const host = document.createElement('div');
  host.innerHTML = html;
  return host;
}

function text(value: string): InlineNode {
  return { type: 'text', value };
}

describe('AST generated-HTML fast path', () => {
  beforeEach(() => clearHtmlCache());

  it('skips hidden DOM parsing for generated nodes while preserving the guarded DOM result', () => {
    const nodes: MarkdownNode[] = [
      { type: 'heading', level: 2, children: [text('Title <script>')] },
      {
        type: 'paragraph',
        children: [
          { type: 'strong', children: [{ type: 'emphasis', children: [{ type: 'strikethrough', children: [text('formatted & text')] }] }] },
          { type: 'code', value: '<img onerror="fetch(1)">' },
          { type: 'break' },
          { type: 'inline_math', content: 'a < b', display_mode: false },
          { type: 'inline_math', content: 'x^2', display_mode: true },
          { type: 'vcp_custom', kind: 'quote', children: [text('“quoted”')] },
          { type: 'vcp_custom', kind: 'alert', value: '<script>literal</script>' },
          { type: 'link', href: 'java\nscript:alert(1)', title: null, needs_asset_conversion: false, children: [text('blocked')] },
          { type: 'link', href: 'https://example.com?a=1&b=2', title: '"title"', needs_asset_conversion: false, children: [text('allowed')] },
          { type: 'image', src: 'https://example.com/image.png', alt: '<image>', title: 'image', needs_asset_conversion: false },
        ],
      },
      { type: 'blockquote', children: [{ type: 'paragraph', children: [text('quoted paragraph')] }] },
      { type: 'list', ordered: false, items: [[{ type: 'paragraph', children: [text('bullet')] }]] },
      { type: 'list', ordered: true, items: [[{ type: 'paragraph', children: [text('numbered')] }]] },
      { type: 'table', wrapper_class: 'vcp-scrollable no-swipe', header: [[text('header')]], rows: [[[text('cell')]]] },
      { type: 'code_block', lang: 'text', code: '<script>code</script>', highlighted_html: null, theme: null },
      { type: 'code_block', lang: 'mermaid', code: 'graph TD; A["a < b"]-->B', highlighted_html: null, theme: null },
      { type: 'thematic_break' },
    ];
    const createElement = vi.spyOn(document, 'createElement');
    const html = renderMarkdownNodes(nodes, 'generated');
    expect(createElement.mock.calls.some(([tag]) => tag === 'template')).toBe(false);

    // The previous pipeline always performed this guard before the final DOM insertion.
    const previousDom = parseHtml(filterTrustedRichHtml(html));
    const host = parseHtml(html);
    expect(host.innerHTML).toBe(previousDom.innerHTML);
    expect(host.querySelector('script, [onerror]')).toBeNull();
    expect(host.querySelector('h2')?.textContent).toBe('Title <script>');
    expect(host.querySelector('a')?.hasAttribute('href')).toBe(false);
    expect(host.querySelectorAll('a')[1].getAttribute('href')).toBe('https://example.com?a=1&b=2');
    expect(host.querySelector('img')?.getAttribute('alt')).toBe('<image>');
    expect(host.querySelector('td')?.textContent).toBe('cell');
    expect(host.querySelector('[data-content-kind="mermaid"]')?.getAttribute('data-mermaid-source'))
      .toBe('graph TD; A["a < b"]-->B');
  });

  it('escapes generated class attributes without opening an HTML parsing boundary', () => {
    const injectedClass = 'custom" onclick="fetch(1)';
    const nodes: MarkdownNode[] = [
      { type: 'table', wrapper_class: injectedClass, header: [], rows: [] },
      { type: 'paragraph', children: [{ type: 'vcp_custom', kind: injectedClass, value: 'kept' }] },
    ];
    const createElement = vi.spyOn(document, 'createElement');
    const html = renderMarkdownNodes(nodes, 'classes');
    expect(createElement.mock.calls.some(([tag]) => tag === 'template')).toBe(false);
    const host = parseHtml(html);
    expect(host.querySelector('[onclick]')).toBeNull();
    expect(host.querySelector('div')?.getAttribute('class')).toBe(injectedClass);
    expect(host.querySelector('span')?.getAttribute('class')).toBe(`vcp-custom-${injectedClass}`);
  });

  it('keeps raw opening and closing tags in one complete filtering context', () => {
    const nodes: MarkdownNode[] = [
      { type: 'raw_html', content: '<section data-container onclick="this.hidden=true">' },
      {
        type: 'paragraph',
        children: [
          { type: 'raw_html_inline', content: '<span data-inline>' },
          { type: 'strong', children: [text('inside')] },
          { type: 'raw_html_inline', content: '</span>' },
        ],
      },
      { type: 'raw_html', content: '</section>' },
    ];
    const createElement = vi.spyOn(document, 'createElement');
    const html = renderMarkdownNodes(nodes, 'raw-context');
    expect(createElement.mock.calls.filter(([tag]) => tag === 'template')).toHaveLength(1);
    const host = parseHtml(html);
    expect(host.children).toHaveLength(1);
    expect(host.querySelector('section > p > span > strong')?.textContent).toBe('inside');
    expect(host.querySelector('section')?.getAttribute('onclick')).toBe('this.hidden=true');
  });

  it('retains the HTML guard for raw fragments nested through generated containers', () => {
    const nestedInline: InlineNode = {
      type: 'strong',
      children: [{
        type: 'emphasis',
        children: [{
          type: 'strikethrough',
          children: [{
            type: 'link', href: 'https://example.com', title: null, needs_asset_conversion: false,
            children: [{
              type: 'vcp_custom', kind: 'quote',
              children: [{ type: 'raw_html_inline', content: '<img src="x" onerror="fetch(1)"><b>kept</b>' }],
            }],
          }],
        }],
      }],
    };
    const nodes: MarkdownNode[] = [{
      type: 'list', ordered: false,
      items: [[{
        type: 'blockquote',
        children: [
          { type: 'table', wrapper_class: null, header: [[nestedInline]], rows: [] },
          { type: 'raw_html', content: '<script>window.__TAURI_INTERNALS__.invoke("read_settings")</script>' },
        ],
      }]],
    }];
    const host = parseHtml(renderMarkdownNodes(nodes, 'nested-raw'));
    expect(host.querySelector('script, [onerror]')).toBeNull();
    expect(host.querySelector('li blockquote table th strong em del a span b')?.textContent).toBe('kept');
  });

  it('still guards pre-rendered highlighting and recursively hardens iframe srcdoc', () => {
    const nodes: MarkdownNode[] = [{
      type: 'code_block', lang: 'html', code: 'source', theme: null,
      highlighted_html: '<pre class="vcp-code-block"><code><span>highlight</span><img src="x" onerror="fetch(1)"></code></pre>'
        + '<iframe sandbox="allow-scripts allow-same-origin allow-top-navigation" srcdoc="<b>frame</b><script>fetch(1)</script>"></iframe>',
    }];
    const host = parseHtml(renderMarkdownNodes(nodes, 'highlighted'));
    expect(host.querySelector('[onerror], script')).toBeNull();
    expect(host.querySelector('pre code span')?.textContent).toBe('highlight');
    const iframe = host.querySelector('iframe');
    expect(iframe?.getAttribute('sandbox')).toBe('allow-scripts');
    expect(iframe?.getAttribute('srcdoc')).toBe('<b>frame</b>');
  });
});
