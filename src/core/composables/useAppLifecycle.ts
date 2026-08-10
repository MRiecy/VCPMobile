import { onMounted, onUnmounted, watch } from 'vue';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useAppLifecycleStore } from '../stores/appLifecycle';
import { useChatStreamStore } from '../stores/chatStreamStore';

export function useAppLifecycle() {
  const lifecycleStore = useAppLifecycleStore();
  const streamStore = useChatStreamStore();
  let unlisten: UnlistenFn | null = null;
  let foregroundRecoveryPromise: Promise<void> | null = null;

  // 检测是否是划词助手窗口
  const isAssistant = typeof window !== 'undefined' && window.location.search.includes("mode=floating");

  const scheduleForegroundRecovery = (source: string) => {
    if (foregroundRecoveryPromise) return foregroundRecoveryPromise;
    console.log(`[useAppLifecycle] ${source}. Triggering stream recovery.`);
    foregroundRecoveryPromise = streamStore.checkAndRecoverInterruptedStreams()
      .catch(err => {
        console.error(`[useAppLifecycle] Failed to recover streams (${source}):`, err);
      })
      .finally(() => {
        foregroundRecoveryPromise = null;
      });
    return foregroundRecoveryPromise;
  };

  // 监听后台状态，控制全局动画挂起，替代 App.vue 中的 watch
  watch(() => lifecycleStore.isBackground, (newVal) => {
    if (isAssistant) return;

    if (newVal) {
      document.documentElement.classList.add("vcp-paused-animations");
      console.log("[useAppLifecycle] App moved to background, pausing animations.");
    } else {
      document.documentElement.classList.remove("vcp-paused-animations");
      console.log("[useAppLifecycle] App moved to foreground, resuming animations.");
      lifecycleStore.hydrateSystemStatus().catch((err) => {
        console.error("[useAppLifecycle] Failed to hydrate system status:", err);
      });
    }
  }, { immediate: true });

  const handleVisibilityChange = () => {
    if (typeof document !== 'undefined') {
      const isHidden = document.hidden;
      lifecycleStore.isBackground = isHidden;
      console.log(`[useAppLifecycle] Visibility changed: hidden=${isHidden}`);
      if (!isHidden) {
        void scheduleForegroundRecovery("Document visible");
      }
    }
  };

  const handleOnline = () => {
    void scheduleForegroundRecovery("Device online");
  };

  onMounted(async () => {
    if (typeof window !== 'undefined') {
      document.addEventListener("visibilitychange", handleVisibilityChange);
      window.addEventListener("online", handleOnline);
    }

    try {
      unlisten = await listen<{ state: string }>("vcp-lifecycle-changed", (event) => {
        const state = event.payload.state;
        console.log(`[useAppLifecycle] Received vcp-lifecycle-changed: state=${state}`);
        
        if (state === 'pause' || state === 'stop') {
          lifecycleStore.isBackground = true;
        } else if (state === 'resume') {
          lifecycleStore.isBackground = false;
          void scheduleForegroundRecovery("Native resume");
        }
      });
    } catch (err) {
      console.error("[useAppLifecycle] Failed to setup Tauri lifecycle listener:", err);
    }
  });

  onUnmounted(() => {
    if (typeof window !== 'undefined') {
      document.removeEventListener("visibilitychange", handleVisibilityChange);
      window.removeEventListener("online", handleOnline);
    }
    if (unlisten) {
      unlisten();
    }
  });
}
