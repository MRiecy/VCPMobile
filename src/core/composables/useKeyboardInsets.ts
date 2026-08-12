import { ref, onMounted, onUnmounted, type Ref } from "vue";

export interface NativeInsetsSnapshot {
  safeTopPx: number;
  safeRightPx: number;
  safeBottomPx: number;
  safeLeftPx: number;
  imeBottomPx: number;
  imeVisible: boolean;
}

interface LegacyKeyboardInsetDetail {
  height?: number;
  visible?: boolean;
  safeAreaBottom?: number;
}

type KeyboardInsetDetail = Partial<NativeInsetsSnapshot> & LegacyKeyboardInsetDetail;

interface CssInsetsSnapshot {
  safeTop: number;
  safeRight: number;
  safeBottom: number;
  safeLeft: number;
  imeExtraBottom: number;
  imeVisible: boolean;
}

declare global {
  interface Window {
    __VCP_NATIVE_INSETS__?: NativeInsetsSnapshot;
  }
}

interface UseKeyboardInsetsReturn {
  keyboardHeight: Ref<number>;
  isKeyboardOpen: Ref<boolean>;
  safeAreaBottom: Ref<number>;
  forceRecalculate: () => void;
}

const keyboardHeight = ref(0);
const isKeyboardOpen = ref(false);
const safeAreaBottom = ref(0);
let nativeBridgeUsers = 0;
let nativeBridgeAttached = false;
let nativeSnapshotSeen = false;

const nonNegativePx = (value: unknown): number =>
  typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : 0;

export function normalizeNativeInsets(detail: KeyboardInsetDetail): NativeInsetsSnapshot {
  return {
    safeTopPx: nonNegativePx(detail.safeTopPx),
    safeRightPx: nonNegativePx(detail.safeRightPx),
    safeBottomPx: nonNegativePx(detail.safeBottomPx ?? detail.safeAreaBottom),
    safeLeftPx: nonNegativePx(detail.safeLeftPx),
    imeBottomPx: nonNegativePx(detail.imeBottomPx ?? detail.height),
    imeVisible: detail.imeVisible ?? detail.visible ?? false,
  };
}

export function nativeInsetsToCss(
  snapshot: NativeInsetsSnapshot,
  devicePixelRatio: number,
): CssInsetsSnapshot {
  const dpr = Number.isFinite(devicePixelRatio) && devicePixelRatio > 0
    ? devicePixelRatio
    : 1;
  const toCssPx = (value: number) => Math.round(nonNegativePx(value) / dpr);
  const safeBottomPx = nonNegativePx(snapshot.safeBottomPx);
  const rawImeBottomPx = snapshot.imeVisible ? nonNegativePx(snapshot.imeBottomPx) : 0;

  return {
    safeTop: toCssPx(snapshot.safeTopPx),
    safeRight: toCssPx(snapshot.safeRightPx),
    safeBottom: toCssPx(safeBottomPx),
    safeLeft: toCssPx(snapshot.safeLeftPx),
    imeExtraBottom: toCssPx(Math.max(0, rawImeBottomPx - safeBottomPx)),
    imeVisible: snapshot.imeVisible,
  };
}

function applyNativeInsets(detail: KeyboardInsetDetail): void {
  const snapshot = normalizeNativeInsets(detail);
  const css = nativeInsetsToCss(snapshot, window.devicePixelRatio || 1);
  const rootStyle = document.documentElement.style;

  nativeSnapshotSeen = true;
  safeAreaBottom.value = css.safeBottom;
  keyboardHeight.value = css.imeExtraBottom;
  isKeyboardOpen.value = css.imeVisible;

  rootStyle.setProperty("--vcp-safe-top", `${css.safeTop}px`);
  rootStyle.setProperty("--vcp-safe-right", `${css.safeRight}px`);
  rootStyle.setProperty("--vcp-safe-bottom", `${css.safeBottom}px`);
  rootStyle.setProperty("--vcp-safe-left", `${css.safeLeft}px`);
  rootStyle.setProperty("--vcp-ime-offset", `${css.imeExtraBottom}px`);
}

const handleNativeInset = (event: Event) => {
  const detail = (event as CustomEvent<KeyboardInsetDetail>).detail;
  if (detail) applyNativeInsets(detail);
};

function replayNativeInsets(): boolean {
  const snapshot = window.__VCP_NATIVE_INSETS__;
  if (!snapshot) return false;
  applyNativeInsets(snapshot);
  return true;
}

/**
 * Retains the single window-level native Insets listener and immediately replays
 * the snapshot cached by the Android bridge. App.vue uses this even when no
 * text editor is mounted so every full-screen surface receives four-edge safe
 * area variables from the same owner.
 */
