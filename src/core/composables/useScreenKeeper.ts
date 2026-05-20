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
