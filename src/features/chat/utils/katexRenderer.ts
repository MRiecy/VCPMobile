import katex from 'katex';

const MAX_ENTRIES = 256;
// Budget the retained UTF-16 source keys and HTML strings; engine/object overhead is separate.
const MAX_STRING_BYTES = 2 * 1024 * 1024;
const htmlCache = new Map<string, string>();
let cachedStringBytes = 0;

/** Loaded on demand by MessageRenderer; all options except displayMode are fixed here. */
export function renderKatexHtml(latex: string, displayMode: boolean): string {
  const key = `${displayMode ? 'display' : 'inline'}:${latex}`;
  const cached = htmlCache.get(key);
  if (cached !== undefined) {
    htmlCache.delete(key);
    htmlCache.set(key, cached);
    return cached;
  }

  const html = katex.renderToString(latex, {
    throwOnError: false,
    strict: false,
    displayMode,
    output: 'htmlAndMathml',
    trust: false,
  });
  const stringBytes = (key.length + html.length) * 2;
  // Large formulas still render normally, but must not evict the cache for a one-off result.
  if (stringBytes <= MAX_STRING_BYTES) {
    for (const [oldKey, oldHtml] of htmlCache) {
      if (htmlCache.size < MAX_ENTRIES && cachedStringBytes + stringBytes <= MAX_STRING_BYTES) break;
      htmlCache.delete(oldKey);
      cachedStringBytes -= (oldKey.length + oldHtml.length) * 2;
    }
    htmlCache.set(key, html);
    cachedStringBytes += stringBytes;
  }
  return html;
}
