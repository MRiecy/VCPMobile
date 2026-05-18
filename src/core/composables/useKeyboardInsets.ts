import { ref, onMounted, onUnmounted, type Ref } from "vue";
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

interface KeyboardInsetDetail {
  height: number;
  visible: boolean;
  safeAreaBottom?: number;
}

interface UseKeyboardInsetsReturn {
  keyboardHeight: Ref<number>;
  isKeyboardOpen: Ref<boolean>;
  safeAreaBottom: Ref<number>;
  forceRecalculate: () => void;
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
  const keyboardHeight = ref(0);
  const isKeyboardOpen = ref(false);
  const safeAreaBottom = ref(0);

  // --- 策略 1：原生注入事件（最可靠） ---
  let unlistenKeyboard: UnlistenFn | null = null;

  const handleNativeInset = (detail: KeyboardInsetDetail) => {
    if (detail && typeof detail.height === "number") {
      // Android WindowInsets 返回的是物理像素，需转换为 CSS 逻辑像素
      const dpr = window.devicePixelRatio || 1;
      keyboardHeight.value = Math.round(detail.height / dpr);
      isKeyboardOpen.value = detail.visible;
      if (typeof detail.safeAreaBottom === "number") {
        safeAreaBottom.value = Math.round(detail.safeAreaBottom / dpr);
      }
    }
  };

  // --- 策略 2：Virtual Keyboard API ---
  let vkCleanup: (() => void) | null = null;
  const setupVirtualKeyboard = () => {
    const vk = (navigator as any).virtualKeyboard;
    if (!vk) return;

    vk.overlaysContent = true;

    const onGeometryChange = (e: any) => {
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

  const estimateFromScroll = () => {
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
    setTimeout(() => {
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
    setTimeout(() => {
      const active = document.activeElement;
      const stillEditing =
        active instanceof HTMLInputElement ||
        active instanceof HTMLTextAreaElement ||
        (active as HTMLElement)?.isContentEditable;
      if (!stillEditing) {
        keyboardHeight.value = 0;
        isKeyboardOpen.value = false;
      }
    }, 150);
  };

  // --- 公共方法：强制重算 ---
  const forceRecalculate = () => {
    // 优先尝试原生事件已在监听器中处理；此处作为兜底再触发一次 scroll 估算
    estimateFromScroll();
  };

  onMounted(async () => {
    unlistenKeyboard = await listen<KeyboardInsetDetail>('vcp-keyboard-inset', (event) => {
      handleNativeInset(event.payload);
    });
    setupVirtualKeyboard();
    document.addEventListener("focusin", handleFocusIn);
    document.addEventListener("focusout", handleFocusOut);
  });

  onUnmounted(() => {
    if (unlistenKeyboard) {
      unlistenKeyboard();
      unlistenKeyboard = null;
    }
    if (vkCleanup) vkCleanup();
    document.removeEventListener("focusin", handleFocusIn);
    document.removeEventListener("focusout", handleFocusOut);
    if (focusTimeout) clearTimeout(focusTimeout);
  });

  return {
    keyboardHeight,
    isKeyboardOpen,
    safeAreaBottom,
    forceRecalculate,
  };
}
