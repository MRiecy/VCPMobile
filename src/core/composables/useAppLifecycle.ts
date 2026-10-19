import { onMounted, onUnmounted, ref, watch } from 'vue';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useAppLifecycleStore } from '../stores/appLifecycle';
import { useChatStreamStore } from '../stores/chatStreamStore';
import { isIOS } from '../utils/platform';
import type { LifecycleSnapshotDto } from '../types/lifecycle';
import { createVoiceHistorySync } from '../../features/chat/voice/voiceHistorySync';

export function useAppLifecycle() {
  const lifecycleStore = useAppLifecycleStore();
  const streamStore = useChatStreamStore();
  const voiceHistory = createVoiceHistorySync();
  let unlisten: UnlistenFn | null = null;
  let unlistenNetwork: UnlistenFn | null = null;
  let recoveryPromise: Promise<void> | null = null;
  let disposed = false;
  const documentHidden = ref(typeof document !== 'undefined' && document.hidden);

  const reconcileInterruptedStreams = (source: string) => {
    if (disposed || lifecycleStore.state !== 'READY' || documentHidden.value) return;
    const phase = lifecycleStore.lifecycleSnapshot?.phase;
    // iOS must receive its first native snapshot before starting foreground recovery.
    if (isIOS && !phase) return;
    if (phase && phase !== 'active') return;
    if (recoveryPromise) return recoveryPromise;
    console.log(`[useAppLifecycle] ${source}. Triggering stream recovery.`);
    recoveryPromise = Promise.allSettled([
      streamStore.checkAndRecoverInterruptedStreams(),
      voiceHistory.reconcile(),
    ]).then(results => {
      for (const result of results) {
        if (result.status === 'rejected') {
          console.error(`[useAppLifecycle] Recovery failed (${source}):`, result.reason);
        }
      }
    })
      .catch(err => {
        console.error(`[useAppLifecycle] Failed to recover streams (${source}):`, err);
      })
      .finally(() => {
        recoveryPromise = null;
      });
    return recoveryPromise;
  };

  // 中断流恢复属于应用生命周期调和：冷启动、WebView 重载均由 READY 转换覆盖。
  watch(
    () => lifecycleStore.state,
    (state, previous) => {
      if (state === 'READY' && previous !== 'READY') {
        void reconcileInterruptedStreams('Core ready');
      }
    },
  );

  // Browser visibility only pauses rendering. It never overwrites the native phase.
  watch([() => lifecycleStore.lifecycleSnapshot?.phase, documentHidden], ([phase, hidden]) => {
    const paused = hidden || (phase !== undefined && phase !== 'active');
    document.documentElement.classList.toggle('vcp-paused-animations', paused);
  }, { immediate: true });

  watch(() => lifecycleStore.lifecycleSnapshot?.phase, (phase, previous) => {
    if (phase === 'active' && previous !== 'active') {
      lifecycleStore.hydrateSystemStatus().catch((err) => {
        console.error("[useAppLifecycle] Failed to hydrate system status:", err);
      });
      void reconcileInterruptedStreams('App foreground');
    }
  });

  const handleVisibilityChange = () => {
    if (typeof document !== 'undefined') {
      documentHidden.value = document.hidden;
      if (!documentHidden.value) void reconcileInterruptedStreams('Document visible');
    }
  };

  const handleOnline = () => {
    void reconcileInterruptedStreams("Device online");
  };

  onMounted(async () => {
    if (typeof window !== 'undefined') {
      document.addEventListener("visibilitychange", handleVisibilityChange);
      window.addEventListener("online", handleOnline);
    }

    // iOS NWPath observations feed the same recovery owner as browser online.
    // A satisfied path is a retry hint, not proof that the configured server is reachable.
    void listen<{ connected: boolean }>('vcp-network-changed', (event) => {
      if (event.payload.connected === true) {
        void reconcileInterruptedStreams('Native network online');
      }
    }).then((stopListening) => {
      if (disposed) stopListening();
      else unlistenNetwork = stopListening;
    }).catch((err) => {
      console.error('[useAppLifecycle] Failed to setup native network listener:', err);
    });

    try {
      const stopListening = await listen<LifecycleSnapshotDto>('vcp-lifecycle-changed', (event) => {
        if (!disposed) lifecycleStore.applyLifecycleSnapshot(event.payload);
      });
      if (disposed) {
        stopListening();
        return;
      }
      unlisten = stopListening;
      // Subscribe first, then replay the owner snapshot. Epoch ordering handles races.
      await lifecycleStore.hydrateSystemStatus();
    } catch (err) {
      console.error("[useAppLifecycle] Failed to setup Tauri lifecycle listener:", err);
    }
  });

  onUnmounted(() => {
    disposed = true;
    voiceHistory.dispose();
    if (typeof window !== 'undefined') {
      document.removeEventListener("visibilitychange", handleVisibilityChange);
      window.removeEventListener("online", handleOnline);
    }
    if (unlisten) {
      unlisten();
    }
    if (unlistenNetwork) {
      unlistenNetwork();
    }
  });
}
