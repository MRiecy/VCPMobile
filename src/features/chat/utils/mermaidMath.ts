import katex from 'katex';
import katexCss from 'katex/dist/katex.min.css?raw';
import type { MathMarkup, MathResolver } from 'pele';

const MAX_MATH_CACHE_BYTES = 256 * 1024;
const MAX_MATH_ENTRY_BYTES = 64 * 1024;
const fontAssets = import.meta.glob<string>('/node_modules/katex/dist/fonts/*.woff2', {
  query: '?inline', import: 'default',
});
type FontSource = { family: string; style: string; weight: string; asset: string };
const fontSources = new Map<string, FontSource>();
// Private font families keep diagram exports independent of the page's KaTeX font faces.
const mathCss = katexCss.replace(/KaTeX_/g, 'VcpDiagramKaTeX_').replace(/@font-face\{([^}]*)\}/g, (_rule: string, body: string) => {
  const family = /font-family:([^;]+)/.exec(body)?.[1]?.replace(/^["']|["']$/g, '');
  const style = /font-style:([^;]+)/.exec(body)?.[1];
  const weight = /font-weight:([^;]+)/.exec(body)?.[1];
  const asset = /url\(fonts\/([^)]*\.woff2)\)/.exec(body)?.[1]?.replace('VcpDiagramKaTeX_', 'KaTeX_');
  if (family && style && weight && asset) fontSources.set(`${family}:${style}:${weight}`, { family, style, weight, asset });
  return '';
}).replace(/(^|})([^{}]+)\{/g, (_rule, boundary: string, selectors: string) => {
  // The pinned KaTeX stylesheet contains ordinary rules after @font-face removal.
  // Inline SVG styles participate in the page cascade, so every rule needs a local scope.
  const scoped = selectors.split(',').map(selector => selector.trim() === 'body'
    ? '.vcp-diagram-math'
    : `.vcp-diagram-math ${selector.trim()}`);
  return `${boundary}${scoped.join(',')}{`;
});
const loadedFonts = new Map<string, string>();
const loadingFonts = new Map<string, Promise<void>>();
type CachedMath = { markup: MathMarkup; fonts: string[]; bytes: number };
const mathCache = new Map<string, CachedMath>();
let mathCacheBytes = 0;

function rememberMath(key: string, value: Omit<CachedMath, 'bytes'>) {
  const bytes = (key.length + ('html' in value.markup ? value.markup.html.length : value.markup.svg.length) + value.fonts.join('').length) * 2;
  if (bytes > MAX_MATH_ENTRY_BYTES) return;
  const existing = mathCache.get(key);
  if (existing) { mathCacheBytes -= existing.bytes; mathCache.delete(key); }
  while (mathCacheBytes + bytes > MAX_MATH_CACHE_BYTES && mathCache.size) {
    const oldest = mathCache.keys().next().value!;
    mathCacheBytes -= mathCache.get(oldest)!.bytes;
    mathCache.delete(oldest);
  }
  mathCache.set(key, { ...value, bytes });
  mathCacheBytes += bytes;
}

function loadFont(key: string): Promise<void> {
  if (loadedFonts.has(key)) return Promise.resolve();
  const pending = loadingFonts.get(key);
  if (pending) return pending;
  const source = fontSources.get(key)!;
  const promise = (async () => {
    const asset = fontAssets[`/node_modules/katex/dist/fonts/${source.asset}`];
    if (!asset) throw new Error(`缺少图表公式字体：${source.asset}`);
    const url = await asset();
    const face = new FontFace(source.family, `url("${url}")`, { style: source.style, weight: source.weight });
    await face.load();
    document.fonts.add(face);
    loadedFonts.set(key, `@font-face{font-family:${source.family};font-style:${source.style};font-weight:${source.weight};src:url("${url}") format("woff2")}`);
  })().finally(() => { loadingFonts.delete(key); });
  loadingFonts.set(key, promise);
  return promise;
}

