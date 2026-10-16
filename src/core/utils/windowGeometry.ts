export interface GeometryRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface ViewportSample extends GeometryRect {
  scale: number;
  layoutWidth: number;
}

export interface SafeInsets {
  top: number;
  right: number;
  bottom: number;
  left: number;
}

export interface NativeWindowGeometry {
  webViewGeneration: number;
  geometrySequence: number;
  sampleId: number;
  viewportRevision: number;
  boundsPoints: GeometryRect;
  keyboardVisible: boolean;
  dockedTopPoints?: number | null;
  floatingRectPoints?: GeometryRect | null;
}

export interface WindowGeometry {
  usableRect: GeometryRect;
  safeInsets: SafeInsets;
  floatingRect: GeometryRect | null;
  keyboardVisible: boolean;
}

const clamp = (n: number, min: number, max: number) => Math.max(min, Math.min(max, n));

export function intersectRects(a: GeometryRect, b: GeometryRect): GeometryRect {
  const x = Math.max(a.x, b.x);
  const y = Math.max(a.y, b.y);
  return { x, y, width: Math.max(0, Math.min(a.x + a.width, b.x + b.width) - x),
    height: Math.max(0, Math.min(a.y + a.height, b.y + b.height) - y) };
}

export function sameViewport(a: ViewportSample, b: ViewportSample): boolean {
  return a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height
    && a.scale === b.scale && a.layoutWidth === b.layoutWidth;
}

/** Inputs are one paired native/viewport sample. DPR is deliberately not involved. */
export function normalizeWindowGeometry(
  viewport: ViewportSample,
  safe: SafeInsets,
  native?: NativeWindowGeometry,
): WindowGeometry {
  const validNative = native && native.boundsPoints.width > 0 && native.boundsPoints.height > 0;
  const ratio = validNative ? viewport.layoutWidth / native.boundsPoints.width / viewport.scale : 1;
  const mapRect = (rect: GeometryRect): GeometryRect => ({
    x: viewport.x + (rect.x - (native?.boundsPoints.x ?? 0)) * ratio,
    y: viewport.y + (rect.y - (native?.boundsPoints.y ?? 0)) * ratio,
    width: rect.width * ratio,
    height: rect.height * ratio,
  });
  const full = validNative ? mapRect(native.boundsPoints) : { ...viewport };
  const usableRect = intersectRects(viewport, full);
  if (validNative && native.keyboardVisible && native.dockedTopPoints != null) {
    const bottom = viewport.y + (native.dockedTopPoints - native.boundsPoints.y) * ratio;
    usableRect.height = clamp(bottom - usableRect.y, 0, usableRect.height);
  }
  // Unsafe bands belong to the original WebView, not the keyboard-adjusted surface.
  const safeInsets = {
    top: clamp(full.y + safe.top - usableRect.y, 0, usableRect.height),
    right: clamp(usableRect.x + usableRect.width - (full.x + full.width - safe.right), 0, usableRect.width),
    bottom: clamp(usableRect.y + usableRect.height - (full.y + full.height - safe.bottom), 0, usableRect.height),
    left: clamp(full.x + safe.left - usableRect.x, 0, usableRect.width),
  };
  const floating = validNative && native.keyboardVisible && native.floatingRectPoints
    ? intersectRects(mapRect(native.floatingRectPoints), usableRect) : null;
  return { usableRect, safeInsets,
    floatingRect: floating && floating.width > 0 && floating.height > 0 ? floating : null,
    keyboardVisible: validNative ? native.keyboardVisible : false };
}
