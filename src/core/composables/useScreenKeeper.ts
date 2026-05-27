import { setKeepScreenOn, clearKeepScreenOn } from 'tauri-plugin-vcp-mobile';

let refCount = 0;

/**
 * Acquire a screen-keep-on lock. The first acquisition actually calls
 * `setKeepScreenOn()`; subsequent calls only bump the ref count.
 */
export function acquireScreenKeep(): void {
  if (refCount === 0) {
    setKeepScreenOn().catch(() => {});
  }
  refCount++;
}

/**
 * Release a screen-keep-on lock. When the ref count drops to zero,
 * `clearKeepScreenOn()` is called.
 */
export function releaseScreenKeep(): void {
  if (refCount > 0) {
    refCount--;
    if (refCount === 0) {
      clearKeepScreenOn().catch(() => {});
    }
  }
}

/**
 * Wrap an async function with screen-keep-on lifecycle.
 * Automatically acquires before execution and releases in `finally`.
 */
export async function withScreenKeep<T>(fn: () => Promise<T>): Promise<T> {
  acquireScreenKeep();
  try {
    return await fn();
  } finally {
    releaseScreenKeep();
  }
}

/**
 * 亮屏 Activity 状态校准：当系统回到前台（Resume）时，
 * 如果当前逻辑引用计数仍 > 0，自动在新重建的 Activity 物理 Window 上施加亮屏标志。
 */
export function reapplyScreenKeepIfActive(): void {
  if (refCount > 0) {
    setKeepScreenOn().catch(() => {});
  } else {
    clearKeepScreenOn().catch(() => {});
  }
}

/**
 * 临时休眠物理亮屏：当系统退到后台（Pause/Stop）时，
 * 物理上清除亮屏状态以达到省电效果，但不清空引用计数，以便 Resume 时重构。
 */
export function suspendPhysicalScreenKeep(): void {
  clearKeepScreenOn().catch(() => {});
}
