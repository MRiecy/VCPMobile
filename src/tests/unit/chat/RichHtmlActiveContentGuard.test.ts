import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import ToolBlock from '@/features/chat/blocks/ToolBlock.vue';
import HtmlPreviewBlock from '@/features/chat/blocks/HtmlPreviewBlock.vue';
import {
  clearHtmlCache,
  filterTrustedRichHtml,
  renderMarkdownNodes,
} from '@/core/utils/astRenderer';
import { applyFrame, cleanupRegistry, rebuildSnapshot } from '@/core/utils/astExecutor';
import { invokeMock } from '@/tests/mocks/tauri';
import type { ContentBlock, MarkdownNode } from '@/core/types/chat';

vi.mock('@/core/stores/theme', () => ({
  useThemeStore: () => ({ isDarkResolved: false }),
}));

// happy-dom rejects DOMPurify's WHOLE_DOCUMENT node hoisting even though Chromium/WebView accepts
// it. This suite verifies the iframe sandbox boundary; DOMPurify's patched version is audit-gated.
vi.mock('dompurify', () => ({
  default: {
    sanitize: (dirty: string) => dirty,
  },
}));

const HOST_HANDLER = "window.__TAURI_INTERNALS__.invoke('read_settings')";

function parseHtml(html: string): HTMLDivElement {
  const host = document.createElement('div');
  host.innerHTML = html;
  document.body.appendChild(host);
  return host;
}

function expectHostCapabilityBlocked(host: ParentNode): void {
  expect(host.querySelector('[onerror]')).toBeNull();
  expect(host.querySelector('[onload]')).toBeNull();
  expect(host.querySelector('script')).toBeNull();
  const link = host.querySelector('a');
  if (link) expect(link.hasAttribute('href')).toBe(false);
  expect(invokeMock).not.toHaveBeenCalled();
}

