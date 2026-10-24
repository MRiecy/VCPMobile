// Output budgets cover application-owned buffers, not allocations made by user HTML.
export const IMAGE_EXPORT_MAX_SIDE = 2048;
export const IMAGE_EXPORT_MAX_PIXELS = 2048 * 2048;
export const IMAGE_EXPORT_MAX_BYTES = 8 * 1024 * 1024;
let exportTail: Promise<unknown> = Promise.resolve();

const PELE_THEME_VARS = [
  '--pele-bg',
  '--pele-fg',
  '--pele-muted',
  '--pele-line',
  '--pele-surface',
  '--pele-surface-alt',
  '--pele-border',
  '--pele-accent',
  '--pele-critical',
  '--pele-error',
  '--pele-radius',
  '--pele-font',
  '--pele-font-mono',
  '--pele-series-1',
  '--pele-series-2',
  '--pele-series-3',
  '--pele-series-4',
  '--pele-series-5',
  '--pele-series-6',
  '--pele-series-7',
  '--pele-series-8',
] as const;

export function cloneSvgWithThemeVariables(source: SVGSVGElement): SVGSVGElement {
  const svg = source.cloneNode(true) as SVGSVGElement;
  svg.setAttribute('xmlns', 'http://www.w3.org/2000/svg');
  const view = source.ownerDocument?.defaultView;
  if (view && source.isConnected) {
    const computed = view.getComputedStyle(source);
    for (const variable of PELE_THEME_VARS) {
      const value = computed.getPropertyValue(variable).trim();
      if (value) svg.style.setProperty(variable, value);
    }
  }
  return svg;
}

export function serializeSvgWithTheme(source: SVGSVGElement): string {
  return new XMLSerializer().serializeToString(cloneSvgWithThemeVariables(source));
}

export function ensureExportActive(signal?: AbortSignal) {
  if (signal?.aborted) throw new DOMException('图片操作已取消', 'AbortError');
}

export function boundedImageSize(width: number, height: number, preferredScale = 1) {
  if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) {
    throw new Error('图片尺寸无效');
  }
  const scale = Math.min(preferredScale, IMAGE_EXPORT_MAX_SIDE / Math.max(width, height), Math.sqrt(IMAGE_EXPORT_MAX_PIXELS / width / height));
  return { width: Math.max(1, Math.floor(width * scale)), height: Math.max(1, Math.floor(height * scale)) };
}

export function withImageExportQueue<T>(task: () => Promise<T>, signal?: AbortSignal): Promise<T> {
  const result = exportTail.then(() => { ensureExportActive(signal); return task(); });
  exportTail = result.catch(() => {});
  return result;
}

export async function encodeCanvas(canvas: HTMLCanvasElement, signal?: AbortSignal): Promise<Blob> {
  ensureExportActive(signal);
  // toBlob cannot be cancelled: retain the queue slot until its callback releases buffers.
  const blob = await new Promise<Blob>((resolve, reject) => {
    canvas.toBlob(value => value ? resolve(value) : reject(new Error('图片 PNG 编码失败')), 'image/png');
  });
  ensureExportActive(signal);
  if (blob.size > IMAGE_EXPORT_MAX_BYTES) throw new Error('图片编码超过 8 MiB，请缩小图形后重试');
  return blob;
}

export function exportCanvas(source: HTMLCanvasElement, signal?: AbortSignal): Promise<Blob> {
  return withImageExportQueue(async () => {
    const size = boundedImageSize(source.width, source.height);
    const canvas = document.createElement('canvas');
    canvas.width = size.width;
    canvas.height = size.height;
    try {
      const context = canvas.getContext('2d');
      if (!context) throw new Error('无法创建图片画布');
      context.drawImage(source, 0, 0, size.width, size.height);
      return await encodeCanvas(canvas, signal);
    } finally { canvas.width = 0; canvas.height = 0; }
  }, signal);
}

