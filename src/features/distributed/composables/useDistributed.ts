// features/distributed/composables/useDistributed.ts
// Self-contained composable for distributed node state.
// Does NOT import chatManager, assistant, or any other existing store.
// Only reads 2 fields from settings (vcpLogUrl, vcpLogKey) for server URL reuse.

import { ref, readonly, onMounted, onUnmounted } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface DistributedStatus {
  connected: boolean;
  server_id: string | null;
  client_id: string | null;
  registered_tools: number;
  last_error: string | null;
}

const status = ref<DistributedStatus>({
  connected: false,
  server_id: null,
  client_id: null,
  registered_tools: 0,
  last_error: null,
});

const loading = ref(false);

let unlisten: UnlistenFn | null = null;
let listenerCount = 0;

async function setupListener() {
  if (unlisten) return;
  unlisten = await listen<DistributedStatus>(
    "vcp-distributed-status",
    (event) => {
      status.value = event.payload;
    },
  );
}

function teardownListener() {
  if (unlisten && listenerCount <= 0) {
    unlisten();
    unlisten = null;
  }
}

export function useDistributed() {
  onMounted(() => {
    listenerCount++;
    setupListener();
    // Fetch initial status
    refreshStatus();
  });

  onUnmounted(() => {
    listenerCount--;
    if (listenerCount <= 0) {
      teardownListener();
    }
  });

  async function start(
    wsUrl: string,
    vcpKey: string,
    deviceName: string,
  ): Promise<void> {
    loading.value = true;
    status.value.last_error = null;
    try {
      await invoke("start_distributed_node", {
        wsUrl,
        vcpKey,
        deviceName,
      });
    } catch (e: any) {
      status.value.last_error = e.toString();
      throw e;
    } finally {
      loading.value = false;
    }
  }

  async function stop(): Promise<void> {
    loading.value = true;
    try {
      await invoke("stop_distributed_node");
      status.value = {
        connected: false,
        server_id: null,
        client_id: null,
        registered_tools: 0,
        last_error: null,
      };
    } catch (e: any) {
      status.value.last_error = e.toString();
    } finally {
      loading.value = false;
    }
  }

  async function refreshStatus(): Promise<void> {
    try {
      const s = await invoke<DistributedStatus>("get_distributed_status");
      status.value = s;
    } catch (e) {
      console.warn("[useDistributed] Failed to get status:", e);
    }
  }

  return {
    status: readonly(status),
    loading: readonly(loading),
    start,
    stop,
    refreshStatus,
  };
}
