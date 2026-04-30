import { ref } from 'vue';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
import { router } from '../router';

/**
 * VCP Mobile Unified Modal History Stack 2.0 (Operation Aegis)
 * Ensures mobile back gestures (swipe/hardware button) close modals instead of exiting the app.
 * Provides "Double-Tap to Exit" protection when no modals are open.
 */

interface ModalInstance {
  id: string;
  close: () => void;
}

// Global stack to track open modals across the entire application
const modalStack = ref<ModalInstance[]>([]);

// Flag to prevent redundant history.back() when closing via popstate
let isProcessingPopState = false;

// Flag to silently block popstate callback when closing via UI (history.back())
let isInternalBack = false;

// Double-tap to exit state
export const showExitToast = ref(false);
let lastBackPressTime = 0;
const EXIT_THRESHOLD = 2000; // 2 seconds

/**
 * Initialize root history state to intercept the final back gesture.
 * Should be called once at app startup (App.vue onMounted).
 */
export const initRootHistory = () => {
  if (typeof window === 'undefined') return;

  // If we are at the very beginning (length 1 or 2), push our dummy state
  // We don't use replaceState here because we WANT to be at depth > 1
  if (window.history.length <= 2) {
    window.history.pushState({ vcpRoot: true, vcpMain: true }, '');
  }
};

const handlePopState = (event: PopStateEvent) => {
  // 1. Check if this back was triggered by unregisterModal (UI action)
  if (isInternalBack) {
    isInternalBack = false;
    return;
  }

  // 2. Handle normal route navigation first.
  // 对于 /settings、/agents/:id 这类真实路由页，返回手势应该交给 vue-router，
  // 不能误判成“要退出应用”或“要关闭底层侧边栏”。
  // 只有在主界面（/ 或 /chat）时，我们才拦截返回手势用于操作 Overlay。
  const currentPath = router.currentRoute.value.path;
  if (currentPath !== '/chat' && currentPath !== '/') {
    return;
  }

  // 3. Handle Modal Stack (LIFO)
  if (modalStack.value.length > 0) {
    const topModal = modalStack.value[modalStack.value.length - 1];

    isProcessingPopState = true;
    try {
      topModal.close();
      modalStack.value.pop();
    } finally {
      isProcessingPopState = false;
    }
    return;
  }

  // 4. Handle Root Exit (Operation Dummy Root - Catch & Bounce)
  // If we hit a state that doesn't have vcpMain, it means we've popped our dummy state
  if (!event.state || !event.state.vcpMain) {
    const currentTime = Date.now();

    if (currentTime - lastBackPressTime < EXIT_THRESHOLD) {
      // Second tap within threshold -> Exit App
      getCurrentWebviewWindow().close();
    } else {
      // First tap -> Show Toast and BOUNCE back
      lastBackPressTime = currentTime;
      showExitToast.value = true;
      setTimeout(() => { showExitToast.value = false; }, EXIT_THRESHOLD);

      // THE BOUNCE: Immediately re-inject the dummy state to keep the user in the "fake" 2nd layer
      window.history.pushState({ vcpRoot: true, vcpMain: true }, '');
    }
  }
};

let popstateHandler: ((e: PopStateEvent) => void) | null = null;
let listenerRegistered = false;

// Initialize the popstate listener only once
if (typeof window !== 'undefined' && !listenerRegistered) {
  listenerRegistered = true;
  popstateHandler = handlePopState;
  window.addEventListener('popstate', popstateHandler);

  // Initial check: if we are at root, push the dummy state
  // Note: App.vue will call this again after router is ready to be 100% sure
  initRootHistory();
}

export function cleanupModalHistory() {
  if (popstateHandler && typeof window !== 'undefined') {
    window.removeEventListener('popstate', popstateHandler);
    popstateHandler = null;
    listenerRegistered = false;
  }
}

export function useModalHistory() {
  /**
   * Registers a modal as "open" and pushes a state to the history stack.
   * @param id Unique identifier for the modal
   * @param closeHandler Callback to close the modal (triggered by back gesture)
   */
  const registerModal = (id: string, closeHandler: () => void) => {
    // Avoid double registration
    if (modalStack.value.some(m => m.id === id)) return;

    // Push state to history
    window.history.pushState({ vcpRoot: true, vcpModalId: id }, '');

    // Add to our LIFO stack
    modalStack.value.push({ id, close: closeHandler });
  };

  /**
   * Unregisters a modal. Should be called when the modal is closed via UI.
   * If not already processing a popstate, it will trigger history.back().
   * @param id Unique identifier for the modal
   */
  const unregisterModal = (id: string) => {
    const index = modalStack.value.findIndex(m => m.id === id);
    if (index === -1) return;

    if (!isProcessingPopState) {
      const currentState = window.history.state;
      if (currentState && currentState.vcpModalId === id) {
        isInternalBack = true;
        window.history.back();
      }
    }

    modalStack.value.splice(index, 1);
  };

  return {
    registerModal,
    unregisterModal,
    modalStackLength: () => modalStack.value.length,
    showExitToast,
    initRootHistory
  };
}
