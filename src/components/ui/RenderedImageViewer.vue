<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { saveImageToGallery } from 'tauri-plugin-vcp-mobile';
import { exportSvg, IMAGE_EXPORT_MAX_BYTES, readBoundedImageBlob, withImageExportQueue } from '../../core/utils/renderedImageExport';
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
  requestId?: string;
  error?: string;
  image?: {
    src?: string;
    bytes?: ArrayBuffer;
    mime?: string;
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
const surfaceRef = ref<HTMLElement | null>(null);
const displaySrc = ref('');
const preparationError = ref('');
const isPreparing = ref(false);
interface PreparedImage { path: string; displayPath: string; mime: string; size: number; width: number; height: number; svgMarkup?: string }
interface ImageLease { image: PreparedImage; temporaryPath?: string; users: number }
let currentLease: ImageLease | null = null;
let preparationController: AbortController | null = null;

async function releaseLease(lease: ImageLease) {
  if (--lease.users !== 0) return;
  await invoke('release_rendered_image', { path: lease.image.path, displayPath: lease.image.displayPath }).catch(console.warn);
  if (lease.temporaryPath) await invoke('release_temporary_export', { path: lease.temporaryPath }).catch(console.warn);
}

watch(() => state.revision, async () => {
  preparationController?.abort();
  const previous = currentLease;
  currentLease = null;
  displaySrc.value = '';
  if (previous) void releaseLease(previous);
  preparationError.value = '';
  isPreparing.value = false;
  if (!state.isOpen || !state.src) return;
  const controller = new AbortController();
  preparationController = controller;
  const revision = state.revision;
  let source = state.src;
  let temporaryPath: string | undefined;
  let prepared: PreparedImage | undefined;
  isPreparing.value = true;
  try {
    if (/^(blob:|data:)/i.test(source)) {
      let blob = await readBoundedImageBlob(source, controller.signal);
      if (blob.type.split(';')[0] === 'image/svg+xml') {
        const svg = new DOMParser().parseFromString(await blob.text(), 'image/svg+xml').documentElement;
        if (svg.localName !== 'svg' || svg.querySelector('parsererror')) throw new Error('SVG 图片格式无效');
        blob = await exportSvg(svg as unknown as SVGSVGElement, controller.signal);
      }
      const bytes = new Uint8Array(await blob.arrayBuffer());
      if (controller.signal.aborted) return;
      const temporary = await invoke<{path: string}>('store_temporary_export', {
        fileBytes: Array.from(bytes), originalName: state.fileName || 'rendered-image',
      });
      temporaryPath = source = temporary.path;
    }
    if (controller.signal.aborted) return;
    prepared = await invoke<PreparedImage>('prepare_rendered_image', { source });
    if (prepared.svgMarkup && !controller.signal.aborted) {
      const vector = prepared;
      prepared = undefined;
      try {
        const svg = new DOMParser().parseFromString(vector.svgMarkup!, 'image/svg+xml').documentElement;
        if (svg.localName !== 'svg' || svg.querySelector('parsererror')) throw new Error('SVG 图片格式无效');
        const blob = await exportSvg(svg as unknown as SVGSVGElement, controller.signal);
        const bytes = new Uint8Array(await blob.arrayBuffer());
        if (controller.signal.aborted) return;
        const raster = await invoke<{path: string}>('store_temporary_export', { fileBytes: Array.from(bytes), originalName: 'rendered-svg.png' });
        if (temporaryPath) await invoke('release_temporary_export', { path: temporaryPath }).catch(console.warn);
        temporaryPath = raster.path;
        if (controller.signal.aborted) return;
        prepared = await invoke<PreparedImage>('prepare_rendered_image', { source: raster.path });
      } finally {
        await invoke('release_rendered_image', { path: vector.path, displayPath: vector.displayPath }).catch(console.warn);
      }
    }
    if (controller.signal.aborted || state.revision !== revision) return;
    currentLease = { image: prepared, temporaryPath, users: 1 };
    displaySrc.value = convertFileSrc(prepared.displayPath);
    // The lease now owns both resources, including the retained original for saving.
    prepared = undefined;
    temporaryPath = undefined;
  } catch (error) {
    if (!controller.signal.aborted) preparationError.value = String(error);
  } finally {
    if (prepared) await invoke('release_rendered_image', { path: prepared.path, displayPath: prepared.displayPath }).catch(console.warn);
    if (temporaryPath) await invoke('release_temporary_export', { path: temporaryPath }).catch(console.warn);
    if (preparationController === controller) isPreparing.value = false;
  }
}, { immediate: true });

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
    } catch (e) {
      // 容错：解析 URL 失败时回退到时间戳命名，记录 debug 日志
      console.debug('[RenderedImageViewer] Failed to parse URL for file name:', e);
    }
  }

  return `vcp-image-${Date.now()}`;
}