export function retainNativeInsetsBridge(): () => void {
  nativeBridgeUsers += 1;
  if (!nativeBridgeAttached) {
    window.addEventListener("vcp-keyboard-inset", handleNativeInset);
    nativeBridgeAttached = true;
  }
  replayNativeInsets();

  let released = false;
  return () => {
    if (released) return;
    released = true;
    nativeBridgeUsers = Math.max(0, nativeBridgeUsers - 1);
    if (nativeBridgeUsers === 0 && nativeBridgeAttached) {
      window.removeEventListener("vcp-keyboard-inset", handleNativeInset);
      nativeBridgeAttached = false;
    }
  };
}

/**
 * 键盘高度检测组合式函数
 *
 * 核心策略：
 * 1. 优先监听 Android 原生层通过 evaluateJavascript 注入的 `vcp-keyboard-inset` 事件
 * 2. Fallback：Virtual Keyboard API（Chrome 94+）
 * 3. Fallback：focusin/focusout + scrollHeight 差值估算
 *
 * 设计动机：Tauri Android WebView 中 visualViewport 在键盘弹起时不会正确更新
 * （tauri-apps/tauri#10631、#13479），因此必须依赖原生事件或 DOM 级 fallback。
 */
export function useKeyboardInsets(): UseKeyboardInsetsReturn {
  let releaseNativeBridge: (() => void) | null = null;

  // --- 策略 2：Virtual Keyboard API ---
  let vkCleanup: (() => void) | null = null;
  const setupVirtualKeyboard = () => {
    const vk = (navigator as any).virtualKeyboard;
    if (!vk) return;

    vk.overlaysContent = true;

    const onGeometryChange = (e: any) => {
      if (nativeSnapshotSeen) return;
      const height = e.target?.boundingRect?.height ?? 0;
      keyboardHeight.value = height;
      isKeyboardOpen.value = height > 0;
    };

    vk.addEventListener("geometrychange", onGeometryChange);
    vkCleanup = () => {
      vk.removeEventListener("geometrychange", onGeometryChange);
    };
  };

  // --- 策略 3：focus + scrollHeight 估算 ---
  let focusTimeout: ReturnType<typeof setTimeout> | null = null;
  let focusInDelayTimeout: ReturnType<typeof setTimeout> | null = null;
  let focusOutDelayTimeout: ReturnType<typeof setTimeout> | null = null;

  const estimateFromScroll = () => {
    if (nativeSnapshotSeen) return;
    // 延迟等待键盘动画完成
    focusTimeout = setTimeout(() => {
      const diff =
        document.documentElement.scrollHeight - window.innerHeight;
      if (diff > 100) {
        keyboardHeight.value = diff;
        isKeyboardOpen.value = true;
      }
    }, 300);
  };

  const handleFocusIn = () => {
    // 若已有原生事件在 200ms 内到达，则跳过 fallback
    const pending = true;
    if (focusInDelayTimeout) clearTimeout(focusInDelayTimeout);
    focusInDelayTimeout = setTimeout(() => {
      if (pending && !isKeyboardOpen.value) {
        estimateFromScroll();
      }
    }, 200);
  };

  const handleFocusOut = () => {
    if (focusTimeout) {
      clearTimeout(focusTimeout);
      focusTimeout = null;
    }
    // 延迟 150ms 检查：若 focus 只是从一个 input 切到另一个 input，
    // 则不应立即重置键盘高度，避免 footer 闪烁
    if (focusOutDelayTimeout) clearTimeout(focusOutDelayTimeout);
    focusOutDelayTimeout = setTimeout(() => {
      const active = document.activeElement;
      const stillEditing =
        active instanceof HTMLInputElement ||
        active instanceof HTMLTextAreaElement ||
        (active as HTMLElement)?.isContentEditable;
      if (!stillEditing && !nativeSnapshotSeen) {
        keyboardHeight.value = 0;
        isKeyboardOpen.value = false;
      }
    }, 150);
  };

  // --- 公共方法：强制重算 ---
  const forceRecalculate = () => {
    if (!replayNativeInsets()) estimateFromScroll();
  };

  onMounted(() => {
    releaseNativeBridge = retainNativeInsetsBridge();
    setupVirtualKeyboard();
    document.addEventListener("focusin", handleFocusIn);
    document.addEventListener("focusout", handleFocusOut);
  });

  onUnmounted(() => {
    releaseNativeBridge?.();
    releaseNativeBridge = null;
    if (vkCleanup) vkCleanup();
    document.removeEventListener("focusin", handleFocusIn);
    document.removeEventListener("focusout", handleFocusOut);
    if (focusTimeout) clearTimeout(focusTimeout);
    if (focusInDelayTimeout) clearTimeout(focusInDelayTimeout);
    if (focusOutDelayTimeout) clearTimeout(focusOutDelayTimeout);
  });

  return {
    keyboardHeight,
    isKeyboardOpen,
    safeAreaBottom,
    forceRecalculate,
  };
}
