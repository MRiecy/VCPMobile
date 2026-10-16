import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { attachIOSWindowGeometry } from '../../core/composables/useIOSWindowGeometry';
import { invokeMock, mockInvoke } from '../mocks/tauri';
import type { NativeWindowGeometry } from '../../core/utils/windowGeometry';

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(complete => { resolve = complete; });
  return { promise, resolve };
}

describe('iOS window sample ownership', () => {
  let release: (() => void) | undefined;
  const viewport = { width: 390, height: 844, offsetLeft: 0, offsetTop: 0, scale: 1,
    addEventListener: vi.fn(), removeEventListener: vi.fn() };
  beforeEach(() => {
    vi.useFakeTimers();
    viewport.height = 844;
    Object.defineProperty(window, 'visualViewport', { configurable: true, value: viewport });
    Object.defineProperty(document.documentElement, 'clientWidth', { configurable: true, value: 390 });
  });
  afterEach(() => {
    release?.();
    release = undefined;
    document.documentElement.removeAttribute('style');
    delete document.documentElement.dataset.vcpPlatform;
    vi.useRealTimers();
  });
  const snapshot = (sampleId: number, viewportRevision: number, generation = 1): NativeWindowGeometry => ({
    webViewGeneration: generation, geometrySequence: sampleId, sampleId, viewportRevision,
    boundsPoints: { x: 0, y: 0, width: 390, height: 844 }, keyboardVisible: true, dockedTopPoints: 520,
  });

  it('rejects mismatched viewport inputs and coalesces dirty events behind one request', async () => {
    const first = deferred<NativeWindowGeometry>();
    mockInvoke('plugin:vcp-mobile|get_webview_geometry', args => {
      if (args?.sampleId === 1) return first.promise;
      return snapshot(Number(args?.sampleId), Number(args?.viewportRevision));
    });
    const applied = vi.fn();
    release = attachIOSWindowGeometry(applied);
    await vi.advanceTimersByTimeAsync(20);
    expect(invokeMock).toHaveBeenCalledTimes(1);
    viewport.height = 520;
    window.dispatchEvent(new Event('resize'));
    window.dispatchEvent(new Event('resize'));
    await vi.advanceTimersByTimeAsync(20);
    expect(invokeMock).toHaveBeenCalledTimes(1);
    first.resolve(snapshot(1, 1));
    await vi.advanceTimersByTimeAsync(20);
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(applied).toHaveBeenCalledTimes(2); // initial viewport + consistent second pair
    expect(applied.mock.calls[1][0].usableRect.height).toBe(520);
    expect(invokeMock.mock.calls[1][1]?.viewportRevision).toBe(2);
  });

  it('does not apply an old WebView reply after replacement or an unmounted owner', async () => {
    const first = deferred<NativeWindowGeometry>();
    const second = deferred<NativeWindowGeometry>();
    mockInvoke('plugin:vcp-mobile|get_webview_geometry', args => args?.sampleId === 1 ? first.promise : second.promise);
    const applied = vi.fn();
    release = attachIOSWindowGeometry(applied);
    await vi.advanceTimersByTimeAsync(20);
    window.dispatchEvent(new CustomEvent('vcp-webview-geometry-changed', { detail: { webViewGeneration: 2 } }));
    first.resolve(snapshot(1, 1, 1));
    await vi.advanceTimersByTimeAsync(20);
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(applied).toHaveBeenCalledTimes(1);
    release();
    second.resolve(snapshot(2, 1, 2));
    await vi.advanceTimersByTimeAsync(20);
    expect(applied).toHaveBeenCalledTimes(1);
  });

  it('constrains only the active textarea around floating occlusion and restores it on blur', async () => {
    mockInvoke('plugin:vcp-mobile|get_webview_geometry', args => ({
      ...snapshot(Number(args?.sampleId), Number(args?.viewportRevision)), dockedTopPoints: null,
      floatingRectPoints: { x: 100, y: 400, width: 230, height: 220 },
    }));
    const editor = document.createElement('textarea');
    editor.style.maxHeight = '600px';
    vi.spyOn(editor, 'getBoundingClientRect').mockReturnValue({
      x: 10, y: 100, left: 10, top: 100, width: 350, height: 600, right: 360, bottom: 700,
      toJSON: () => ({}),
    });
    const next = document.createElement('button');
    document.body.append(editor, next);
    editor.focus();
    release = attachIOSWindowGeometry(vi.fn());
    await vi.advanceTimersByTimeAsync(20);
    expect(document.documentElement.style.getPropertyValue('--vcp-viewport-height')).toBe('844px');
    expect(editor.style.maxHeight).toBe('292px');
    next.focus();
    await vi.advanceTimersByTimeAsync(20);
    expect(editor.style.maxHeight).toBe('600px');
    expect(editor.style.marginTop).toBe('');
  });
});
