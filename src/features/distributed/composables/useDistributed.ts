// features/distributed/composables/useDistributed.ts
// Plugin center connection status owner. It consumes the Rust snapshot command and
// status event without importing global settings or unrelated feature stores.

import { ref, readonly, onUnmounted } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface DistributedStatus {
  state: "disconnected" | "connecting" | "connected" | "disconnecting";
  connected: boolean;
  server_id: string | null;
  client_id: string | null;
  registered_tools: number;
  last_error: string | null;
  session_id: number;
}

const status = ref<DistributedStatus>({
  state: "disconnected",
  connected: false,
  server_id: null,
  client_id: null,
  registered_tools: 0,
  last_error: null,
  session_id: 0,
});

let pendingListener: {
  generation: number;
  promise: Promise<void>;
} | null = null;
let unlisten: UnlistenFn | null = null;
let listenerCount = 0;
let listenerGeneration = 0;
let statusEventRevision = 0;

async function setupListener(generation: number) {
  if (unlisten || listenerCount <= 0 || generation !== listenerGeneration) {
    return;
  }
  if (!pendingListener || pendingListener.generation !== generation) {
    const registration = listen<DistributedStatus>(
      "vcp-distributed-status",
      (event) => {
        if (
          listenerCount <= 0 ||
          generation !== listenerGeneration ||
          event.payload.session_id < status.value.session_id
        ) {
          return;
        }
        console.log(
          "[Distributed] State transition:",
          JSON.stringify(event.payload),
        );
        statusEventRevision++;
        status.value = event.payload;
      },
    );
    let owner!: { generation: number; promise: Promise<void> };
    const promise = registration
      .then((resolvedUnlisten) => {
        if (listenerCount <= 0 || generation !== listenerGeneration) {
          resolvedUnlisten();
          return;
        }
        // 只有创建 registration 的 owner 可安装/释放这个 handle。
        // 其他消费者只 await 同一 promise，不得对同一 unlisten 二次处置。
        if (!unlisten) {
          unlisten = resolvedUnlisten;
        } else {
          resolvedUnlisten();
        }
      })
      .finally(() => {
        if (pendingListener === owner) {
          pendingListener = null;
        }
      });
    owner = { generation, promise };
    pendingListener = owner;
  }

  const owner = pendingListener;
  if (!owner) {
    return;
  }
  await owner.promise;
}

function teardownListener() {
  if (unlisten && listenerCount <= 0) {
    unlisten();
    unlisten = null;
  }
}

export function useDistributed() {
  const isThisInstanceActive = ref(false);

  function releaseListenerReference() {
    listenerCount = Math.max(0, listenerCount - 1);
    if (listenerCount === 0) {
      listenerGeneration++;
      teardownListener();
    }
  }

  async function activate() {
    if (isThisInstanceActive.value) return;
    isThisInstanceActive.value = true;
    listenerCount++;
    const generation = listenerGeneration;
    try {
      await setupListener(generation);
    } catch (error) {
      if (isThisInstanceActive.value && generation === listenerGeneration) {
        isThisInstanceActive.value = false;
        releaseListenerReference();
      }
      console.warn("[useDistributed] Failed to listen for status:", error);
      return;
    }
    if (!isThisInstanceActive.value || generation !== listenerGeneration) {
      return;
    }
    // Fetch initial status
    await refreshStatus(generation);
  }

  function deactivate() {
    if (!isThisInstanceActive.value) return;
    isThisInstanceActive.value = false;
    releaseListenerReference();
  }

  onUnmounted(() => {
    if (isThisInstanceActive.value) {
      deactivate();
    }
  });

  async function refreshStatus(generation = listenerGeneration): Promise<void> {
    const eventRevision = statusEventRevision;
    try {
      const s = await invoke<DistributedStatus>("get_distributed_status");
      if (
        listenerCount <= 0 ||
        generation !== listenerGeneration ||
        statusEventRevision !== eventRevision ||
        s.session_id < status.value.session_id
      ) {
        return;
      }
      status.value = s;
    } catch (e) {
      console.warn("[useDistributed] Failed to get status:", e);
    }
  }

  return {
    status: readonly(status),
    activate,
    deactivate,
    refreshStatus,
  };
}
