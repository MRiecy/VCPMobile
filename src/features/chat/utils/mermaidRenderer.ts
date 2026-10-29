/** Shared message-domain renderer: one Pele module load, bounded reusable templates. */
const MAX_CACHE_BYTES = 4 * 1024 * 1024;
const MAX_ENTRY_BYTES = 512 * 1024;
const cache = new Map<string, { svg: string; bytes: number }>();
let cacheBytes = 0;
let serial = 0;
type PeleModule = typeof import('pele');
type MathModule = typeof import('./mermaidMath');
let peleModule: PeleModule | null = null;
let mathModule: MathModule | null = null;
let loadingPele: Promise<PeleModule> | null = null;
type Rendering = { controller: AbortController; consumers: number; promise: Promise<string> };
const rendering = new Map<string, Rendering>();

function abortError() { return new DOMException('图表渲染已取消', 'AbortError'); }

function remember(key: string, svg: string) {
  // UTF-16 storage estimate includes both source key and rendered SVG.
  const bytes = (key.length + svg.length) * 2;
  if (bytes > MAX_ENTRY_BYTES) return;
  if (cache.has(key)) {
    cacheBytes -= cache.get(key)!.bytes;
    cache.delete(key);
  }
  while (cacheBytes + bytes > MAX_CACHE_BYTES && cache.size) {
    const oldest = cache.keys().next().value!;
    cacheBytes -= cache.get(oldest)!.bytes;
    cache.delete(oldest);
  }
  cache.set(key, { svg, bytes });
  cacheBytes += bytes;
}

function loadPele(): Promise<PeleModule> {
  if (peleModule) return Promise.resolve(peleModule);
  if (!loadingPele) {
    loadingPele = Promise.all([import('pele'), import('./mermaidMath')])
      .then(([mod, math]) => {
        peleModule = mod;
        mathModule = math;
        return mod;
      })
      .finally(() => {
        loadingPele = null;
      });
  }
  return loadingPele;
}

export async function renderMermaidTemplate(source: string, signal: AbortSignal): Promise<string> {
  if (signal.aborted) throw abortError();
  const hit = cache.get(source);
  if (hit) {
    cache.delete(source);
    cache.set(source, hit);
    return hit.svg;
  }
  const pele = peleModule ?? await new Promise<PeleModule>((resolve, reject) => {
    const onAbort = () => reject(abortError());
    signal.addEventListener('abort', onAbort, { once: true });
    loadPele().then(
      mod => {
        signal.removeEventListener('abort', onAbort);
        resolve(mod);
      },
      err => {
        signal.removeEventListener('abort', onAbort);
        reject(err);
      },
    );
  });
  if (signal.aborted) throw abortError();
  const cachedAfterLoad = cache.get(source);
  if (cachedAfterLoad) {
    cache.delete(source);
    cache.set(source, cachedAfterLoad);
    return cachedAfterLoad.svg;
  }
  let job = rendering.get(source);
  if (!job) {
    const controller = new AbortController();
    const idPrefix = `vcp-mermaid-template-${++serial}`;
    const created: Rendering = { controller, consumers: 0, promise: Promise.resolve('') };
    created.promise = Promise.resolve().then(async () => {
      const svg = await mathModule!.renderWithMermaidMath(
        math => pele.render(source, { idPrefix, math }).svg,
        controller.signal,
      );
      if (controller.signal.aborted) throw abortError();
      remember(source, svg);
      return svg;
    }).finally(() => { if (rendering.get(source) === created) rendering.delete(source); });
    rendering.set(source, created);
    job = created;
  }
  const currentJob = job;
  currentJob.consumers++;
  return new Promise<string>((resolve, reject) => {
    let finished = false;
    const finish = () => {
      if (finished) return false;
      finished = true;
      signal.removeEventListener('abort', onAbort);
      currentJob.consumers--;
      return true;
    };
    const onAbort = () => {
      if (!finish()) return;
      if (!currentJob.consumers) {
        currentJob.controller.abort();
        if (rendering.get(source) === currentJob) rendering.delete(source);
      }
      reject(abortError());
    };
    signal.addEventListener('abort', onAbort, { once: true });
    currentJob.promise.then(
      svg => { if (finish()) resolve(svg); },
      error => { if (finish()) reject(error); },
    );
  });
}

function escapeRegExp(value: string) { return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'); }

/** Each DOM consumer, including the full-screen viewer, owns its SVG identifiers. */
export function instantiateMermaidSvg(template: string): string {
  const doc = new DOMParser().parseFromString(template, 'image/svg+xml');
  const root = doc.documentElement;
  if (root.localName !== 'svg' || doc.querySelector('parsererror')) throw new Error('图表 SVG 格式无效');
  const prefix = `vcp-diagram-${++serial}-`;
  const ids = new Map<string, string>();
  for (const element of [root, ...Array.from(root.querySelectorAll('[id]'))]) {
    const id = element.getAttribute('id');
    if (id) ids.set(id, `${prefix}${ids.size}`);
  }
  const rewriteIds = (text: string) => {
    for (const [id, replacement] of ids) {
      text = text.replace(new RegExp(`#${escapeRegExp(id)}(?=[\\s)"'\\]{}:;,.>+~]|$)`, 'g'), `#${replacement}`);
    }
    return text;
  };
  const rewrite = (text: string) => text.split(/(url\([^)]*\))/gi).map(part => {
    if (!/^url\(/i.test(part)) return rewriteIds(part);
    return part.replace(/^(url\(\s*["']?)#([^\s"')]+)(["']?\s*\))$/i, (value, start, id, end) => ids.has(id) ? `${start}#${ids.get(id)}${end}` : value);
  }).join('');
  for (const element of [root, ...Array.from(root.querySelectorAll('*'))]) {
    for (const attr of Array.from(element.attributes)) {
      if (attr.name === 'id') element.setAttribute('id', ids.get(attr.value)!);
      else if (attr.localName === 'href') {
        const replacement = attr.value.startsWith('#') ? ids.get(attr.value.slice(1)) : undefined;
        if (replacement) element.setAttributeNS(attr.namespaceURI, attr.name, `#${replacement}`);
      }
      else if (attr.name === 'aria-labelledby' || attr.name === 'aria-describedby') {
        element.setAttribute(attr.name, attr.value.split(/\s+/).map(id => ids.get(id) || id).join(' '));
      } else {
        let value = rewrite(attr.value);
        if (attr.name === 'begin' || attr.name === 'end') {
          value = value.replace(/(^|;)([\w-]+)\./g, (_, separator, id) => `${separator}${ids.get(id) || id}.`);
        }
        if (value !== attr.value) element.setAttributeNS(attr.namespaceURI, attr.name, value);
      }
    }
    if (element.localName === 'style') element.textContent = rewrite(element.textContent || '');
  }
  return new XMLSerializer().serializeToString(root);
}