export function exportSvg(source: SVGSVGElement, signal?: AbortSignal, options: { preferredScale?: number; background?: string } = {}): Promise<Blob> {
  return withImageExportQueue(async () => {
    const viewBox = source.viewBox?.baseVal;
    const bounds = viewBox?.width && viewBox?.height ? viewBox : source.getBBox?.();
    const width = bounds?.width || source.clientWidth || Number(source.getAttribute('width')) || 600;
    const height = bounds?.height || source.clientHeight || Number(source.getAttribute('height')) || 400;
    const size = boundedImageSize(width, height, options.preferredScale ?? 1);
    const svg = cloneSvgWithThemeVariables(source);
    if (!viewBox?.width || !viewBox?.height) svg.setAttribute('viewBox', `${bounds?.x || 0} ${bounds?.y || 0} ${width} ${height}`);
    svg.setAttribute('width', String(size.width));
    svg.setAttribute('height', String(size.height));
    svg.style.width = `${size.width}px`;
    svg.style.height = `${size.height}px`;
    svg.style.maxWidth = 'none';
    svg.style.maxHeight = 'none';
    const markup = new XMLSerializer().serializeToString(svg);
    if (new Blob([markup]).size > IMAGE_EXPORT_MAX_BYTES) throw new Error('SVG 图片超过 8 MiB');
    const image = new Image(size.width, size.height);
    const canvas = document.createElement('canvas');
    try {
      await new Promise<void>((resolve, reject) => {
        const timer = setTimeout(() => { clear(); image.src = ''; reject(new Error('SVG 图片加载超时')); }, 30000);
        const clear = () => { clearTimeout(timer); image.onload = null; image.onerror = null; signal?.removeEventListener('abort', abort); };
        const abort = () => { clear(); image.src = ''; reject(new DOMException('图片操作已取消', 'AbortError')); };
        image.onload = () => { clear(); resolve(); };
        image.onerror = () => { clear(); reject(new Error('SVG 图片加载失败')); };
        signal?.addEventListener('abort', abort, { once: true });
        // Preserve the Android WebView inline-SVG loading path used by Mermaid.
        image.src = `data:image/svg+xml;base64,${window.btoa(unescape(encodeURIComponent(markup)))}`;
      });
      ensureExportActive(signal);
      canvas.width = size.width;
      canvas.height = size.height;
      const context = canvas.getContext('2d');
      if (!context) throw new Error('无法创建图片画布');
      if (options.background) {
        context.fillStyle = options.background;
        context.fillRect(0, 0, size.width, size.height);
      }
      context.drawImage(image, 0, 0, size.width, size.height);
      return await encodeCanvas(canvas, signal);
    } finally { image.src = ''; canvas.width = 0; canvas.height = 0; }
  }, signal);
}

export async function readBoundedImageBlob(src: string, signal?: AbortSignal): Promise<Blob> {
  ensureExportActive(signal);
  // Bound encoded data before fetch/decoding allocates another copy.
  if (src.startsWith('data:') && src.length > IMAGE_EXPORT_MAX_BYTES * 4 / 3 + 1024) throw new Error('图片数据超过 8 MiB');
  const response = await fetch(src, { signal });
  if (!response.ok) throw new Error(`图片读取失败：${response.status}`);
  const type = response.headers.get('content-type')?.split(';')[0] || 'application/octet-stream';
  const length = Number(response.headers.get('content-length'));
  if (length > IMAGE_EXPORT_MAX_BYTES) { await response.body?.cancel(); throw new Error('图片数据超过 8 MiB'); }
  const reader = response.body?.getReader();
  if (!reader) throw new Error('图片来源不支持有界读取');
  const chunks: Uint8Array<ArrayBuffer>[] = [];
  let size = 0;
  try {
    while (true) {
      ensureExportActive(signal);
      const result = await reader.read();
      if (result.done) break;
      size += result.value.byteLength;
      if (size > IMAGE_EXPORT_MAX_BYTES) throw new Error('图片数据超过 8 MiB');
      chunks.push(result.value);
    }
    return new Blob(chunks, { type });
  } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
}
