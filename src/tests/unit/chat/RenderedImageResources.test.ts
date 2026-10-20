import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import RenderedImageViewer from '@/components/ui/RenderedImageViewer.vue';
import { closeRenderedImageViewer, openRenderedImageViewer } from '@/core/composables/useRenderedImageViewer';

const { invoke, save, notify } = vi.hoisted(() => ({ invoke: vi.fn(), save: vi.fn(), notify: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke, convertFileSrc: (path: string) => `asset://localhost/${path}` }));
vi.mock('tauri-plugin-vcp-mobile', () => ({ saveImageToGallery: save }));
vi.mock('@/core/stores/notification', () => ({ useNotificationStore: () => ({ addNotification: notify }) }));
vi.mock('@/core/composables/useModalHistory', () => ({ useModalHistory: () => ({ registerModal: vi.fn(), unregisterModal: vi.fn() }) }));

const prepared = { path: '/original.png', displayPath: '/display.png', mime: 'image/png', size: 100, width: 100, height: 100 };
let wrapper: VueWrapper;

beforeEach(() => {
  closeRenderedImageViewer();
  invoke.mockImplementation(async (command: string) => command === 'prepare_rendered_image' ? prepared : undefined);
  wrapper = mount(RenderedImageViewer, { global: { stubs: { Teleport: true, Transition: true } } });
});
afterEach(async () => { wrapper.unmount(); closeRenderedImageViewer(); await flushPromises(); });

describe('rendered-image file lifetimes', () => {
  it('keeps the original and display lease until an in-flight native save completes', async () => {
    let complete!: () => void;
    save.mockImplementationOnce(() => new Promise<void>(resolve => { complete = resolve; }));
    openRenderedImageViewer({ src: 'https://example.com/original.png', fileName: 'original.png' });
    await flushPromises();
    expect(wrapper.get('img').attributes('src')).toBe('asset://localhost//display.png');
    const buttons = wrapper.findAll('button');
    await buttons[3].trigger('click');
    await flushPromises();
    expect(save).toHaveBeenCalledWith('/original.png', 'original.png');
    closeRenderedImageViewer();
    await flushPromises();
    expect(invoke.mock.calls.some(args => args[0] === 'release_rendered_image')).toBe(false);
    complete();
    await flushPromises();
    expect(invoke).toHaveBeenCalledWith('release_rendered_image', { path: '/original.png', displayPath: '/display.png' });
    expect(notify).toHaveBeenCalledWith(expect.objectContaining({ type: 'success' }));
  });

  it('releases a late preparation result after its viewer session closes', async () => {
    let complete!: (image: typeof prepared) => void;
    invoke.mockImplementation((command: string) => command === 'prepare_rendered_image' ? new Promise(resolve => { complete = resolve; }) : Promise.resolve());
    openRenderedImageViewer({ src: 'https://example.com/late.png' });
    await flushPromises();
    closeRenderedImageViewer();
    await flushPromises();
    complete(prepared);
    await flushPromises();
    expect(wrapper.find('img').exists()).toBe(false);
    expect(invoke).toHaveBeenCalledWith('release_rendered_image', { path: '/original.png', displayPath: '/display.png' });
  });

  it('rejects messages from a mismatched opaque-frame nonce and iframe-owned Blob URLs', async () => {
    const frame = document.createElement('iframe');
    frame.srcdoc = '';
    frame.dataset.vcpImageNonce = 'current';
    document.body.append(frame);
    window.dispatchEvent(new MessageEvent('message', { origin: 'null', source: frame.contentWindow, data: { source: 'vcp-mobile', type: 'rendered-image-click', nonce: 'old', image: { src: 'https://example.com/stale.png' } } }));
    window.dispatchEvent(new MessageEvent('message', { origin: 'null', source: frame.contentWindow, data: { source: 'vcp-mobile', type: 'rendered-image-click', nonce: 'current', image: { src: 'blob:null/owned-by-frame' } } }));
    await flushPromises();
    expect(invoke).not.toHaveBeenCalled();
    expect(wrapper.find('img').exists()).toBe(false);
  });

  it('grants only one opaque-document pixel operation until its owner completes', async () => {
    const makeFrame = (nonce: string) => {
      const frame = document.createElement('iframe');
      frame.srcdoc = '';
      frame.dataset.vcpImageNonce = nonce;
      document.body.append(frame);
      return { frame, send: vi.spyOn(frame.contentWindow!, 'postMessage') };
    };
    const first = makeFrame('first');
    const second = makeFrame('second');
    const dispatch = (frame: HTMLIFrameElement, type: string, requestId: string) => window.dispatchEvent(new MessageEvent('message', {
      origin: 'null', source: frame.contentWindow,
      data: { source: 'vcp-mobile', type, nonce: frame.dataset.vcpImageNonce, requestId, image: { src: 'https://example.com/image.png' } },
    }));
    dispatch(first.frame, 'rendered-image-request', 'one');
    dispatch(second.frame, 'rendered-image-request', 'two');
    await flushPromises();
    expect(first.send).toHaveBeenCalledWith(expect.objectContaining({ type: 'rendered-image-grant' }), '*');
    expect(second.send).not.toHaveBeenCalled();
    dispatch(first.frame, 'rendered-image-click', 'one');
    await flushPromises();
    expect(second.send).toHaveBeenCalledWith(expect.objectContaining({ type: 'rendered-image-grant' }), '*');
    // Removing the preview releases its document-owned work without waiting for a missing reply.
    second.frame.remove();
    await flushPromises();
  });
});
