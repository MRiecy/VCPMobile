import { flushPromises } from '@vue/test-utils';
import { describe, expect, it, vi } from 'vitest';
import { instantiateMermaidSvg, renderMermaidTemplate } from '@/features/chat/utils/mermaidRenderer';

const { render } = vi.hoisted(() => ({ render: vi.fn() }));
vi.mock('pele', () => ({ render }));

describe('Mermaid resource ownership', () => {
  it('gives repeated SVG instances independent IDs and rewrites their internal references', () => {
    const source = '<svg xmlns="http://www.w3.org/2000/svg" id="graph" aria-labelledby="title"><style>#graph .node{fill:url(#gradient)} #node:hover{opacity:0.5}</style><title id="title">Example</title><defs><linearGradient id="gradient"/><clipPath id="clip"/><marker id="arrow"/></defs><g id="node" clip-path="url(#clip)"><path marker-end="url(#arrow)"/><use href="#node"/><animate begin="node.click"/></g></svg>';
    const first = new DOMParser().parseFromString(instantiateMermaidSvg(source), 'image/svg+xml');
    const second = new DOMParser().parseFromString(instantiateMermaidSvg(source), 'image/svg+xml');
    const firstIds = [...first.querySelectorAll('[id]')].map(el => el.id);
    expect(firstIds.length).toBe(6);
    expect(firstIds.every(id => !second.getElementById(id))).toBe(true);
    const g = first.querySelector('g')!;
    expect(first.querySelector('use')?.getAttribute('href')).toBe(`#${g.id}`);
    expect(first.querySelector('animate')?.getAttribute('begin')).toBe(`${g.id}.click`);
    expect(first.documentElement.getAttribute('aria-labelledby')).toBe(first.querySelector('title')?.id);
    expect(first.querySelector('style')?.textContent).toContain(`#${first.documentElement.id}`);
    expect(g.getAttribute('clip-path')).toBe(`url(#${first.querySelector('clipPath')?.id})`);
    expect(first.querySelector('path')?.getAttribute('marker-end')).toBe(`url(#${first.querySelector('marker')?.id})`);
  });

  it('reuses cached Pele templates across calls and rejects aborted requests immediately', async () => {
    const svg = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"/>';
    render.mockReturnValue({ svg, width: 10, height: 10, links: [], type: 'flowchart', mermaidType: 'flowchart-v2' });
    const source = 'graph TD; queueA-->queueB';
    const owner = new AbortController();
    const duplicate = new AbortController();
    const cancelled = new AbortController();
    cancelled.abort();
    const abandoned = renderMermaidTemplate('graph TD; gone-->forever', cancelled.signal).catch(error => error.name);
    const first = renderMermaidTemplate(source, owner.signal);
    const shared = renderMermaidTemplate(source, duplicate.signal);
    await flushPromises();
    await expect(first).resolves.toBe(svg);
    await expect(shared).resolves.toBe(svg);
    expect(await abandoned).toBe('AbortError');
    expect(render).toHaveBeenCalledTimes(1);
    await expect(renderMermaidTemplate(source, new AbortController().signal)).resolves.toBe(svg);
    expect(render).toHaveBeenCalledTimes(1);
  });

  it('does not retain an oversized SVG in the reusable cache', async () => {
    const svg = `<svg xmlns="http://www.w3.org/2000/svg"><text>${'x'.repeat(300000)}</text></svg>`;
    render.mockReturnValue({ svg, width: 10, height: 10, links: [], type: 'flowchart', mermaidType: 'flowchart-v2' });
    const before = render.mock.calls.length;
    await renderMermaidTemplate('oversized-cache-entry', new AbortController().signal);
    await renderMermaidTemplate('oversized-cache-entry', new AbortController().signal);
    expect(render.mock.calls.length - before).toBe(2);
  });
});
