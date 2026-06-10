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

const containerRef = ref<HTMLElement | null>(null);
const imageRef = ref<HTMLImageElement | null>(null);

const zoom = ref(1);
const panX = ref(0);
const panY = ref(0);
const isSaving = ref(false);
const isAnimating = ref(false);
const modalId = "RenderedImageViewer";

let touchStartDist = 0;
let touchStartZoom = 1;
let touchStartPanX = 0;
let touchStartPanY = 0;
let touchStartMidX = 0;
let touchStartMidY = 0;

let singleTouchStartX = 0;
let singleTouchStartY = 0;
let lastTapTime = 0;

type GestureMode = "none" | "pan" | "pinch";
const gestureMode = ref<GestureMode>("none");

const displayTitle = computed(
  () => state.title || state.alt || state.fileName || "AI 渲染图片",
);
const imageStyle = computed(() => ({
  transform: `translate3d(${panX.value}px, ${panY.value}px, 0) scale(${zoom.value})`,
  transition: isAnimating.value ? "transform 0.25s cubic-bezier(0.16, 1, 0.3, 1)" : "none",
}));

function getNaturalImageRenderSize() {
  if (!imageRef.value || !containerRef.value) return { width: 0, height: 0 };
  const img = imageRef.value;
  const container = containerRef.value;

  const cw = container.clientWidth;
  const ch = container.clientHeight;
  const iw = img.naturalWidth;
  const ih = img.naturalHeight;

  if (!iw || !ih) return { width: cw, height: ch };

  const containerRatio = cw / ch;
  const imageRatio = iw / ih;

  let renderWidth = cw;
  let renderHeight = ch;

  if (imageRatio > containerRatio) {
    renderHeight = cw / imageRatio;
  } else {
    renderWidth = ch * imageRatio;
  }

  return { width: renderWidth, height: renderHeight };
}

function clampPan(animate = true): void {
  if (!containerRef.value || !imageRef.value) return;

  const { width: iw, height: ih } = getNaturalImageRenderSize();
  const cw = containerRef.value.clientWidth;
  const ch = containerRef.value.clientHeight;

  const scaledW = iw * zoom.value;
  const scaledH = ih * zoom.value;

  let minPanX = 0;
  let maxPanX = 0;
  let minPanY = 0;
  let maxPanY = 0;

  if (scaledW > cw) {
    maxPanX = (scaledW - cw) / 2;
    minPanX = -maxPanX;
  }
  if (scaledH > ch) {
    maxPanY = (scaledH - ch) / 2;
    minPanY = -maxPanY;
  }

  const targetPanX = Math.min(maxPanX, Math.max(minPanX, panX.value));
  const targetPanY = Math.min(maxPanY, Math.max(minPanY, panY.value));

  if (targetPanX !== panX.value || targetPanY !== panY.value) {
    if (animate) {
      isAnimating.value = true;
      panX.value = targetPanX;
      panY.value = targetPanY;
      setTimeout(() => {
        isAnimating.value = false;
      }, 250);
    } else {
      panX.value = targetPanX;
      panY.value = targetPanY;
    }
  }
}

function limitPanDuringMove(px: number, py: number, currentZoom: number) {
  if (!containerRef.value || !imageRef.value) return { x: px, y: py };

  const { width: iw, height: ih } = getNaturalImageRenderSize();
  const cw = containerRef.value.clientWidth;
  const ch = containerRef.value.clientHeight;

  const scaledW = iw * currentZoom;
  const scaledH = ih * currentZoom;

  let maxX = scaledW > cw ? (scaledW - cw) / 2 : 0;
  let maxY = scaledH > ch ? (scaledH - ch) / 2 : 0;

  const extraX = cw * 0.3;
  const extraY = ch * 0.3;

  const limitX = maxX + extraX;
  const limitY = maxY + extraY;

  return {
    x: Math.min(limitX, Math.max(-limitX, px)),
    y: Math.min(limitY, Math.max(-limitY, py)),
  };
}

function resetView(): void {
  isAnimating.value = true;
  gestureMode.value = "none";
  zoom.value = 1;
  panX.value = 0;
  panY.value = 0;
  setTimeout(() => {
    isAnimating.value = false;
  }, 250);
}

