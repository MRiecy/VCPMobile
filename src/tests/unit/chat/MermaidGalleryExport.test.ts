import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { saveImageToGallery } from 'tauri-plugin-vcp-mobile';
import MermaidFullScreenViewer from '@/features/chat/blocks/MermaidFullScreenViewer.vue';

const { notify } = vi.hoisted(() => ({ notify: vi.fn() }));
vi.mock('@/core/stores/notification', () => ({ useNotificationStore: () => ({ addNotification: notify }) }));
vi.mock('@/core/stores/theme', () => ({ useThemeStore: () => ({ isDarkResolved: false }) }));
vi.mock('@/core/utils/platform', () => ({ isAndroid: true }));
vi.mock('@/core/composables/useModalHistory', () => ({
  useModalHistory: () => ({ registerModal: vi.fn(), unregisterModal: vi.fn() }),
}));

class PendingImage {
  static instances: PendingImage[] = [];
  src = '';
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  constructor(public width: number, public height: number) {
    PendingImage.instances.push(this);
  }
}

class PendingReader {
  static instances: PendingReader[] = [];
  result: string | null = null;
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onabort: (() => void) | null = null;
  readAsDataURL = vi.fn();
  abort = vi.fn(() => this.onabort?.());
  constructor() { PendingReader.instances.push(this); }
  complete() {
    this.result = 'data:image/png;base64,cG5n';
    this.onload?.();
  }
}

