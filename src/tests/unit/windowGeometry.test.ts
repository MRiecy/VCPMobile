import { describe, expect, it } from 'vitest';
import { normalizeWindowGeometry, type NativeWindowGeometry } from '../../core/utils/windowGeometry';

const native: NativeWindowGeometry = {
  webViewGeneration: 1, geometrySequence: 1, sampleId: 1, viewportRevision: 1,
  boundsPoints: { x: 0, y: 0, width: 390, height: 844 }, keyboardVisible: true,
  dockedTopPoints: 520,
};
const safe = { top: 47, right: 0, bottom: 34, left: 0 };

describe('paired window geometry', () => {
  it('intersects an already resized visual viewport with the same docked keyboard once', () => {
    const result = normalizeWindowGeometry({ x: 0, y: 0, width: 390, height: 520, scale: 1, layoutWidth: 390 }, safe, native);
    expect(result.usableRect).toEqual({ x: 0, y: 0, width: 390, height: 520 });
    expect(result.safeInsets).toEqual({ top: 47, right: 0, bottom: 0, left: 0 });
    expect(result.keyboardVisible).toBe(true);
  });

  it('uses scale and offsets, independently of device pixel density', () => {
    const result = normalizeWindowGeometry({ x: 25, y: 80, width: 195, height: 400, scale: 2, layoutWidth: 390 }, safe, native);
    expect(result.usableRect).toEqual({ x: 25, y: 80, width: 195, height: 260 });
    expect(result.safeInsets.bottom).toBe(0);
  });

  it('keeps floating obstruction local and preserves the full surface', () => {
    const result = normalizeWindowGeometry({ x: 0, y: 0, width: 390, height: 844, scale: 1, layoutWidth: 390 }, safe, {
      ...native, dockedTopPoints: null, floatingRectPoints: { x: 100, y: 400, width: 230, height: 220 },
    });
    expect(result.usableRect.height).toBe(844);
    expect(result.floatingRect).toEqual({ x: 100, y: 400, width: 230, height: 220 });
    expect(result.safeInsets.bottom).toBe(34);
  });

  it('does not infer keyboard visibility from zoom/viewport shrink or hidden guide safe area', () => {
    const viewport = { x: 0, y: 0, width: 195, height: 422, scale: 2, layoutWidth: 390 };
    expect(normalizeWindowGeometry(viewport, safe).keyboardVisible).toBe(false);
    const result = normalizeWindowGeometry(viewport, safe, { ...native, keyboardVisible: false, dockedTopPoints: 810 });
    expect(result.usableRect.height).toBe(422);
    expect(result.keyboardVisible).toBe(false);
  });
});