function zoomIn(): void {
  isAnimating.value = true;
  zoom.value = Math.min(5, Number((zoom.value + 0.5).toFixed(1)));
  clampPan(false);
  setTimeout(() => {
    isAnimating.value = false;
  }, 250);
}

function zoomOut(): void {
  isAnimating.value = true;
  zoom.value = Math.max(1, Number((zoom.value - 0.5).toFixed(1)));
  if (zoom.value === 1) {
    panX.value = 0;
    panY.value = 0;
  } else {
    clampPan(false);
  }
  setTimeout(() => {
    isAnimating.value = false;
  }, 250);
}

function handleDoubleClick(event: MouseEvent | TouchEvent): void {
  isAnimating.value = true;
  if (zoom.value > 1) {
    resetView();
  } else {
    const targetZoom = 2.5;
    let clickX = 0;
    let clickY = 0;

    if (window.TouchEvent && event instanceof TouchEvent) {
      if (event.touches.length > 0) {
        clickX = event.touches[0].clientX;
        clickY = event.touches[0].clientY;
      } else if (event.changedTouches.length > 0) {
        clickX = event.changedTouches[0].clientX;
        clickY = event.changedTouches[0].clientY;
      }
    } else if (event instanceof MouseEvent) {
      clickX = event.clientX;
      clickY = event.clientY;
    }

    if (clickX && clickY && containerRef.value) {
      const rect = containerRef.value.getBoundingClientRect();
      const centerX = rect.left + rect.width / 2;
      const centerY = rect.top + rect.height / 2;

      const offsetX = clickX - centerX;
      const offsetY = clickY - centerY;

      zoom.value = targetZoom;
      panX.value = -offsetX * (targetZoom - 1);
      panY.value = -offsetY * (targetZoom - 1);

      clampPan(false);
    } else {
      zoom.value = targetZoom;
    }
  }
  setTimeout(() => {
    isAnimating.value = false;
  }, 250);
}

function handleTouchStart(event: TouchEvent): void {
  isAnimating.value = false;

  const now = Date.now();
  if (event.touches.length === 1 && gestureMode.value === "none") {
    if (now - lastTapTime < 300) {
      if (event.cancelable) event.preventDefault();
      handleDoubleClick(event);
      lastTapTime = 0;
      return;
    }
    lastTapTime = now;
  }

  if (event.touches.length === 1) {
    if (gestureMode.value === "none") {
      gestureMode.value = "pan";
      const t = event.touches[0];
      singleTouchStartX = t.clientX;
      singleTouchStartY = t.clientY;
      touchStartPanX = panX.value;
      touchStartPanY = panY.value;
    }
  } else if (event.touches.length >= 2) {
    gestureMode.value = "pinch";
    const t1 = event.touches[0];
    const t2 = event.touches[1];
    const d = Math.hypot(t2.clientX - t1.clientX, t2.clientY - t1.clientY);
    touchStartDist = d > 20 ? d : 20; // 避免初始距离过小发生抖动
    touchStartZoom = zoom.value;
    touchStartMidX = (t1.clientX + t2.clientX) / 2;
    touchStartMidY = (t1.clientY + t2.clientY) / 2;
    touchStartPanX = panX.value;
    touchStartPanY = panY.value;
  }
}