describe('Mermaid gallery export', () => {
  let wrapper: VueWrapper | undefined;
  let encodings: Array<{ canvas: HTMLCanvasElement; width: number; height: number; finish: BlobCallback }>;
  const drawImage = vi.fn();

  beforeEach(() => {
    encodings = [];
    PendingImage.instances = [];
    PendingReader.instances = [];
    vi.stubGlobal('Image', PendingImage);
    vi.stubGlobal('FileReader', PendingReader);
    vi.spyOn(console, 'error').mockImplementation(() => {});
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({
      fillRect: vi.fn(), drawImage,
    } as unknown as CanvasRenderingContext2D);
    vi.spyOn(HTMLCanvasElement.prototype, 'toBlob').mockImplementation(function (this: HTMLCanvasElement, finish) {
      encodings.push({ canvas: this, width: this.width, height: this.height, finish });
    });
  });

  afterEach(() => {
    wrapper?.unmount();
    wrapper = undefined;
    vi.unstubAllGlobals();
  });

  function open(width = 600, height = 400) {
    wrapper = mount(MermaidFullScreenViewer, {
      props: {
        visible: true,
        svgHtml: `<svg viewBox="0 0 ${width} ${height}" style="max-width: ${width}px"><text>中文图表</text></svg>`,
        sourceCode: 'graph TD; A-->B',
      },
      global: { stubs: { Teleport: true, Transition: true } },
    });
    // happy-dom 不实现 SVGAnimatedRect；保留真实 SVG markup 并补齐浏览器尺寸 API。
    Object.defineProperty(wrapper.get('svg').element, 'viewBox', {
      value: { baseVal: { x: 0, y: 0, width, height } }, configurable: true,
    });
    return wrapper.get('button[aria-busy]');
  }

  async function startEncoding() {
    await open().trigger('click');
    PendingImage.instances[0].onload?.();
    await flushPromises();
    expect(encodings).toHaveLength(1);
  }

  it.each([
    [600, 400, 1500, 1000],
    [8000, 4000, 2048, 1024],
    [8000, 8000, 2048, 2048],
  ])('bounds SVG decoding and PNG canvas for %sx%s while retaining small-image detail', async (width, height, outputWidth, outputHeight) => {
    const button = open(width, height);
    await Promise.all([button.trigger('click'), button.trigger('click')]);
    expect(PendingImage.instances).toHaveLength(1);
    expect(button.attributes('disabled')).toBeDefined();
    const image = PendingImage.instances[0];
    expect([image.width, image.height]).toEqual([outputWidth, outputHeight]);
    const source = decodeURIComponent(escape(atob(image.src.split(',')[1])));
    const svg = new DOMParser().parseFromString(source, 'image/svg+xml').documentElement;
    expect(svg.getAttribute('width')).toBe(String(outputWidth));
    expect(svg.getAttribute('height')).toBe(String(outputHeight));
    expect(svg.getAttribute('viewBox')).toBe(`0 0 ${width} ${height}`);
    expect(svg.textContent).toContain('中文图表');
    image.onload?.();
    await flushPromises();
    expect(encodings[0]).toMatchObject({ width: outputWidth, height: outputHeight });
    expect(drawImage).toHaveBeenCalledWith(image, 0, 0, outputWidth, outputHeight);
    encodings[0].finish(new Blob(['png'], { type: 'image/png' }));
    await flushPromises();
    expect(encodings[0].canvas.width).toBe(0);
    PendingReader.instances[0].complete();
    await flushPromises();
    expect(saveImageToGallery).toHaveBeenCalledOnce();
    expect(notify).toHaveBeenCalledWith(expect.objectContaining({ type: 'success' }));
    expect(button.attributes('disabled')).toBeUndefined();
  });

  it('rejects oversized PNG before Base64 allocation or native submission', async () => {
    await startEncoding();
    const png = new Blob(['png'], { type: 'image/png' });
    Object.defineProperty(png, 'size', { value: 16 * 1024 * 1024 + 1 });
    encodings[0].finish(png);
    await flushPromises();
    expect(PendingReader.instances).toHaveLength(0);
    expect(saveImageToGallery).not.toHaveBeenCalled();
    expect(notify).toHaveBeenCalledWith(expect.objectContaining({ type: 'error', message: expect.stringContaining('16 MiB') }));
    expect(encodings[0].canvas.width).toBe(0);
  });

  it('reports null PNG encoding and releases the busy state', async () => {
    await startEncoding();
    encodings[0].finish(null);
    await flushPromises();
    expect(saveImageToGallery).not.toHaveBeenCalled();
    expect(notify).toHaveBeenCalledWith(expect.objectContaining({ type: 'error' }));
    expect(wrapper!.get('button[aria-busy]').attributes('disabled')).toBeUndefined();
    expect(encodings[0].canvas.width).toBe(0);
  });

  it('reports a tainted-canvas encoding failure without submitting an image', async () => {
    vi.mocked(HTMLCanvasElement.prototype.toBlob).mockImplementationOnce(() => {
      throw new DOMException('The canvas has been tainted', 'SecurityError');
    });
    await open().trigger('click');
    PendingImage.instances[0].onload?.();
    await flushPromises();
    expect(saveImageToGallery).not.toHaveBeenCalled();
    expect(notify).toHaveBeenCalledWith(expect.objectContaining({ type: 'error', message: expect.stringContaining('SecurityError') }));
    expect(wrapper!.get('button[aria-busy]').attributes('disabled')).toBeUndefined();
    expect(PendingImage.instances[0].src).toBe('');
  });

  it('does not resume a stale image callback after the viewer closes', async () => {
    await open().trigger('click');
    const image = PendingImage.instances[0];
    const lateLoad = image.onload;
    await wrapper!.setProps({ visible: false });
    lateLoad?.();
    await flushPromises();
    expect(image.src).toBe('');
    expect(image.onload).toBeNull();
    expect(encodings).toHaveLength(0);
    expect(saveImageToGallery).not.toHaveBeenCalled();
    expect(notify).not.toHaveBeenCalled();
  });

  it('waits for uncancellable encoding after close and prevents overlapping work on reopen', async () => {
    await startEncoding();
    await wrapper!.setProps({ visible: false });
    await wrapper!.setProps({ visible: true });
    await wrapper!.get('button[aria-busy]').trigger('click');
    expect(PendingImage.instances).toHaveLength(1);
    encodings[0].finish(new Blob(['png'], { type: 'image/png' }));
    await flushPromises();
    expect(PendingReader.instances).toHaveLength(0);
    expect(saveImageToGallery).not.toHaveBeenCalled();
    expect(notify).not.toHaveBeenCalled();
    expect(wrapper!.get('button[aria-busy]').attributes('disabled')).toBeUndefined();
  });

  it('aborts pending Base64 conversion when the component unmounts', async () => {
    await startEncoding();
    encodings[0].finish(new Blob(['png'], { type: 'image/png' }));
    await flushPromises();
    const reader = PendingReader.instances[0];
    wrapper!.unmount();
    wrapper = undefined;
    reader.complete();
    await flushPromises();
    expect(reader.abort).toHaveBeenCalledOnce();
    expect(saveImageToGallery).not.toHaveBeenCalled();
    expect(notify).not.toHaveBeenCalled();
  });

  it('reports actual native completion even after the preview has closed', async () => {
    let finishSave!: (value: Awaited<ReturnType<typeof saveImageToGallery>>) => void;
    vi.mocked(saveImageToGallery).mockImplementationOnce(() => new Promise((resolve) => { finishSave = resolve; }));
    await startEncoding();
    encodings[0].finish(new Blob(['png'], { type: 'image/png' }));
    await flushPromises();
    PendingReader.instances[0].complete();
    await flushPromises();
    await wrapper!.setProps({ visible: false });
    expect(notify).not.toHaveBeenCalled();
    finishSave({ uri: 'content://gallery/image', displayName: 'mermaid.png', mimeType: 'image/png', size: 3 });
    await flushPromises();
    expect(notify).toHaveBeenCalledWith(expect.objectContaining({ type: 'success' }));
  });
});