async function saveImage(): Promise<void> {
  const lease = currentLease;
  if (!lease || isSaving.value) return;
  lease.users++;
  isSaving.value = true;
  const requestedName = guessFileName();
  const fileName = lease.image.mime === 'image/png' && /\.svg$/i.test(requestedName) ? requestedName.replace(/\.svg$/i, '.png') : requestedName;

  try {
    await saveImageToGallery(lease.image.path, fileName);

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
    await releaseLease(lease);
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

interface SandboxExportSlot {
  frame: HTMLIFrameElement;
  granted: boolean;
  controller: AbortController;
  finish: () => void;
}
const sandboxSlots = new Map<string, SandboxExportSlot>();

function requestSandboxSlot(frame: HTMLIFrameElement, nonce: string, requestId: string) {
  if (!requestId || requestId.length > 100 || [...sandboxSlots.values()].some(slot => slot.frame === frame)) return;
  const key = `${nonce}:${requestId}`;
  const controller = new AbortController();
  let complete!: () => void;
  const completion = new Promise<void>(resolve => { complete = resolve; });
  const slot: SandboxExportSlot = { frame, granted: false, controller, finish: complete };
  const timer = setTimeout(() => {
    if (!slot.granted) { controller.abort(); complete(); }
    else frame.contentWindow?.postMessage({ source: 'vcp-mobile', type: 'rendered-image-cancel', nonce, requestId }, '*');
    // A granted toBlob has no cancellation API; its terminal reply releases the slot.
  }, 30000);
  const observer = new MutationObserver(() => {
    if (!frame.isConnected || frame.dataset.vcpImageNonce !== nonce) { controller.abort(); complete(); }
  });
  observer.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['data-vcp-image-nonce'] });
  sandboxSlots.set(key, slot);
  void withImageExportQueue(async () => {
    if (!frame.isConnected || frame.dataset.vcpImageNonce !== nonce) return;
    slot.granted = true;
    frame.contentWindow?.postMessage({ source: 'vcp-mobile', type: 'rendered-image-grant', nonce, requestId }, '*');
    await completion;
  }, controller.signal).catch(() => {}).finally(() => {
    clearTimeout(timer);
    observer.disconnect();
    sandboxSlots.delete(key);
  });
}

function handleSandboxMessage(event: MessageEvent<SandboxImageMessage>): void {
  const data = event.data;
  if (
    !data ||
    typeof data !== "object" ||
    data.source !== "vcp-mobile" ||
    !['rendered-image-click', 'rendered-image-error', 'rendered-image-request'].includes(data.type || '')
  )
    return;
  const frame = trustedSandboxFrame(event);
  if (!frame || !data.nonce || frame.dataset.vcpImageNonce !== data.nonce) {
    return;
  }
  if (data.type === 'rendered-image-request') {
    requestSandboxSlot(frame, data.nonce, data.requestId || '');
    return;
  }
  const slot = sandboxSlots.get(`${data.nonce}:${data.requestId}`);
  if (!slot || slot.frame !== frame) return;
  slot.finish();
  if (!slot.granted) slot.controller.abort();
  if (!slot.granted || slot.controller.signal.aborted) return;
  if (data.type === 'rendered-image-error') {
    notificationStore.addNotification({ type: 'error', title: '图片预览失败', message: String(data.error || '无法取得当前预览图片').slice(0, 500), toastOnly: true });
    return;
  }
  const image = data.image;
  if (!image) return;
  let src = image.src || '';
  let ownedObjectUrl = false;
  if (image.bytes instanceof ArrayBuffer) {
    if (!image.bytes.byteLength || image.bytes.byteLength > IMAGE_EXPORT_MAX_BYTES || !/^image\//i.test(image.mime || '')) return;
    src = URL.createObjectURL(new Blob([image.bytes], { type: image.mime }));
    ownedObjectUrl = true;
  } else {
    // A blob belongs to its opaque document and must arrive as bounded bytes.
    if (!/^(https?:\/\/|asset:|file:)/i.test(src)) return;
  }
  openRenderedImageViewer({
    src,
    ownedObjectUrl,
    alt: image.alt,
    title: image.title,
    fileName: image.fileName,
    sourceLabel: image.sourceLabel || "HTML 预览图片",
  });
  frame.contentWindow?.postMessage({ source: 'vcp-mobile', type: 'rendered-image-received', nonce: data.nonce, requestId: data.requestId }, '*');
}

watch(
  () => state.isOpen,
  (isOpen) => {
    if (isOpen) {
      resetView();
      registerModal(modalId, close, () => surfaceRef.value);
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
  for (const slot of sandboxSlots.values()) { slot.controller.abort(); slot.finish(); }
  preparationController?.abort();
  if (currentLease) void releaseLease(currentLease);
  currentLease = null;
  closeRenderedImageViewer();
  window.removeEventListener("message", handleSandboxMessage);
  unregisterModal(modalId);
});
</script>

<template>
  <Teleport to="body">
    <Transition name="rendered-image-viewer">
      <div
        v-if="state.isOpen"
        ref="surfaceRef"
        class="rendered-image-viewer vcp-viewport-surface vcp-safe-inline fixed inset-0 flex flex-col bg-[#05070a] text-white pointer-events-auto select-none z-viewer"
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
              :disabled="isSaving || isPreparing || !displaySrc"
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
          class="relative flex-1 overflow-hidden flex items-center justify-center px-2 py-4 pb-[calc(var(--vcp-safe-bottom,48px)+16px)]"
          @touchstart="handleTouchStart"
          @touchmove="handleTouchMove"
          @touchend="handleTouchEnd"
          @touchcancel="handleTouchEnd"
        >
          <div v-if="isPreparing" class="text-sm text-white/60">正在准备图片…</div>
          <div v-else-if="preparationError" class="text-sm text-red-300 break-all p-4">{{ preparationError }}</div>
          <img
            v-else-if="displaySrc"
            ref="imageRef"
            :src="displaySrc"
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
