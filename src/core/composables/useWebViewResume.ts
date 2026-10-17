import { nextTick, onMounted, onUnmounted, watch, type Ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { isAndroid, isIOS } from '../utils/platform';

interface ResumeMarker {
  documentId: string;
  readyToken: string | null;
}

declare global {
  interface Window {
    __vcpWebViewResume?: ResumeMarker;
  }
}

interface ResumeSurface {
  root: Ref<HTMLElement | null>;
  ready: () => boolean;
  appearance: () => string;
  wallpaperUrl: () => string | null;
}

const cancelled = () => new DOMException('Resume surface changed', 'AbortError');

function layoutFrame(signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) { reject(cancelled()); return; }
    const stop = () => {
      cancelAnimationFrame(frame);
      reject(cancelled());
    };
    const frame = requestAnimationFrame(() => {
      signal.removeEventListener('abort', stop);
      resolve();
    });
    signal.addEventListener('abort', stop, { once: true });
  });
}

function decodeWallpaper(url: string | null, signal: AbortSignal): Promise<void> {
  if (!url) return Promise.resolve();
  return new Promise((resolve, reject) => {
    if (signal.aborted) { reject(cancelled()); return; }
    const image = new Image();
    let settled = false;
    const finish = (aborted = false) => {
      if (settled) return;
      settled = true;
      image.onload = null;
      image.onerror = null;
      signal.removeEventListener('abort', stop);
      if (aborted) {
        image.src = '';
        reject(cancelled());
      } else resolve();
    };
    const stop = () => finish(true);
    signal.addEventListener('abort', stop, { once: true });
    image.onerror = () => finish(); // A failed wallpaper leaves the existing CSS fallback visible.
    image.src = url;
    if (typeof image.decode === 'function') {
      void image.decode().then(() => finish(), () => finish());
    } else {
      image.onload = () => finish();
      if (image.complete) finish();
    }
  });
}

/** Native owns the snapshot and drawing barrier; Vue only acknowledges its current document. */
export function useWebViewResume(surface: ResumeSurface): void {
  if (!isAndroid && !isIOS) return;
  const marker: ResumeMarker = { documentId: crypto.randomUUID(), readyToken: null };
  window.__vcpWebViewResume = marker;
  let mounted = false;
  let disposed = false;
  let token: string | null = null;
  let completedToken: string | null = null;
  let work: AbortController | null = null;
  let eventRevision = 0;
  let connecting = false;
  let connectQueued = false;
  const ownsDocument = () => !disposed && window.__vcpWebViewResume === marker;

  const prepare = () => {
    if (!ownsDocument() || !mounted || document.hidden || !surface.ready()
      || !token || work || completedToken === token) return;
    const acceptedToken = token;
    const controller = new AbortController();
    work = controller;
    const current = () => ownsDocument() && work === controller && !controller.signal.aborted
      && token === acceptedToken && !document.hidden && surface.ready();
    void (async () => {
      try {
        await nextTick();
        if (!current()) return;
        await decodeWallpaper(surface.wallpaperUrl(), controller.signal);
        await nextTick();
        await layoutFrame(controller.signal);
        if (!current() || !surface.root.value?.isConnected) return;
        // Consume the current layout after the initial history owner and Vue have
        // settled. Future tokens/stream chunks do not have to become quiet.
        surface.root.value.getBoundingClientRect();
        await layoutFrame(controller.signal);
        if (!current()) return;
        marker.readyToken = acceptedToken;
        await invoke('plugin:vcp-mobile|complete_webview_resume', {
          documentId: marker.documentId, token: acceptedToken,
        });
        if (current()) completedToken = acceptedToken;
      } catch (error) {
        if (current()) console.warn('[WebViewResume] Surface acknowledgement failed:', error);
      } finally {
        if (work === controller) work = null;
      }
    })();
  };

  const invalidate = () => {
    work?.abort();
    work = null;
    if (!ownsDocument()) return;
    marker.readyToken = null;
    completedToken = null;
    prepare();
  };

  const receiveToken = (next: string | null) => {
    if (!ownsDocument()) return;
    if (next === token) { prepare(); return; }
    token = next;
    invalidate();
  };

  const connect = async () => {
    if (!ownsDocument()) return;
    if (connecting) { connectQueued = true; return; }
    connecting = true;
    connectQueued = false;
    const observedEvents = eventRevision;
    try {
      const receipt = await invoke<{ token: string | null }>('plugin:vcp-mobile|connect_webview_resume', {
        documentId: marker.documentId,
      });
      // UUID tokens are identities, not sortable versions. A newer event always
      // wins over an older connect response that was already in flight.
      if (ownsDocument() && eventRevision === observedEvents) {
        if (receipt.token === null || typeof receipt.token === 'string' && receipt.token.length > 0) {
          receiveToken(receipt.token);
        }
      }
    } catch (error) {
      if (ownsDocument()) console.warn('[WebViewResume] Native connection failed:', error);
    } finally {
      connecting = false;
      if (connectQueued && ownsDocument() && !document.hidden) void connect();
    }
  };

  const onResume = (event: Event) => {
    const detail = (event as CustomEvent<{ token?: unknown; finished?: boolean }>).detail;
    const next = detail?.token;
    if (typeof next !== 'string' || !next) return;
    if (detail.finished === true) {
      if (next !== token) return;
      eventRevision += 1;
      receiveToken(null);
      return;
    }
    eventRevision += 1;
    receiveToken(next);
  };
  const onVisibility = () => {
    invalidate();
    if (!document.hidden) void connect();
  };
  // Observe only viewport/native geometry, never message heights. The native
  // frame can keep its size while keyboard avoidance changes the Vue surface.
  const geometryEvent = isIOS ? 'vcp-window-geometry-applied' : 'vcp-keyboard-inset';
  const viewport = window.visualViewport;
  // Install before the first connect so a resume arriving during registration is retained.
  window.addEventListener('vcp-webview-resume', onResume);
  window.addEventListener(geometryEvent, invalidate);
  viewport?.addEventListener('resize', invalidate);
  viewport?.addEventListener('scroll', invalidate);
  document.addEventListener('visibilitychange', onVisibility);
  const stopWatching = watch(
    () => [surface.ready(), surface.appearance(), surface.wallpaperUrl()] as const,
    invalidate,
    { flush: 'sync' },
  );
  onMounted(() => {
    mounted = true;
    prepare();
    void connect();
  });
  onUnmounted(() => {
    disposed = true;
    work?.abort();
    work = null;
    stopWatching();
    window.removeEventListener('vcp-webview-resume', onResume);
    window.removeEventListener(geometryEvent, invalidate);
    viewport?.removeEventListener('resize', invalidate);
    viewport?.removeEventListener('scroll', invalidate);
    document.removeEventListener('visibilitychange', onVisibility);
    if (window.__vcpWebViewResume === marker) delete window.__vcpWebViewResume;
  });
}
