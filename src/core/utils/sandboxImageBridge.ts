/** This script executes inside the opaque document; its Blob URLs never escape it. */
export function sandboxImageBridge(nonce: string): string {
  return `(() => {
    const nonce = ${JSON.stringify(nonce)};
    const maxBytes = 8 * 1024 * 1024;
    let busy = false;
    let disposed = false;
    let controller = null;
    let currentRequestId = '';
    window.addEventListener('message', event => {
      if (event.source === window.parent && event.data?.source === 'vcp-mobile' && event.data?.type === 'rendered-image-cancel' && event.data?.nonce === nonce && event.data?.requestId === currentRequestId) controller?.abort();
    });
    window.addEventListener('pagehide', () => { disposed = true; controller?.abort(); });
    async function readBlob(src) {
      if (src.startsWith('data:') && src.length > maxBytes * 4 / 3 + 1024) throw new Error('图片数据超过 8 MiB');
      const response = await fetch(src, { signal: controller.signal });
      if (!response.ok) throw new Error('无法读取当前预览图片');
      if (Number(response.headers.get('content-length')) > maxBytes) {
        await response.body?.cancel();
        throw new Error('图片数据超过 8 MiB');
      }
      const reader = response.body?.getReader();
      if (!reader) throw new Error('图片来源不支持有界读取');
      const chunks = [];
      let bytes = 0;
      try {
        while (true) {
          const result = await reader.read();
          if (result.done) break;
          bytes += result.value.byteLength;
          if (bytes > maxBytes) throw new Error('图片数据超过 8 MiB');
          chunks.push(result.value);
        }
        return new Blob(chunks, { type: response.headers.get('content-type') || 'image/png' });
      } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
    }
    async function canvasBlob(source) {
      const width = source instanceof HTMLImageElement ? source.naturalWidth : source.width;
      const height = source instanceof HTMLImageElement ? source.naturalHeight : source.height;
      if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) throw new Error('画布尺寸无效');
      const scale = Math.min(1, 2048 / Math.max(width, height), Math.sqrt(4194304 / width / height));
      const canvas = document.createElement('canvas');
      canvas.width = Math.max(1, Math.floor(width * scale));
      canvas.height = Math.max(1, Math.floor(height * scale));
      try {
        const context = canvas.getContext('2d');
        if (!context) throw new Error('无法创建导出画布');
        context.drawImage(source, 0, 0, canvas.width, canvas.height);
        return await new Promise((resolve, reject) => canvas.toBlob(blob => blob ? resolve(blob) : reject(new Error('图片编码失败')), 'image/png'));
      } finally { canvas.width = 0; canvas.height = 0; }
    }
    function acquireExportSlot(requestId) {
      return new Promise((resolve, reject) => {
        const cleanup = () => { window.removeEventListener('message', granted); controller.signal.removeEventListener('abort', aborted); };
        const aborted = () => { cleanup(); reject(new Error('图片操作已取消或超时')); };
        const granted = event => {
          if (event.source !== window.parent || event.data?.source !== 'vcp-mobile' || event.data?.type !== 'rendered-image-grant' || event.data?.nonce !== nonce || event.data?.requestId !== requestId) return;
          cleanup(); resolve();
        };
        window.addEventListener('message', granted);
        controller.signal.addEventListener('abort', aborted, { once: true });
        window.parent.postMessage({ source: 'vcp-mobile', type: 'rendered-image-request', nonce, requestId }, '*');
      });
    }
    document.addEventListener('click', async event => {
      const target = event.target instanceof Element ? event.target : null;
      if (!target) return;
      const link = target.closest('a');
      if (link) {
        const href = link.getAttribute('href');
        if (!href || href === '#' || href.startsWith('javascript:')) event.preventDefault();
      }
      const image = target.closest('img, canvas');
      if (!image) return;
      event.preventDefault();
      if (busy) return;
      busy = true;
      controller = new AbortController();
      const timeout = setTimeout(() => controller?.abort(), 30000);
      const requestId = crypto.randomUUID ? crypto.randomUUID() : String(Date.now());
      currentRequestId = requestId;
      try {
        await acquireExportSlot(requestId);
        const details = { alt: image.alt || '', title: image.title || '' };
        let blob;
        if (image instanceof HTMLCanvasElement) blob = await canvasBlob(image);
        else {
          const src = image.currentSrc || image.src;
          if (!/^(blob:|data:)/i.test(src)) {
            if (!disposed) window.parent.postMessage({ source: 'vcp-mobile', type: 'rendered-image-click', nonce, requestId, image: { ...details, src } }, '*');
            return;
          }
          blob = await readBlob(src);
          if (blob.type.split(';')[0] === 'image/svg+xml') blob = await canvasBlob(image);
        }
        if (!blob.size || blob.size > maxBytes) throw new Error('图片数据超过 8 MiB');
        const bytes = await blob.arrayBuffer();
        if (controller.signal.aborted) throw new Error('图片操作已取消或超时');
        if (!disposed && !controller.signal.aborted) window.parent.postMessage({ source: 'vcp-mobile', type: 'rendered-image-click', nonce, requestId, image: { ...details, bytes, mime: blob.type, fileName: image instanceof HTMLCanvasElement ? 'vcp-canvas.png' : '' } }, '*', [bytes]);
      } catch (error) {
        if (!disposed) window.parent.postMessage({ source: 'vcp-mobile', type: 'rendered-image-error', nonce, requestId, error: String(error) }, '*');
      } finally { clearTimeout(timeout); controller = null; currentRequestId = ''; busy = false; }
    }, true);
  })();`;
}
