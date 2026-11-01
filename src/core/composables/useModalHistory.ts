import { ref, nextTick } from 'vue';

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
  /** Return false when the current modal owns a non-dismissible operation. */
  close: () => boolean | void;
  surface?: () => HTMLElement | null;
  opener: HTMLElement | null;
  route: string;
  canDismiss?: () => boolean;
  canLeaveForNavigation?: () => boolean;
}

// Global stack to track open modals across the entire application
const modalStack = ref<ModalInstance[]>([]);
let stackRevision = 0;

function resolveSurface(modal: ModalInstance): HTMLElement | null {
  return modal.surface?.() ?? document.querySelector<HTMLElement>(`[data-vcp-modal-id="${CSS.escape(modal.id)}"]`);
}

function isAvailableFocusTarget(element: HTMLElement): boolean {
  if (!element.isConnected || element.matches(':disabled')
    || element.closest('[hidden], [inert], [aria-hidden="true"]')) return false;
  for (let node: HTMLElement | null = element; node; node = node.parentElement) {
    const style = getComputedStyle(node);
    if (style.display === 'none' || style.visibility === 'hidden') return false;
  }
  return true;
}

function removeModal(index: number) {
  const wasTop = index === modalStack.value.length - 1;
  const modal = modalStack.value[index];
  const activeAtClose = document.activeElement;
  modalStack.value.splice(index, 1);
  const revision = ++stackRevision;
  if (!wasTop) return;
  const surface = resolveSurface(modal);
  if (surface && activeAtClose !== modal.opener && activeAtClose !== document.body
    && activeAtClose !== document.documentElement && !surface.contains(activeAtClose)) return;
  void nextTick(() => {
    const active = document.activeElement;
    if (revision !== stackRevision || !modal.opener?.isConnected || modal.route !== window.location.hash) return;
    // A different focus chosen while closing belongs to the user/new context.
    if (active !== activeAtClose && active !== document.body && active !== document.documentElement) return;
    const parent = modalStack.value[modalStack.value.length - 1];
    const parentSurface = parent ? resolveSurface(parent) : null;
    if (parentSurface && !parentSurface.contains(modal.opener)) return;
    if (!isAvailableFocusTarget(modal.opener)) return;
    modal.opener.focus({ preventScroll: true });
  });
}

function closeModal(modal: ModalInstance) {
  if (modal.canDismiss?.() === false) return false;
  if (modal.close() === false) return false;
  useModalHistory().unregisterModal(modal.id);
  return true;
}

function handleModalKeydown(event: KeyboardEvent) {
  const modal = modalStack.value[modalStack.value.length - 1];
  if (!modal || event.isComposing || event.keyCode === 229 || event.defaultPrevented) return;
  if (event.key === 'Escape') {
    event.preventDefault();
    event.stopImmediatePropagation();
    // Reuse unregisterModal's history ordering for keyboard-driven close.
    closeModal(modal);
    return;
  }
  if (event.key !== 'Tab') return;
  const surface = resolveSurface(modal);
  if (!surface) return;
  const focusable = Array.from(surface.querySelectorAll<HTMLElement>(
    'button, a[href], input, textarea, select, [tabindex], [contenteditable="true"]',
  )).filter(element => element.tabIndex >= 0 && isAvailableFocusTarget(element));
  const first = focusable[0];
  const last = focusable[focusable.length - 1];
  const active = document.activeElement;
  if (!first) {
    event.preventDefault();
    surface.tabIndex = -1;
    surface.focus({ preventScroll: true });
  } else if (active === surface || !surface.contains(active) || (event.shiftKey ? active === first : active === last)) {
    event.preventDefault();
    (event.shiftKey ? last : first)?.focus({ preventScroll: true });
  }
}

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
    let shouldClose = true;
    try {
      shouldClose = topModal.canDismiss?.() !== false && topModal.close() !== false;
    } finally {
      if (shouldClose) {
        // 主动从栈中移除，不再依赖 close 回调自行调用 unregisterModal
        const idx = modalStack.value.findIndex(m => m.id === topModal.id);
        if (idx !== -1) removeModal(idx);
      } else {
        // 返回键已将当前 history entry 弹出；拒绝关闭时立即补回同一个
        // modal entry，否则下一次系统返回会绕过仍在屏幕上的页面。
        window.history.pushState({ vcpRoot: true, vcpModalId: topModal.id }, '');
      }
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

// Module initialization runs once; the handler reference owns later cleanup.
if (typeof window !== 'undefined') {
  popstateHandler = handlePopState;
  window.addEventListener('popstate', popstateHandler);
  window.addEventListener('keydown', handleModalKeydown, true);

  // Initial check: if we are at root, push the dummy state
  // Note: App.vue will call this again after router is ready to be 100% sure
  initRootHistory();
}

export function cleanupModalHistory() {
  if (popstateHandler && typeof window !== 'undefined') {
    window.removeEventListener('popstate', popstateHandler);
    window.removeEventListener('keydown', handleModalKeydown, true);
    popstateHandler = null;
  }
}

export function useModalHistory() {
  /**
   * Registers a modal as "open" and pushes a state to the history stack.
   * @param id Unique identifier for the modal
   * @param closeHandler Callback to close the modal (triggered by back gesture)
   */
  const registerModal = (id: string, closeHandler: () => boolean | void, surface?: () => HTMLElement | null, canDismiss?: () => boolean, canLeaveForNavigation?: () => boolean) => {
    // Avoid double registration
    const existing = modalStack.value.find(m => m.id === id);
    if (existing) {
      if (surface) existing.surface = surface;
      if (canDismiss) existing.canDismiss = canDismiss;
      if (canLeaveForNavigation) existing.canLeaveForNavigation = canLeaveForNavigation;
      return;
    }

    // Push state to history
    window.history.pushState({ vcpRoot: true, vcpModalId: id }, '');

    // Add to our LIFO stack
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    modalStack.value.push({ id, close: closeHandler, surface, canDismiss, canLeaveForNavigation, opener, route: window.location.hash });
    stackRevision += 1;
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

    removeModal(index);
  };

  const closeTopModal = (): boolean => {
    if (modalStack.value.length > 0) {
      const topModal = modalStack.value[modalStack.value.length - 1];
      
      console.log(`[ModalHistory] Closing top modal: ${topModal.id}`);

      try {
        closeModal(topModal);
      } catch (err) {
        console.error(`[ModalHistory] Failed to call close on modal ${topModal.id}:`, err);
      }
      
      return true;
    }
    return false;
  };

  return {
    registerModal,
    unregisterModal,
    modalStackLength: () => modalStack.value.length,
    // Some back handlers start an asynchronous settings save. Only a dedicated
    // read-only predicate can admit notification navigation past those surfaces.
    canCloseAllModals: () => modalStack.value.every(modal =>
      modal.canLeaveForNavigation ? modal.canLeaveForNavigation() : !modal.canDismiss),
    initRootHistory,
    closeTopModal
  };
}
