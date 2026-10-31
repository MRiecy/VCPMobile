import { beforeEach, describe, expect, it, vi } from 'vitest';

beforeEach(() => {
  vi.resetModules();
  // happy-dom lacks compatMode; production index.html uses standards mode.
  Object.defineProperty(document, 'compatMode', { configurable: true, value: 'CSS1Compat' });
});

async function loadRenderer() {
  const { default: katex } = await import('katex');
  const { renderKatexHtml } = await import('@/features/chat/utils/katexRenderer');
  return { katex, renderKatexHtml };
}

// KaTeX's DOM and string builders differ in empty class attributes, redundant xmlns
// and CSS whitespace. Compare the resulting tree, namespaces and style declarations.
function describeDom(node: Node): unknown {
  if (!(node instanceof Element)) return { type: node.nodeType, text: node.textContent };
  const attributes = Array.from(node.attributes)
    .filter(attribute => !(attribute.name === 'class' && attribute.value === '')
      && !(attribute.name === 'xmlns' && attribute.value === node.namespaceURI))
    .map(attribute => {
      if (attribute.name !== 'style') return [attribute.name, attribute.value];
      const style = document.createElement('span').style;
      style.cssText = attribute.value;
      return [attribute.name, style.cssText];
    })
    .sort(([left], [right]) => left.localeCompare(right));
  return {
    name: node.localName,
    namespace: node.namespaceURI,
    attributes,
    children: Array.from(node.childNodes, describeDom),
  };
}

describe('cached message math', () => {
  it('preserves KaTeX markup for inline, display, invalid input and untrusted commands', async () => {
    const { katex, renderKatexHtml } = await loadRenderer();
    for (const latex of [
      String.raw`\frac{x^2}{\sqrt{y}} + \cancel{z}`,
      String.raw`\begin{matrix}a & b \\ c & d\end{matrix}`,
      String.raw`\frac{`,
      String.raw`\href{javascript:alert(1)}{unsafe}`,
      String.raw`\htmlClass{injected}{x}`,
    ]) {
      for (const displayMode of [false, true]) {
        const expected = document.createElement('div');
        katex.render(latex, expected, { displayMode, throwOnError: false, strict: false });
        // happy-dom cannot infer MathML namespaces from HTML. Parse the old DOM's
        // serialization as well so this checks markup equivalence, not browser layout.
        const reparsedExpected = document.createElement('div');
        reparsedExpected.innerHTML = expected.innerHTML;
        const actual = document.createElement('div');
        actual.innerHTML = renderKatexHtml(latex, displayMode);
        expect(describeDom(actual)).toEqual(describeDom(reparsedExpected));
        expect(actual.querySelector('[href^="javascript:"], .injected')).toBeNull();
      }
    }
  });

  it('reuses HTML while keeping display mode distinct and retaining error markup', async () => {
    const { katex, renderKatexHtml } = await loadRenderer();
    const render = vi.spyOn(katex, 'renderToString');
    const inline = renderKatexHtml('x^2', false);
    expect(renderKatexHtml('x^2', false)).toBe(inline);
    const display = renderKatexHtml('x^2', true);
    expect(renderKatexHtml('x^2', true)).toBe(display);
    expect(display).not.toBe(inline);
    const error = renderKatexHtml(String.raw`\frac{`, false);
    expect(error).toContain('katex-error');
    expect(renderKatexHtml(String.raw`\frac{`, false)).toBe(error);
    expect(render).toHaveBeenCalledTimes(3);
  });

  it('evicts the least recently used formula when the entry limit is reached', async () => {
    const { katex, renderKatexHtml } = await loadRenderer();
    const render = vi.spyOn(katex, 'renderToString').mockImplementation(latex => `<span>${latex}</span>`);
    for (let i = 0; i < 256; i++) renderKatexHtml(`x_${i}`, false);
    renderKatexHtml('x_0', false); // Keep the oldest entry alive with a read.
    renderKatexHtml('x_256', false);
    render.mockClear();
    renderKatexHtml('x_0', false);
    renderKatexHtml('x_1', false);
    expect(render).toHaveBeenCalledExactlyOnceWith('x_1', expect.any(Object));
  });

  it('bounds retained source and output strings and bypasses oversized entries', async () => {
    const { katex, renderKatexHtml } = await loadRenderer();
    const output = 'x'.repeat(600_000);
    const render = vi.spyOn(katex, 'renderToString').mockReturnValue(output);
    expect(renderKatexHtml('first', false)).toBe(output);
    renderKatexHtml('second', false);
    render.mockClear();
    renderKatexHtml('second', false);
    renderKatexHtml('first', false);
    expect(render).toHaveBeenCalledExactlyOnceWith('first', expect.any(Object));

    render.mockReturnValue('oversized-source');
    const largeSource = 'y'.repeat(1_048_576);
    renderKatexHtml(largeSource, false);
    renderKatexHtml(largeSource, false);
    expect(render).toHaveBeenCalledTimes(3);
    render.mockClear();
    expect(renderKatexHtml('first', false)).toBe(output);
    expect(render).not.toHaveBeenCalled();

    render.mockReturnValue('z'.repeat(1_048_576));
    renderKatexHtml('oversized-output', false);
    renderKatexHtml('oversized-output', false);
    expect(render).toHaveBeenCalledTimes(2);
    render.mockClear();
    renderKatexHtml('first', false);
    expect(render).not.toHaveBeenCalled();
  });

  it('does not cache unexpected renderer failures', async () => {
    const { katex, renderKatexHtml } = await loadRenderer();
    const render = vi.spyOn(katex, 'renderToString')
      .mockImplementationOnce(() => { throw new Error('render failed'); })
      .mockReturnValue('<span>recovered</span>');
    expect(() => renderKatexHtml('x', false)).toThrow('render failed');
    expect(renderKatexHtml('x', false)).toBe('<span>recovered</span>');
    expect(renderKatexHtml('x', false)).toBe('<span>recovered</span>');
    expect(render).toHaveBeenCalledTimes(2);
  });
});
