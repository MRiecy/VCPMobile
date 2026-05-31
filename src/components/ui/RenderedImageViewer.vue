<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { Download, Minus, Plus, RotateCcw, X } from "lucide-vue-next";
import { useModalHistory } from "../../core/composables/useModalHistory";
import { useNotificationStore } from "../../core/stores/notification";
import {
  useRenderedImageViewer,
  openRenderedImageViewer,
} from "../../core/composables/useRenderedImageViewer";

interface SandboxImageMessage {
  source?: string;
  type?: string;
  nonce?: string;
  image?: {
    src?: string;
    alt?: string;
    title?: string;
    fileName?: string;
    sourceLabel?: string;
  };
}

const { state, closeRenderedImageViewer } = useRenderedImageViewer();
const notificationStore = useNotificationStore();
const { registerModal, unregisterModal } = useModalHistory();

const zoom = ref(1);
const panX = ref(0);
const panY = ref(0);
const isSaving = ref(false);
const dragState = ref<{
  pointerId: number;
  startX: number;
  startY: number;
  panX: number;
  panY: number;
} | null>(null);
const modalId = "RenderedImageViewer";

const displayTitle = computed(
  () => state.title || state.alt || state.fileName || "AI 渲染图片",
);
const imageStyle = computed(() => ({
  transform: `translate3d(${panX.value}px, ${panY.value}px, 0) scale(${zoom.value})`,
  cursor: zoom.value > 1 ? (dragState.value ? "grabbing" : "grab") : "zoom-in",
}));

function resetView(): void {
  zoom.value = 1;
  panX.value = 0;
  panY.value = 0;
}

function zoomIn(): void {
  zoom.value = Math.min(5, Number((zoom.value + 0.5).toFixed(1)));
}

function zoomOut(): void {
  zoom.value = Math.max(1, Number((zoom.value - 0.5).toFixed(1)));
  if (zoom.value === 1) {
    panX.value = 0;
    panY.value = 0;
  }
}

function toggleZoom(): void {
  if (zoom.value > 1) {
    resetView();
  } else {
    zoom.value = 2.5;
  }
}