function handleTouchMove(event: TouchEvent): void {
  if (zoom.value > 1 || event.touches.length >= 2 || gestureMode.value !== "none") {
    if (event.cancelable) event.preventDefault();
  }

  if (gestureMode.value === "pan") {
    if (event.touches.length === 1 && zoom.value > 1) {
      const t = event.touches[0];
      const deltaX = t.clientX - singleTouchStartX;
      const deltaY = t.clientY - singleTouchStartY;
      const targetX = touchStartPanX + deltaX;
      const targetY = touchStartPanY + deltaY;

      const limited = limitPanDuringMove(targetX, targetY, zoom.value);
      panX.value = limited.x;
      panY.value = limited.y;
    }
  } else if (gestureMode.value === "pinch") {
    if (event.touches.length >= 2) {
      const t1 = event.touches[0];
      const t2 = event.touches[1];
      const dist = Math.hypot(t2.clientX - t1.clientX, t2.clientY - t1.clientY);

      if (touchStartDist > 20 && containerRef.value) {
        const sensitivity = 0.6; // 减小缩放灵敏度，使体验更平滑
        const rawFactor = dist / touchStartDist;
        const factor = 1 + (rawFactor - 1) * sensitivity;

        let newZoom = touchStartZoom * factor;
        newZoom = Math.min(6, Math.max(0.8, newZoom));

        const midX = (t1.clientX + t2.clientX) / 2;
        const midY = (t1.clientY + t2.clientY) / 2;

        zoom.value = newZoom;

        // 获取视口/画布中心坐标（偏移基准）
        const cw = containerRef.value.clientWidth;
        const ch = containerRef.value.clientHeight;
        const vcx = cw / 2;
        const vcy = ch / 2;

        // 物理精确的以双指中点为轴心的缩放平移算法
        const targetX = midX - vcx - factor * (touchStartMidX - vcx - touchStartPanX);
        const targetY = midY - vcy - factor * (touchStartMidY - vcy - touchStartPanY);

        // 引入实时滑动范围硬性保护边界约束，杜绝飞图
        const limited = limitPanDuringMove(targetX, targetY, newZoom);
        panX.value = limited.x;
        panY.value = limited.y;
      }
    }
  }
}

function handleTouchEnd(event: TouchEvent): void {
  if (event.touches.length === 0) {
    gestureMode.value = "none";
    if (zoom.value < 1.0) {
      resetView();
    } else if (zoom.value > 5.0) {
      isAnimating.value = true;
      zoom.value = 5.0;
      clampPan(false);
      setTimeout(() => {
        isAnimating.value = false;
      }, 250);
    } else {
      clampPan(true);
    }
  } else if (event.touches.length === 1) {
    gestureMode.value = "pan";
    const t = event.touches[0];
    singleTouchStartX = t.clientX;
    singleTouchStartY = t.clientY;
    touchStartPanX = panX.value;
    touchStartPanY = panY.value;
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
    let sourceUrl = state.src;

    // 如果是 blob 链接，由于 Android 底层无法直接读取 WebView 的 blob 协议内存数据，
    // 我们在前端将其转换为 data:url (Base64) 字符串。
    // 这在 JS 中是异步且高效的，不需要转为普通 JSON 数组，避免了 GC 和序列化开销。
    if (state.src.startsWith("blob:")) {
      const response = await fetch(state.src);
      const blob = await response.blob();
      sourceUrl = await new Promise<string>((resolve, reject) => {
        const reader = new FileReader();
        reader.onloadend = () => resolve(reader.result as string);
        reader.onerror = reject;
        reader.readAsDataURL(blob);
      });
    }

    // 直接调用原生插件提供的 save_image_to_gallery 接口，仅传输几十字节的 URL 或 Base64 字符串
    await invoke("plugin:vcp-mobile|save_image_to_gallery", {
      sourceUrl,
      fileName,
    });

    notificationStore.addNotification({
      type: "success",
      title: "保存成功",
      message: "图片已保存到相册",
      toastOnly: true,
    });
  } catch (error) {
    console.error("[RenderedImageViewer] Save image failed:", error);
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
      touchStartDist = 0;
      lastTapTime = 0;
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
          ref="containerRef"
          class="relative flex-1 overflow-hidden flex items-center justify-center px-2 py-4 pb-[calc(var(--vcp-safe-bottom,0px)+16px)]"
          @touchstart="handleTouchStart"
          @touchmove="handleTouchMove"
          @touchend="handleTouchEnd"
          @touchcancel="handleTouchEnd"
        >
          <img
            ref="imageRef"
            :src="state.src"
            :alt="state.alt || displayTitle"
            class="max-w-full max-h-full object-contain rendered-image-viewer__image"
            :style="imageStyle"
            draggable="false"
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
  will-change: transform;
  touch-action: none;
}
</style>
