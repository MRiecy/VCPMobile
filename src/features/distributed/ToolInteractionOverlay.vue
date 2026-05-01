<script setup lang="ts">
// ToolInteractionOverlay.vue
// Container for Interactive tool UIs (camera, biometric, etc.)
// Phase 3 skeleton — will be populated when Interactive tools are added.

import { ref, onMounted, onUnmounted } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

interface ToolUiRequest {
  tool: string;
  id: string;
  args?: Record<string, any>;
}

const activeRequest = ref<ToolUiRequest | null>(null);
let unlisten: UnlistenFn | null = null;

onMounted(async () => {
  // Listen for tool UI requests from the Rust backend
  unlisten = await listen<ToolUiRequest>("tool-ui-request", (event) => {
    activeRequest.value = event.payload;
  });
});

onUnmounted(() => {
  unlisten?.();
});

// Notification handler — listens for distributed-notification events
let unlistenNotification: UnlistenFn | null = null;

onMounted(async () => {
  unlistenNotification = await listen<{ title: string; body: string }>(
    "distributed-notification",
    (event) => {
      // Use browser Notification API or fallback
      if ("Notification" in window && Notification.permission === "granted") {
        new Notification(event.payload.title, { body: event.payload.body });
      } else {
        console.log(
          `[Distributed Notification] ${event.payload.title}: ${event.payload.body}`,
        );
      }
    },
  );
});

onUnmounted(() => {
  unlistenNotification?.();
});

// Clipboard write handler
let unlistenClipboard: UnlistenFn | null = null;

onMounted(async () => {
  unlistenClipboard = await listen<{ content: string }>(
    "distributed-clipboard-write",
    async (event) => {
      try {
        await navigator.clipboard.writeText(event.payload.content);
        console.log("[Distributed Clipboard] Content written.");
      } catch (e) {
        console.error("[Distributed Clipboard] Write failed:", e);
      }
    },
  );
});

onUnmounted(() => {
  unlistenClipboard?.();
});
</script>

<template>
  <!-- Interactive tool UI overlay — shown when a tool needs user interaction -->
  <Teleport to="#vcp-feature-overlays">
    <Transition name="fade">
      <div
        v-if="activeRequest"
        class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm"
      >
        <div
          class="bg-secondary-bg rounded-2xl p-6 mx-4 max-w-sm w-full shadow-2xl"
        >
          <div class="text-center space-y-3">
            <div class="text-lg font-bold text-primary-text">
              工具请求: {{ activeRequest.tool }}
            </div>
            <div class="text-sm opacity-60">
              此工具需要您的操作才能完成。
            </div>
            <!-- Phase 3+: tool-specific UI components will be rendered here -->
            <!-- e.g. <CameraCapture v-if="activeRequest.tool === 'camera'" /> -->
            <!-- e.g. <BiometricPrompt v-if="activeRequest.tool === 'biometric'" /> -->
            <div class="pt-4">
              <button
                class="px-4 py-2 bg-white/10 rounded-lg text-sm active:scale-95 transition-transform"
                @click="activeRequest = null"
              >
                取消
              </button>
            </div>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>
