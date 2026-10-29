import { afterEach, describe, expect, it, vi } from 'vitest';
import { render } from 'pele';

// Vitest disables CSS imports globally, including ?raw. Read the actual packaged stylesheet
// so these integration checks still exercise its font registry and selector scoping.
vi.mock('katex/dist/katex.min.css?raw', async () => {
  const { readFile } = await import('node:fs/promises');
  return { default: await readFile('node_modules/katex/dist/katex.min.css', 'utf8') };
});
if (!document.doctype) document.insertBefore(document.implementation.createDocumentType('html', '', ''), document.documentElement);
// happy-dom does not implement compatMode even with the standards-mode doctype above.
Object.defineProperty(document, 'compatMode', { configurable: true, value: 'CSS1Compat' });

const originalFonts = Object.getOwnPropertyDescriptor(document, 'fonts');
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.resetModules();
  if (originalFonts) Object.defineProperty(document, 'fonts', originalFonts);
  else Reflect.deleteProperty(document, 'fonts');
});

function mockFontLoading(load: () => Promise<unknown> = () => Promise.resolve()) {
  const families: string[] = [];
  class DiagramFontFace {
    constructor(family: string, public source: string) { families.push(family); }
    async load() { await load(); return this; }
  }
  vi.stubGlobal('FontFace', DiagramFontFace);
  Object.defineProperty(document, 'fonts', { configurable: true, value: { add: vi.fn() } });
  // happy-dom has neither layout nor the shadow-tree font cascade. Browser probes cover
  // real font discovery/geometry; this quoted family exercises the packaged font lookup.
  vi.stubGlobal('getComputedStyle', () => ({
    fontFamily: '"VcpDiagramKaTeX_SansSerif"', fontStyle: 'normal', fontWeight: '400',
  }));
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 0, 40, 24));
  return families;
}

describe('Pele KaTeX ownership', () => {
  it('preserves real KaTeX HTML, cancellation strokes, scoped CSS and embedded font resources', async () => {
    const families = mockFontLoading();
    const { renderWithMermaidMath } = await import('@/features/chat/utils/mermaidMath');
    const { instantiateMermaidSvg } = await import('@/features/chat/utils/mermaidRenderer');
    const source = 'flowchart LR\nA["$$\\cancel{\\mathsf{x}}$$"] --> B';
    const run = vi.fn(math => render(source, { math }).svg);
    const before = document.body.childElementCount;
    const template = await renderWithMermaidMath(run, new AbortController().signal);
    const doc = new DOMParser().parseFromString(instantiateMermaidSvg(template), 'image/svg+xml');
    expect(doc.querySelector('parsererror')).toBeNull();
    expect(doc.querySelector('foreignObject .vcp-diagram-math .katex-html')).not.toBeNull();
    expect(doc.querySelector('foreignObject svg line')).not.toBeNull();
    expect(doc.querySelector('foreignObject math')).not.toBeNull();
    expect(families).toContain('VcpDiagramKaTeX_SansSerif');
    expect(doc.querySelector('style')?.textContent).toMatch(/data:[^;]*;base64,/);
    const style = doc.querySelector('style')!.textContent!.replace(/@font-face\{[^}]*\}/g, '');
    for (const rule of style.matchAll(/(?:^|})([^{}]+)\{/g)) {
      expect(rule[1].split(',').every(selector => selector.trim().startsWith('.vcp-diagram-math'))).toBe(true);
    }
    expect(document.body.childElementCount).toBe(before);
    expect(run).toHaveBeenCalledTimes(2);
    run.mockClear();
    await renderWithMermaidMath(run, new AbortController().signal);
    expect(run).toHaveBeenCalledOnce();
  });

  it('releases measurement DOM and the cancelled caller while shared font preparation finishes', async () => {
    let release!: () => void;
    const fontGate = new Promise<void>(resolve => { release = resolve; });
    mockFontLoading(() => fontGate);
    const { renderWithMermaidMath } = await import('@/features/chat/utils/mermaidMath');
    const source = 'flowchart LR\nA["$$x^3$$"] --> B';
    const controller = new AbortController();
    const before = document.body.childElementCount;
    const cancelled = renderWithMermaidMath(math => render(source, { math }).svg, controller.signal);
    const survivor = renderWithMermaidMath(math => render(source, { math }).svg, new AbortController().signal);
    const rejection = expect(cancelled).rejects.toMatchObject({ name: 'AbortError' });
    controller.abort();
    await rejection;
    expect(document.body.childElementCount).toBe(before);
    release();
    await expect(survivor).resolves.toContain('foreignObject');
    expect(document.body.childElementCount).toBe(before);
  });

  it('keeps a shared template alive when only one consumer cancels during font preparation', async () => {
    let release!: () => void;
    const gate = new Promise<void>(resolve => { release = resolve; });
    const families = mockFontLoading(() => gate);
    const { renderMermaidTemplate } = await import('@/features/chat/utils/mermaidRenderer');
    const source = 'flowchart LR\nA["$$x^4$$"] --> B';
    const owner = new AbortController();
    const cancelled = renderMermaidTemplate(source, owner.signal);
    const survivor = renderMermaidTemplate(source, new AbortController().signal);
    await vi.waitFor(() => expect(families.length).toBeGreaterThan(0));
    const rejection = expect(cancelled).rejects.toMatchObject({ name: 'AbortError' });
    owner.abort();
    await rejection;
    release();
    await expect(survivor).resolves.toContain('foreignObject');
  });

  it('starts a fresh template after every previous consumer cancels', async () => {
    let release!: () => void;
    const gate = new Promise<void>(resolve => { release = resolve; });
    const families = mockFontLoading(() => gate);
    const { renderMermaidTemplate } = await import('@/features/chat/utils/mermaidRenderer');
    const source = 'flowchart LR\nA["$$x^5$$"] --> B';
    const owner = new AbortController();
    const abandoned = renderMermaidTemplate(source, owner.signal);
    await vi.waitFor(() => expect(families.length).toBeGreaterThan(0));
    const rejection = expect(abandoned).rejects.toMatchObject({ name: 'AbortError' });
    owner.abort();
    await rejection;
    const replacement = renderMermaidTemplate(source, new AbortController().signal);
    release();
    await expect(replacement).resolves.toContain('foreignObject');
  });
});