function handlePointerDown(event: PointerEvent): void {
  if (zoom.value <= 1) return;
  dragState.value = {
    pointerId: event.pointerId,
    startX: event.clientX,
    startY: event.clientY,
    panX: panX.value,
    panY: panY.value,
  };
  (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
}

function handlePointerMove(event: PointerEvent): void {
  const drag = dragState.value;
  if (!drag || drag.pointerId !== event.pointerId) return;
  panX.value = drag.panX + event.clientX - drag.startX;
  panY.value = drag.panY + event.clientY - drag.startY;
}

function handlePointerUp(event: PointerEvent): void {
  if (dragState.value?.pointerId === event.pointerId) {
    dragState.value = null;
  }
}

function normalizeFileName(raw: string): string {
  const name = raw.trim().replace(/[\\/:*?"<>|\u0000-\u001f]/g, "_");
  return name || "vcp-image";
}

function guessFileName(): string {
  const preferred = state.fileName || state.title || state.alt;
  if (preferred) return normalizeFileName(preferred);

  if (!state.src.startsWith("data:") && !state.src.startsWith("blob:")) {
    try {
      const parsed = new URL(state.src);
      const segment = parsed.pathname.split("/").filter(Boolean).pop();
      if (segment) return normalizeFileName(decodeURIComponent(segment));
    } catch {
      // fall through to timestamp fallback
    }
  }

  return `vcp-image-${Date.now()}`;
}

async function saveImage(): Promise<void> {
  if (!state.src || isSaving.value) return;
  isSaving.value = true;
  const fileName = guessFileName();

  try {
    // 1. 高速拉取原始二进制流 (不进行任何 base64 冗余转换)
    const response = await fetch(state.src);
    if (!response.ok) throw new Error(`HTTP 资源加载失败: ${response.status}`);
    const blob = await response.blob();
    
    // 2. 转换为 Uint8Array 原生字节数组，Tauri 可以以极低开销序列化传输它
    const arrayBuffer = await blob.arrayBuffer();
    const bytes = new Uint8Array(arrayBuffer);
    const u8Array = Array.from(bytes); // 适配 Rust Vec<u8> 反序列化

    // 3. 利用 Rust 侧文件写入权，将字节流写入本地沙箱 Cache 目录中（Zero-IPC Path）
    const tempFileName = `vcp_tmp_${Date.now()}.png`;
    const tempPath = await invoke<string>("plugin:vcp-mobile|write_temp_file", {
      bytes: u8Array,
      fileName: tempFileName,
    });

    // 4. JNI IPC 跨越：只传输这个 100 字节以内的物理路径字符串！
    await invoke("plugin:vcp-mobile|save_image_from_path", {
      imagePath: tempPath,
      fileName,
    });

    notificationStore.addNotification({
      type: "success",
      title: "保存成功",
      message: "图片已保存到相册",
      toastOnly: true,
    });
  } catch (error) {
    console.error("[RenderedImageViewer] Zero-IPC save failed:", error);
    notificationStore.addNotification({
      type: "error",
      title: "保存失败",
      message: String(error),
      toastOnly: true,
    });
  } finally {
    isSaving.value = false;
  }
}

function close(): void {
  closeRenderedImageViewer();
}

function trustedSandboxFrame(
  event: MessageEvent<SandboxImageMessage>,
): HTMLIFrameElement | null {
  if (event.origin !== "null" || !event.source) return null;

  const frames = document.querySelectorAll<HTMLIFrameElement>(
    "iframe[data-vcp-image-nonce]",
  );
  for (const frame of frames) {
    if (frame.contentWindow === event.source) {
      return frame;
    }
  }
  return null;
}

function handleSandboxMessage(event: MessageEvent<SandboxImageMessage>): void {
  const data = event.data;
  if (
    !data ||
    typeof data !== "object" ||
    data.source !== "vcp-mobile" ||
    data.type !== "rendered-image-click"
  )
    return;
  const frame = trustedSandboxFrame(event);
  if (!frame || !data.nonce || frame.dataset.vcpImageNonce !== data.nonce) {
    return;
  }
  const image = data.image;
  if (!image?.src) return;
  openRenderedImageViewer({
    src: image.src,
    alt: image.alt,
    title: image.title,
    fileName: image.fileName,
    sourceLabel: image.sourceLabel || "HTML 预览图片",
  });
}

watch(
  () => state.isOpen,
  (isOpen) => {
    if (isOpen) {
      resetView();
      registerModal(modalId, close);
    } else {
      unregisterModal(modalId);
      dragState.value = null;
    }
  },
);

onMounted(() => {
  window.addEventListener("message", handleSandboxMessage);
});

onUnmounted(() => {
  window.removeEventListener("message", handleSandboxMessage);
  unregisterModal(modalId);
});
</script>

<template>
  <Teleport to="body">
    <Transition name="rendered-image-viewer">
      <div
        v-if="state.isOpen"
        class="fixed inset-0 flex flex-col bg-[#05070a] text-white pointer-events-auto select-none z-viewer"
        @click.self="close"
      >
        <div
          class="flex items-center justify-between gap-3 px-4 pt-[calc(var(--vcp-safe-top,24px)+8px)] pb-3 bg-black/55 border-b border-white/10 shrink-0"
        >
          <div class="min-w-0">
            <div class="text-sm font-semibold truncate">{{ displayTitle }}</div>
            <div
              class="text-[10px] text-white/45 uppercase tracking-widest truncate"
            >
              {{ state.sourceLabel }}
            </div>
          </div>

          <div class="flex items-center gap-1 shrink-0">
            <button
              class="p-2 rounded-full text-white/70 active:bg-white/10 active:scale-95 transition"
              :disabled="zoom <= 1"
              @click="zoomOut"
            >
              <Minus :size="19" />
            </button>
            <button
              class="p-2 rounded-full text-white/70 active:bg-white/10 active:scale-95 transition"
              @click="zoomIn"
            >
              <Plus :size="19" />
            </button>
            <button
              class="p-2 rounded-full text-white/70 active:bg-white/10 active:scale-95 transition"
              @click="resetView"
            >
              <RotateCcw :size="19" />
            </button>
            <button
              class="p-2 rounded-full text-white/80 active:bg-white/10 active:scale-95 transition disabled:opacity-40"
              :disabled="isSaving"
              @click="saveImage"
            >
              <Download :size="20" />
            </button>
            <button
              class="p-2 -mr-2 rounded-full text-white/80 active:bg-white/10 active:scale-95 transition"
              @click="close"
            >
              <X :size="24" />
            </button>
          </div>
        </div>

        <div
          class="relative flex-1 overflow-hidden flex items-center justify-center px-2 py-4 pb-[calc(var(--vcp-safe-bottom,0px)+16px)]"
          @pointerdown="handlePointerDown"
          @pointermove="handlePointerMove"
          @pointerup="handlePointerUp"
          @pointercancel="handlePointerUp"
          @dblclick="toggleZoom"
        >
          <img
            :src="state.src"
            :alt="state.alt || displayTitle"
            class="max-w-full max-h-full object-contain rendered-image-viewer__image"
            :style="imageStyle"
            draggable="false"
            @click.stop="toggleZoom"
          />
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.rendered-image-viewer-enter-active,
.rendered-image-viewer-leave-active {
  transition:
    opacity 0.2s ease,
    transform 0.2s ease;
}

.rendered-image-viewer-enter-from,
.rendered-image-viewer-leave-to {
  opacity: 0;
  transform: scale(1.02);
}

.rendered-image-viewer__image {
  transform-origin: center center;
  transition: transform 0.16s ease;
  will-change: transform;
  touch-action: none;
}
</style>
