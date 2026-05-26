import { ref } from 'vue';

/**
 * VCP Mobile Unified Modal History Stack 2.0 (Operation Aegis)
 * Ensures mobile back gestures (swipe/hardware button) close modals instead of exiting the app.
 *
 * Root exit handling (double-tap to exit with toast) has been moved to App.vue via
 * the `vcp-exit-requested` custom event, which is fired when the user reaches the
 * bottom of the history stack. This keeps modal history logic decoupled from UI/toast.
 */

interface ModalInstance {
  id: string;
  close: () => void;
}

// Global stack to track open modals across the entire application
const modalStack = ref<ModalInstance[]>([]);

// Modal history runtime state machine
// IDLE: normal state
// POPSTATE_HANDLING: currently processing a browser popstate event (closing top modal)
// INTERNAL_BACK: triggered an internal history.back() from unregisterModal
let state: 'IDLE' | 'POPSTATE_HANDLING' | 'INTERNAL_BACK' = 'IDLE';

/**
 * Initialize root history state to intercept the final back gesture.
 * Should be called once at app startup (App.vue onMounted).
 */
export const initRootHistory = () => {
  if (typeof window === 'undefined') return;

  const state = window.history.state;
  // 自校准状态检测：只要当前栈顶没有 vcpMain 标记，就压入 dummy state，确保防护盾始终有效
  if (!state || !state.vcpMain) {
    window.history.pushState({ vcpRoot: true, vcpMain: true }, '');
  }
};

const handlePopState = (event: PopStateEvent) => {
  // 1. Check if this back was triggered by unregisterModal (UI action)
  if (state === 'INTERNAL_BACK') {
    state = 'IDLE';
    return;
  }

  // 2. Handle normal route navigation first.
  // 对于 /settings、/agents/:id 这类真实路由页，返回手势应该交给 vue-router，
  // 不能误判成“要退出应用”或“要关闭底层侧边栏”。
  // 只有在主界面（/ 或 /chat）时，我们才拦截返回手势用于操作 Overlay。
  const currentPath = window.location.hash.replace(/^#/, '') || '/';
  if (currentPath !== '/chat' && currentPath !== '/') {
    return;
  }

  // 3. Handle Modal Stack (LIFO)
  if (modalStack.value.length > 0) {
    const topModal = modalStack.value[modalStack.value.length - 1];

    state = 'POPSTATE_HANDLING';
    try {
      topModal.close();
    } finally {
      state = 'IDLE';
    }
    return;
  }

  // 4. Handle Root Exit — delegate to App.vue via custom event
  // If we hit a state that doesn't have vcpMain, it means we've popped our dummy state
  if (!event.state || !event.state.vcpMain) {
    window.dispatchEvent(new CustomEvent('vcp-exit-requested'));

    // THE BOUNCE: Re-inject the dummy state so the next back gesture also triggers popstate
    window.history.pushState({ vcpRoot: true, vcpMain: true }, '');
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

    // If we're currently handling a popstate event, don't trigger history.back()
    // to avoid recursive popstate. The modal will be removed from stack regardless.
    if (state !== 'POPSTATE_HANDLING') {
      const currentState = window.history.state;
      if (currentState && currentState.vcpModalId === id) {
        state = 'INTERNAL_BACK';
        window.history.back();
      }
    }

    modalStack.value.splice(index, 1);
  };

  const closeTopModal = (): boolean => {
    if (modalStack.value.length > 0) {
      const topModal = modalStack.value[modalStack.value.length - 1];
      topModal.close();
      return true;
    }
    return false;
  };

  return {
    registerModal,
    unregisterModal,
    modalStackLength: () => modalStack.value.length,
    initRootHistory,
    closeTopModal
  };
}