function waitForFonts(keys: string[], signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const abort = () => { signal.removeEventListener('abort', abort); reject(new DOMException('图表渲染已取消', 'AbortError')); };
    if (signal.aborted) { abort(); return; }
    signal.addEventListener('abort', abort, { once: true });
    // Font assets and loaded faces are finite, shared module resources. Cancelling a caller
    // releases its wait immediately without aborting another diagram's font preparation.
    Promise.all(keys.map(loadFont)).then(
      () => { signal.removeEventListener('abort', abort); resolve(); },
      error => { signal.removeEventListener('abort', abort); reject(error); },
    );
  });
}

/** Keep KaTeX's HTML layout (including SVG accents) and accessibility MathML together. */
export async function renderWithMermaidMath(render: (math: MathResolver) => string, signal: AbortSignal): Promise<string> {
  const usedFonts = new Set<string>();
  const run = () => {
    let host: HTMLDivElement | undefined;
    let measure: HTMLSpanElement | undefined;
    const resolveMath: MathResolver = (tex, size) => {
      const key = `${size}\n${tex}`;
      const cached = mathCache.get(key);
      if (cached) {
        mathCache.delete(key);
        mathCache.set(key, cached);
        cached.fonts.forEach(font => usedFonts.add(font));
        return cached.markup;
      }
      if (!measure) {
        host = document.createElement('div');
        host.setAttribute('aria-hidden', 'true');
        host.style.cssText = 'position:fixed;left:-100000px;top:0;visibility:hidden;pointer-events:none;contain:layout style paint';
        const shadow = host.attachShadow({ mode: 'closed' });
        const style = document.createElement('style');
        style.textContent = mathCss;
        measure = document.createElement('span');
        measure.className = 'vcp-diagram-math';
        measure.style.cssText = 'display:inline-block;white-space:nowrap;line-height:normal;font-family:sans-serif';
        shadow.append(style, measure);
        document.body.append(host);
      }
      measure.innerHTML = katex.renderToString(tex, { output: 'htmlAndMathml', displayMode: false, throwOnError: false, strict: false, trust: false });
      measure.style.fontSize = `${size}px`;
      const formula = measure.querySelector<HTMLElement>('.katex') ?? measure.firstElementChild as HTMLElement;
      formula.style.fontSize = `${size}px`;
      const fonts = new Set<string>();
      for (const element of [formula, ...Array.from(formula.querySelectorAll('*'))]) {
        const style = getComputedStyle(element);
        const family = style.fontFamily.split(',')[0].trim().replace(/^["']|["']$/g, '');
        const fontKey = `${family}:${style.fontStyle}:${style.fontWeight}`;
        if (fontSources.has(fontKey)) { fonts.add(fontKey); usedFonts.add(fontKey); }
      }
      const bounds = measure.getBoundingClientRect();
      const markup = { html: measure.outerHTML, width: bounds.width, height: bounds.height };
      // Never retain fallback-font geometry. The first pass is discarded while fonts load.
      if ([...fonts].every(font => loadedFonts.has(font))) rememberMath(key, { markup, fonts: [...fonts] });
      return markup;
    };
    try { return render(resolveMath); }
    finally { host?.remove(); }
  };
  if (signal.aborted) throw new DOMException('图表渲染已取消', 'AbortError');
  let svg = run();
  const missing = [...usedFonts].filter(key => !loadedFonts.has(key));
  if (missing.length) {
    await waitForFonts(missing, signal);
    if (signal.aborted) throw new DOMException('图表渲染已取消', 'AbortError');
    svg = run();
  }
  if (!usedFonts.size) return svg;
  const style = document.createElementNS('http://www.w3.org/2000/svg', 'style');
  style.textContent = mathCss + [...usedFonts].map(key => loadedFonts.get(key)!).join('');
  // Every diagram owns one copy of its actual font subset; cloning and standalone image
  // decoding therefore preserve the same formula layout without fetching page CSS/fonts.
  return svg.replace(/<svg\b[^>]*>/, root => root + new XMLSerializer().serializeToString(style));
}