describe('trusted-circle rich HTML active-content guard', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    clearHtmlCache();
    (window as any).__TAURI_INTERNALS__ = { invoke: invokeMock };
  });

  it('blocks direct host capabilities while preserving rich visuals and local DOM interactions', () => {
    const html = filterTrustedRichHtml(`
      <style>.panel { display: grid; color: rgb(12, 34, 56); }</style>
      <custom-panel id="panel" class="panel" data-mode="rich" aria-label="demo"
        onclick="this.classList.toggle('active')">
        <img id="danger-image" src="https://example.com/image.png" onerror="${HOST_HANDLER}">
        <img id="data-image" src="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg'/%3E">
        <svg viewBox="0 0 10 10" onload="fetch('https://example.com/leak')">
          <defs><linearGradient id="gradient"><stop offset="0%" stop-color="red" /></linearGradient></defs>
          <rect width="10" height="10" fill="url(#gradient)" />
        </svg>
        <math><mrow><mi>x</mi><mo>=</mo><mn>1</mn></mrow></math>
        <details open><summary>More</summary><canvas width="12" height="8"></canvas></details>
        <form><input name="query"><button type="button">Local action</button></form>
        <a id="danger-link" href="java&#x0a;script:${HOST_HANDLER}">danger</a>
        <a id="safe-link" href="https://example.com/docs" target="_blank">docs</a>
        <iframe id="preview-frame" srcdoc="<style>body{color:red}</style><button onclick=&quot;this.hidden=true&quot;>ok</button><script>${HOST_HANDLER}</script>" allow="fullscreen"></iframe>
        <script>${HOST_HANDLER}</script>
        <base href="https://example.com/">
        <meta http-equiv="refresh" content="0;url=https://example.com/">
        <object data="https://example.com/active.html"></object>
      </custom-panel>
    `);
    const host = parseHtml(html);

    expectHostCapabilityBlocked(host);
    expect(host.querySelector('base, meta[http-equiv="refresh"], object')).toBeNull();

    const panel = host.querySelector('custom-panel');
    expect(panel).not.toBeNull();
    expect(panel?.getAttribute('class')).toBe('panel');
    expect(panel?.getAttribute('data-mode')).toBe('rich');
    expect(panel?.getAttribute('aria-label')).toBe('demo');
    expect(panel?.getAttribute('onclick')).toBe("this.classList.toggle('active')");
    expect(host.querySelector('style')?.textContent).toContain('display: grid');
    expect(host.querySelector('svg linearGradient')).not.toBeNull();
    expect(host.querySelector('math mrow')).not.toBeNull();
    expect(host.querySelector('canvas')).not.toBeNull();
    expect(host.querySelector('form input[name="query"]')).not.toBeNull();
    expect(host.querySelector('#data-image')?.getAttribute('src')).toContain('data:image/svg+xml');
    expect(host.querySelector('#safe-link')?.getAttribute('href')).toBe('https://example.com/docs');
    expect(host.querySelector('#safe-link')?.getAttribute('rel')).toContain('noopener');

    const iframe = host.querySelector('#preview-frame');
    const sandbox = iframe?.getAttribute('sandbox') || '';
    expect(sandbox).toContain('allow-scripts');
    expect(sandbox).not.toContain('allow-same-origin');
    expect(iframe?.getAttribute('srcdoc')).toContain('this.hidden=true');
    expect(iframe?.getAttribute('srcdoc')).not.toContain('__TAURI_INTERNALS__');
  });

  it('guards stable AST output, including raw HTML and Markdown URLs', () => {
    const nodes: MarkdownNode[] = [
      {
        type: 'raw_html',
        content: `<section><img src="x" onerror="${HOST_HANDLER}"></section>`,
      },
      {
        type: 'paragraph',
        children: [
          { type: 'raw_html_inline', content: `<svg onload="fetch('/leak')"></svg>` },
          { type: 'link', href: 'javascript:alert(1)', children: [{ type: 'text', value: 'bad' }] },
          { type: 'link', href: 'https://example.com', children: [{ type: 'text', value: 'good' }] },
        ],
      },
    ];

    const host = parseHtml(renderMarkdownNodes(nodes, 'stable-message', 1));

    expectHostCapabilityBlocked(host);
    const links = host.querySelectorAll('a');
    expect(links[0].hasAttribute('href')).toBe(false);
    expect(links[1].getAttribute('href')).toBe('https://example.com');
  });

  it('guards AST snapshot and replace/tail mutations before they reach the live sandbox', () => {
    const sandbox = document.createElement('div');
    document.body.appendChild(sandbox);

    rebuildSnapshot([
      { type: 'raw_html', content: `<img src="x" onerror="${HOST_HANDLER}">` },
      {
        type: 'paragraph',
        children: [
          { type: 'raw_html_inline', content: `<svg onload="fetch('/leak')"></svg>` },
          { type: 'link', href: 'javascript:alert(1)', children: [{ type: 'text', value: 'bad' }] },
        ],
      },
    ], 'stream-message', sandbox);

    expectHostCapabilityBlocked(sandbox);

    const result = applyFrame([
      {
        op: 'replace',
        id: 't0',
        node: {
          type: 'raw_html',
          content: `<iframe srcdoc="<script>${HOST_HANDLER}</script><b>kept</b>"></iframe><a href="vbscript:msgbox(1)">bad</a>`,
        },
      } as any,
    ], 'stream-message', sandbox);

    expect(result.ok).toBe(true);
    expect(sandbox.querySelector('script')).toBeNull();
    expect(sandbox.querySelector('iframe')?.getAttribute('sandbox')).not.toContain('allow-same-origin');
    expect(sandbox.querySelector('iframe')?.getAttribute('srcdoc')).toContain('<b>kept</b>');
    expect(sandbox.querySelector('a')?.hasAttribute('href')).toBe(false);
    expect(invokeMock).not.toHaveBeenCalled();

    cleanupRegistry('stream-message');
  });

  it('guards marked ToolBlock output without flattening its Markdown or safe local HTML', () => {
    const block: ContentBlock = {
      type: 'tool-result',
      tool_name: 'RichResult',
      status: 'success',
      details: [{
        key: 'result',
        value: `## Result\n\n<table><tr><td onclick="this.classList.toggle('selected')">kept</td></tr></table><img src="x" onerror="${HOST_HANDLER}"><script>${HOST_HANDLER}</script>`,
      }],
    };
    const wrapper = mount(ToolBlock, {
      props: {
        type: 'tool-result',
        block,
        defaultExpanded: true,
      },
    });

    const rendered = wrapper.get('.vcp-markdown-block').element;
    expect(rendered.querySelector('h2')?.textContent).toBe('Result');
    expect(rendered.querySelector('table td')?.getAttribute('onclick')).toBe("this.classList.toggle('selected')");
    expect(rendered.querySelector('[onerror]')).toBeNull();
    expect(rendered.querySelector('script')).toBeNull();
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('keeps unrestricted scripts inside the existing sandboxed HTML Preview', async () => {
    const wrapper = mount(HtmlPreviewBlock, {
      props: {
        content: '<main><script>document.body.dataset.preview = "active"</script><p>Preview</p></main>',
        messageId: 'preview-message',
      },
    });
    const previewButton = wrapper.findAll('button').find((button) => button.text() === '预览');
    expect(previewButton).toBeDefined();
    await previewButton!.trigger('click');
    const iframe = wrapper.get('iframe');
    const sandbox = iframe.attributes('sandbox') || '';
    const srcdoc = iframe.attributes('srcdoc') || '';

    expect(sandbox).toContain('allow-scripts');
    expect(sandbox).not.toContain('allow-same-origin');
    expect(srcdoc).toContain('document.body.dataset.preview');
    expect(srcdoc).toContain('<p>Preview</p>');
  });
});
