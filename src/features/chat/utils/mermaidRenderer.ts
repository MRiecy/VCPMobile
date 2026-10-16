/** Shared message-domain renderer: one Mermaid execution, bounded reusable templates. */
const MAX_CACHE_BYTES = 4 * 1024 * 1024;
const MAX_ENTRY_BYTES = 512 * 1024;
const OPTIONS = { startOnLoad: false, securityLevel: 'strict' as const };
const cache = new Map<string, { svg: string; bytes: number }>();
let cacheBytes = 0;
let serial = 0;
let running = false;

interface Subscriber {
  resolve: (svg: string) => void;
  reject: (error: unknown) => void;
  detach: () => void;
}
interface Task {
  key: string;
  source: string;
  theme: 'dark' | 'default';
  subscribers: Set<Subscriber>;
  started: boolean;
}
const pending = new Map<string, Task>();
const queue: Task[] = [];

function abortError() { return new DOMException('图表渲染已取消', 'AbortError'); }

function remember(key: string, svg: string) {
  // UTF-16 storage estimate includes both source/options key and rendered SVG.
  const bytes = (key.length + svg.length) * 2;
  if (bytes > MAX_ENTRY_BYTES) return;
  while (cacheBytes + bytes > MAX_CACHE_BYTES && cache.size) {
    const oldest = cache.keys().next().value!;
    cacheBytes -= cache.get(oldest)!.bytes;
    cache.delete(oldest);
  }
  cache.set(key, { svg, bytes });
  cacheBytes += bytes;
}

async function drain() {
  if (running) return;
  running = true;
  try {
    while (queue.length) {
      const task = queue.shift()!;
      task.started = true;
      if (!task.subscribers.size) { pending.delete(task.key); continue; }
      try {
        const mermaid = (await import('mermaid')).default;
        if (!task.subscribers.size) continue;
        mermaid.initialize({ ...OPTIONS, theme: task.theme });
        const { svg } = await mermaid.render(`vcp-mermaid-template-${++serial}`, task.source);
        // A completed library operation has no owner after the last consumer leaves.
        if (task.subscribers.size) remember(task.key, svg);
        for (const subscriber of task.subscribers) { subscriber.detach(); subscriber.resolve(svg); }
      } catch (error) {
        for (const subscriber of task.subscribers) { subscriber.detach(); subscriber.reject(error); }
      } finally {
        pending.delete(task.key);
        task.subscribers.clear();
      }
    }
  } finally { running = false; }
}

export function renderMermaidTemplate(source: string, dark: boolean, signal: AbortSignal): Promise<string> {
  if (signal.aborted) return Promise.reject(abortError());
  const theme = dark ? 'dark' : 'default';
  const key = JSON.stringify([source, theme, OPTIONS]);
  const hit = cache.get(key);
  if (hit) {
    cache.delete(key);
    cache.set(key, hit);
    return Promise.resolve(hit.svg);
  }
  let task = pending.get(key);
  if (!task) {
    task = { key, source, theme, subscribers: new Set(), started: false };
    pending.set(key, task);
    queue.push(task);
  }
  const owner = task;
  const result = new Promise<string>((resolve, reject) => {
    const subscriber: Subscriber = { resolve, reject, detach: () => signal.removeEventListener('abort', abort) };
    const abort = () => {
      owner.subscribers.delete(subscriber);
      subscriber.detach();
      if (!owner.started && !owner.subscribers.size) {
        const position = queue.indexOf(owner);
        if (position !== -1) queue.splice(position, 1);
        pending.delete(owner.key);
      }
      reject(abortError());
    };
    owner.subscribers.add(subscriber);
    signal.addEventListener('abort', abort, { once: true });
  });
  void drain();
  return result;
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
