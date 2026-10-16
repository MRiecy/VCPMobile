import { invoke } from '@tauri-apps/api/core';
import { normalizeWindowGeometry, sameViewport, intersectRects,
  type NativeWindowGeometry, type ViewportSample, type WindowGeometry } from '../utils/windowGeometry';

let refresh: (() => void) | null = null;

export function refreshIOSWindowGeometry(): void { refresh?.(); }

/** One root owner, one native sample in flight; editors only consume its result. */
export function attachIOSWindowGeometry(onGeometry: (geometry: WindowGeometry) => void): () => void {
  const root = document.documentElement;
  root.dataset.vcpPlatform = 'ios';
  const probe = document.createElement('div');
  probe.setAttribute('aria-hidden', 'true');
  probe.style.cssText = 'position:fixed;inset:0;visibility:hidden;pointer-events:none;contain:strict;'
    + 'padding:env(safe-area-inset-top,0px) env(safe-area-inset-right,0px) env(safe-area-inset-bottom,0px) env(safe-area-inset-left,0px)';
  document.body.append(probe);
  let disposed = false;
  let frame = 0;
  let inFlight = false;
  let dirty = false;
  let sampleId = 0;
  let viewportRevision = 0;
  let nativeRevision = 0;
  let generation = -1;
  let sequence = -1;
  let geometry: WindowGeometry;
  let lastViewport: ViewportSample | null = null;
  let constrainedEditor: { element: HTMLTextAreaElement; maxHeight: string; marginTop: string } | null = null;

  const restoreEditor = () => {
    if (!constrainedEditor) return;
    const { element, maxHeight, marginTop } = constrainedEditor;
    element.style.maxHeight = maxHeight;
    element.style.marginTop = marginTop;
    constrainedEditor = null;
  };

  const readViewport = (): ViewportSample => {
    const viewport = window.visualViewport;
    return { x: viewport?.offsetLeft ?? 0, y: viewport?.offsetTop ?? 0,
      width: viewport?.width ?? root.clientWidth, height: viewport?.height ?? root.clientHeight,
      scale: viewport?.scale || 1, layoutWidth: root.clientWidth };
  };
  const readSafe = () => {
    const style = getComputedStyle(probe);
    return { top: parseFloat(style.paddingTop) || 0, right: parseFloat(style.paddingRight) || 0,
      bottom: parseFloat(style.paddingBottom) || 0, left: parseFloat(style.paddingLeft) || 0 };
  };
  const publish = (value: WindowGeometry) => {
    geometry = value;
    const rect = value.usableRect;
    for (const [name, n] of Object.entries({ left: rect.x, top: rect.y, width: rect.width, height: rect.height })) {
      root.style.setProperty(`--vcp-viewport-${name}`, `${n}px`);
    }
    for (const [name, n] of Object.entries(value.safeInsets)) root.style.setProperty(`--vcp-safe-${name}`, `${n}px`);
    root.style.setProperty('--vcp-ime-offset', '0px');
    onGeometry(value);
    revealEditor();
    window.dispatchEvent(new CustomEvent('vcp-window-geometry-applied'));
  };

  // Only scroll the active editor's own scroll owner. Never move the document/root
  // in response to a floating keyboard, which would change every visible panel.
  const revealEditor = () => {
    restoreEditor();
    const active = document.activeElement;
    const obstacle = geometry?.floatingRect;
    if (!obstacle || !(active instanceof HTMLElement)
      || !active.matches('input,textarea,[contenteditable="true"]')) return;
    const bounds = active.getBoundingClientRect();
    let target = { x: bounds.x, y: bounds.y, width: bounds.width, height: bounds.height };
    const selection = window.getSelection();
    if (active.isContentEditable && selection?.rangeCount && active.contains(selection.anchorNode)) {
      const caret = selection.getRangeAt(0).getBoundingClientRect();
      if (caret.height > 0) target = { x: caret.x, y: caret.y, width: Math.max(1, caret.width), height: caret.height };
    }
    const overlap = intersectRects(target, obstacle);
    if (overlap.width === 0 || overlap.height === 0) return;
    if (active instanceof HTMLTextAreaElement) {
      // Full-screen textareas may have no scrollable ancestor. Constrain only
      // this editor's visible box to the larger unobscured vertical segment;
      // its own scrolling continues to reveal the caret. The surface stays full.
      const above = obstacle.y - bounds.y - 8;
      const belowTop = obstacle.y + obstacle.height + 8;
      const below = geometry.usableRect.y + geometry.usableRect.height - belowTop;
      const lineHeight = parseFloat(getComputedStyle(active).lineHeight) || 24;
      if (Math.max(above, below) >= Math.max(44, lineHeight * 2)) {
        constrainedEditor = { element: active, maxHeight: active.style.maxHeight, marginTop: active.style.marginTop };
        if (above >= below) {
          active.style.maxHeight = `${Math.min(bounds.height, above)}px`;
        } else {
          const margin = parseFloat(getComputedStyle(active).marginTop) || 0;
          active.style.marginTop = `${margin + Math.max(0, belowTop - bounds.y)}px`;
          active.style.maxHeight = `${Math.min(bounds.height, below)}px`;
        }
        return;
      }
    }
    // Textareas have no DOM caret rect: the visible input bounds are conservative.
    const up = target.y + target.height - obstacle.y + 8;
    const down = target.y - (obstacle.y + obstacle.height) - 8;
    const delta = target.y - up >= geometry.usableRect.y ? up : down;
    let owner = active.parentElement;
    while (owner && owner !== document.body && !owner.classList.contains('vcp-viewport-surface')) {
      const overflow = getComputedStyle(owner).overflowY;
      if ((overflow === 'auto' || overflow === 'scroll') && owner.scrollHeight > owner.clientHeight) {
        owner.scrollTop += delta;
        return;
      }
      owner = owner.parentElement;
    }
  };

  const sample = async () => {
    frame = 0;
    if (disposed || inFlight) return;
    dirty = false;
    inFlight = true;
    const viewport = readViewport();
    if (!lastViewport || !sameViewport(lastViewport, viewport)) viewportRevision += 1;
    lastViewport = viewport;
    const requestId = ++sampleId;
    const revision = viewportRevision;
    const invalidation = nativeRevision;
    try {
      const native = await invoke<NativeWindowGeometry>('plugin:vcp-mobile|get_webview_geometry', {
        sampleId: requestId, viewportRevision: revision,
      });
      if (disposed) return;
      const current = readViewport();
      if (!sameViewport(viewport, current) || nativeRevision !== invalidation) {
        dirty = true;
        return;
      }
      if (native.sampleId !== requestId || native.viewportRevision !== revision
        || native.webViewGeneration < generation
        || (native.webViewGeneration === generation && native.geometrySequence <= sequence)) return;
      generation = native.webViewGeneration;
      sequence = native.geometrySequence;
      if (native.boundsPoints.width <= 0 || native.boundsPoints.height <= 0) return;
      publish(normalizeWindowGeometry(viewport, readSafe(), native));
    } catch (error) {
      // The viewport remains usable before attachment / after replacement. A later
      // native or viewport change retries; failures must not become a polling loop.
      if (!disposed) {
        publish(normalizeWindowGeometry(readViewport(), readSafe()));
        console.warn('[WindowGeometry] Native sample unavailable:', error);
      }
    } finally {
      inFlight = false;
      if (dirty && !disposed) schedule();
    }
  };
  const schedule = () => {
    dirty = true;
    if (!frame && !inFlight && !disposed) frame = requestAnimationFrame(() => { void sample(); });
  };
  const onNativeChange = (event: Event) => {
    nativeRevision += 1;
    const next = (event as CustomEvent<{ webViewGeneration?: number }>).detail?.webViewGeneration;
    if (next != null && next > generation) { generation = next; sequence = -1; }
    schedule();
  };
  const onFocus = () => { revealEditor(); schedule(); };
  const onBlur = () => queueMicrotask(() => { if (!disposed) revealEditor(); });
  refresh = schedule;
  window.addEventListener('vcp-webview-geometry-changed', onNativeChange);
  window.addEventListener('resize', schedule);
  window.visualViewport?.addEventListener('resize', schedule);
  window.visualViewport?.addEventListener('scroll', schedule);
  document.addEventListener('focusin', onFocus);
  document.addEventListener('focusout', onBlur);
  document.addEventListener('selectionchange', revealEditor);
  document.addEventListener('input', revealEditor);
  publish(normalizeWindowGeometry(readViewport(), readSafe()));
  schedule();
  return () => {
    disposed = true;
    if (refresh === schedule) refresh = null;
    if (frame) cancelAnimationFrame(frame);
    window.removeEventListener('vcp-webview-geometry-changed', onNativeChange);
    window.removeEventListener('resize', schedule);
    window.visualViewport?.removeEventListener('resize', schedule);
    window.visualViewport?.removeEventListener('scroll', schedule);
    document.removeEventListener('focusin', onFocus);
    document.removeEventListener('focusout', onBlur);
    document.removeEventListener('selectionchange', revealEditor);
    document.removeEventListener('input', revealEditor);
    restoreEditor();
    probe.remove();
  };
}
