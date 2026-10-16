import { flushPromises } from '@vue/test-utils';
import { describe, expect, it, vi } from 'vitest';
import { instantiateMermaidSvg, renderMermaidTemplate } from '@/features/chat/utils/mermaidRenderer';

const { render, initialize } = vi.hoisted(() => ({ render: vi.fn(), initialize: vi.fn() }));
vi.mock('mermaid', () => ({ default: { render, initialize } }));

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

  it('merges subscribers, serializes themes, and skips a queued operation with no consumers', async () => {
    const completions: Array<(result: { svg: string }) => void> = [];
    render.mockImplementation(() => new Promise(resolve => completions.push(resolve)));
    const source = 'graph TD; queueA-->queueB';
    const owner = new AbortController();
    const duplicate = new AbortController();
    const cancelled = new AbortController();
    const first = renderMermaidTemplate(source, false, owner.signal);
    const shared = renderMermaidTemplate(source, false, duplicate.signal);
    const nextTheme = renderMermaidTemplate(source, true, new AbortController().signal);
    const abandoned = renderMermaidTemplate('graph TD; gone-->forever', false, cancelled.signal).catch(error => error.name);
    cancelled.abort();
    await flushPromises();
    expect(render).toHaveBeenCalledTimes(1);
    const svg = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"/>';
    completions.shift()!({ svg });
    await expect(first).resolves.toBe(svg);
    await expect(shared).resolves.toBe(svg);
    await flushPromises();
    expect(render).toHaveBeenCalledTimes(2);
    expect(initialize.mock.calls.map(args => args[0].theme)).toEqual(['default', 'dark']);
    completions.shift()!({ svg });
    await nextTheme;
    expect(await abandoned).toBe('AbortError');
    await expect(renderMermaidTemplate(source, false, new AbortController().signal)).resolves.toBe(svg);
    expect(render).toHaveBeenCalledTimes(2);
  });

  it('does not retain an oversized SVG in the reusable cache', async () => {
    const svg = `<svg xmlns="http://www.w3.org/2000/svg"><text>${'x'.repeat(300000)}</text></svg>`;
    render.mockResolvedValue({ svg });
    const before = render.mock.calls.length;
    await renderMermaidTemplate('oversized-cache-entry', false, new AbortController().signal);
    await renderMermaidTemplate('oversized-cache-entry', false, new AbortController().signal);
    expect(render.mock.calls.length - before).toBe(2);
  });
});
