<script setup lang="ts">
import { ref, watch, onUnmounted } from 'vue';
import { useModalHistory } from '../../../core/composables/useModalHistory';
import { useNotificationStore } from '../../../core/stores/notification';
import { useThemeStore } from '../../../core/stores/theme';
import { isAndroid } from '../../../core/utils/platform';
import { saveImageToGallery } from 'tauri-plugin-vcp-mobile';

const props = defineProps<{
  visible: boolean;
  svgHtml: string;
  sourceCode: string;
}>();

const emit = defineEmits<{
  (e: 'close'): void;
}>();

const themeStore = useThemeStore();
const notificationStore = useNotificationStore();

const modalId = `MermaidFullScreen_${Math.random().toString(36).substring(2, 9)}`;
const { registerModal, unregisterModal } = useModalHistory();

// Transform States
const scale = ref(1);
const translateX = ref(0);
const translateY = ref(0);
const isDragging = ref(false);

const fullscreenViewportRef = ref<HTMLElement | null>(null);
const fullscreenCanvasRef = ref<HTMLElement | null>(null);

const isCopyDropdownVisible = ref(false);
const isExporting = ref(false);
let exportController: AbortController | null = null;

const toggleCopyDropdown = (e: MouseEvent) => {
  e.stopPropagation();
  isCopyDropdownVisible.value = !isCopyDropdownVisible.value;
};

const closeCopyDropdown = () => {
  isCopyDropdownVisible.value = false;
};

watch(() => props.visible, (newVal) => {
  if (newVal) {
    registerModal(modalId, () => {
      emit('close');
    });
    // Reset transform on open to default 100% size
    scale.value = 1;
    translateX.value = 0;
    translateY.value = 0;
    
    window.addEventListener('click', closeCopyDropdown);
  } else {
    exportController?.abort();
    unregisterModal(modalId);
    window.removeEventListener('click', closeCopyDropdown);
    isCopyDropdownVisible.value = false;
  }
});

watch(() => props.svgHtml, () => exportController?.abort());

onUnmounted(() => {
  exportController?.abort();
  unregisterModal(modalId);
  window.removeEventListener('click', closeCopyDropdown);
});

// Operations
const zoomIn = () => {
  scale.value = Math.min(5, scale.value * 1.25);
};

const zoomOut = () => {
  scale.value = Math.max(0.15, scale.value / 1.25);
};

const resetView = () => {
  scale.value = 1;
  translateX.value = 0;
  translateY.value = 0;
};

const fitToWidth = () => {
  resetView();
};

// Pointer Handlers for drag and pinch zoom
const activePointers = new Map<number, { clientX: number; clientY: number }>();
let initialPinchDistance = 0;
let initialPinchScale = 1;
let startDragX = 0;
let startDragY = 0;
let startTranslateX = 0;
let startTranslateY = 0;

const handlePointerDown = (e: PointerEvent) => {
  if (!fullscreenViewportRef.value) return;
  
  try {
    fullscreenViewportRef.value.setPointerCapture(e.pointerId);
  } catch (err) {}
  
  activePointers.set(e.pointerId, { clientX: e.clientX, clientY: e.clientY });
  const pointerList = Array.from(activePointers.values());
  
  if (pointerList.length === 1) {
    isDragging.value = true;
    startDragX = e.clientX;
    startDragY = e.clientY;
    startTranslateX = translateX.value;
    startTranslateY = translateY.value;
  } else if (pointerList.length === 2) {
    isDragging.value = false;
    const dx = pointerList[0].clientX - pointerList[1].clientX;
    const dy = pointerList[0].clientY - pointerList[1].clientY;
    initialPinchDistance = Math.sqrt(dx * dx + dy * dy);
    initialPinchScale = scale.value;
  }
};

const handlePointerMove = (e: PointerEvent) => {
  if (!activePointers.has(e.pointerId)) return;
  
  activePointers.set(e.pointerId, { clientX: e.clientX, clientY: e.clientY });
  const pointerList = Array.from(activePointers.values());
  
  if (pointerList.length === 1 && isDragging.value) {
    const dx = e.clientX - startDragX;
    const dy = e.clientY - startDragY;
    translateX.value = startTranslateX + dx;
    translateY.value = startTranslateY + dy;
  } else if (pointerList.length === 2) {
    const dx = pointerList[0].clientX - pointerList[1].clientX;
    const dy = pointerList[0].clientY - pointerList[1].clientY;
    const distance = Math.sqrt(dx * dx + dy * dy);
    if (initialPinchDistance > 0 && distance > 0) {
      const factor = distance / initialPinchDistance;
      scale.value = Math.min(5, Math.max(0.15, initialPinchScale * factor));
    }
  }
};

const handlePointerUp = (e: PointerEvent) => {
  if (fullscreenViewportRef.value) {
    try {
      fullscreenViewportRef.value.releasePointerCapture(e.pointerId);
    } catch (err) {}
  }
  
  activePointers.delete(e.pointerId);
  
  if (activePointers.size < 2) {
    initialPinchDistance = 0;
  }
  
  if (activePointers.size === 0) {
    isDragging.value = false;
  } else if (activePointers.size === 1) {
    // Smooth transition from pinch back to drag
    const remainingId = Array.from(activePointers.keys())[0];
    const remainingPointer = activePointers.get(remainingId)!;
    isDragging.value = true;
    startDragX = remainingPointer.clientX;
    startDragY = remainingPointer.clientY;
    startTranslateX = translateX.value;
    startTranslateY = translateY.value;
  }
};

const handleWheel = (e: WheelEvent) => {
  const factor = e.deltaY < 0 ? 1.15 : 1 / 1.15;
  scale.value = Math.min(5, Math.max(0.15, scale.value * factor));
};

const handleDblClick = (e: MouseEvent) => {
  e.preventDefault();
  fitToWidth();
};

// Copy actions
const copySource = async () => {
  try {
    await navigator.clipboard.writeText(props.sourceCode);
    notificationStore.addNotification({
      type: 'success',
      title: '复制成功',
      message: 'Mermaid 源码已成功复制到剪贴板',
      toastOnly: true
    });
    isCopyDropdownVisible.value = false;
  } catch (err) {
    console.error('Copy source failed', err);
  }
};

const copySvg = async () => {
  try {
    await navigator.clipboard.writeText(props.svgHtml);
    notificationStore.addNotification({
      type: 'success',
      title: '复制成功',
      message: 'SVG 代码已成功复制到剪贴板',
      toastOnly: true
    });
    isCopyDropdownVisible.value = false;
  } catch (err) {
    console.error('Copy SVG failed', err);
  }
};


// 保持小图最高 2.5x 的清晰度；大图在 SVG 解码及画布分配前限制为 2048² 像素。
const MAX_EXPORT_EDGE = 2048;
const MAX_EXPORT_PIXELS = MAX_EXPORT_EDGE * MAX_EXPORT_EDGE;
// 编码文件预算独立于像素预算；16 MiB 与最大 RGBA8 画布的裸像素量相当。
const MAX_EXPORT_PNG_BYTES = 16 * 1024 * 1024;

const exportToGallery = async () => {
  if (isExporting.value || !props.visible || !fullscreenCanvasRef.value) return;
  const svg = fullscreenCanvasRef.value.querySelector('svg');
  if (!svg) return;

  isExporting.value = true;
  const controller = new AbortController();
  exportController = controller;
  let img: HTMLImageElement | null = null;
  let canvas: HTMLCanvasElement | null = null;
  let submitted = false;
  const ensureActive = () => {
    if (controller.signal.aborted) throw new DOMException('导出已取消', 'AbortError');
  };

  try {
    const viewBox = svg.viewBox?.baseVal;
    const bounds = viewBox?.width && viewBox?.height ? viewBox : svg.getBBox?.();
    const svgWidth = bounds?.width || svg.clientWidth || 600;
    const svgHeight = bounds?.height || svg.clientHeight || 400;
    if (!Number.isFinite(svgWidth) || !Number.isFinite(svgHeight) || svgWidth <= 0 || svgHeight <= 0) {
      throw new Error('图表尺寸无效');
    }
    const scaleFactor = Math.min(
      2.5,
      MAX_EXPORT_EDGE / Math.max(svgWidth, svgHeight),
      Math.sqrt(MAX_EXPORT_PIXELS / svgWidth / svgHeight),
    );
    const width = Math.max(1, Math.floor(svgWidth * scaleFactor));
    const height = Math.max(1, Math.floor(svgHeight * scaleFactor));
    const background = themeStore.isDarkResolved ? '#0d1117' : '#ffffff';

    const clonedSvg = svg.cloneNode(true) as SVGSVGElement;
    clonedSvg.setAttribute('xmlns', 'http://www.w3.org/2000/svg');
    if (!viewBox?.width || !viewBox?.height) {
      clonedSvg.setAttribute('viewBox', `${bounds?.x || 0} ${bounds?.y || 0} ${svgWidth} ${svgHeight}`);
    }
    clonedSvg.setAttribute('width', String(width));
    clonedSvg.setAttribute('height', String(height));
    clonedSvg.style.width = `${width}px`;
    clonedSvg.style.height = `${height}px`;
    clonedSvg.style.maxWidth = 'none';
    clonedSvg.style.maxHeight = 'none';

    // 保留现有 Android WebView 的 inline SVG 加载方式；外部资源仍可能使画布污损，
    // 由编码错误路径反馈，不把 Data URL 或 Object URL 当作安全保证。
    const svgString = new XMLSerializer().serializeToString(clonedSvg);
    const dataUrlSrc = `data:image/svg+xml;base64,${window.btoa(unescape(encodeURIComponent(svgString)))}`;
    img = new Image(width, height);
    const exportImage = img;
    await new Promise<void>((resolve, reject) => {
      const cleanup = () => {
        exportImage.onload = null;
        exportImage.onerror = null;
        controller.signal.removeEventListener('abort', abort);
      };
      const abort = () => {
        cleanup();
        exportImage.src = '';
        reject(new DOMException('导出已取消', 'AbortError'));
      };
      exportImage.onload = () => { cleanup(); resolve(); };
      exportImage.onerror = () => { cleanup(); reject(new Error('图表图像加载失败，无法渲染')); };
      controller.signal.addEventListener('abort', abort, { once: true });
      exportImage.src = dataUrlSrc;
    });
    ensureActive();

    canvas = document.createElement('canvas');
    canvas.width = width;
    canvas.height = height;
    const ctx = canvas.getContext('2d');
    if (!ctx) throw new Error('无法创建导出画布');
    ctx.fillStyle = background;
    ctx.fillRect(0, 0, width, height);
    ctx.drawImage(exportImage, 0, 0, width, height);
    exportImage.src = '';

    // toBlob 无取消 API；关闭后仍等待编码结束，避免重开页面积压并行编码。
    const png = await new Promise<Blob>((resolve, reject) => {
      canvas!.toBlob((blob) => {
        if (blob) resolve(blob);
        else reject(new Error('图表 PNG 编码失败'));
      }, 'image/png');
    });
    canvas.width = 0;
    canvas.height = 0;
    ensureActive();
    if (png.size > MAX_EXPORT_PNG_BYTES) {
      throw new Error('图表 PNG 超过 16 MiB，请复制 SVG 代码保存');
    }

    // 仅已通过像素和编码大小预算的 PNG 才转换为现有相册接口的 Data URL。
    const dataUrl = await new Promise<string>((resolve, reject) => {
      const reader = new FileReader();
      const cleanup = () => {
        reader.onload = null;
        reader.onerror = null;
        reader.onabort = null;
        controller.signal.removeEventListener('abort', abort);
      };
      const abort = () => reader.abort();
      reader.onload = () => {
        const result = reader.result;
        cleanup();
        if (typeof result === 'string') resolve(result);
        else reject(new Error('读取图表 PNG 失败'));
      };
      reader.onerror = () => { cleanup(); reject(new Error('读取图表 PNG 失败')); };
      reader.onabort = () => { cleanup(); reject(new DOMException('导出已取消', 'AbortError')); };
      controller.signal.addEventListener('abort', abort, { once: true });
      reader.readAsDataURL(png);
    });
    ensureActive();
    submitted = true;
    await saveImageToGallery(dataUrl, `mermaid_${Date.now()}.png`);
    // 原生保存已提交后，关闭预览不意味着取消；按实际结果提示。
    notificationStore.addNotification({
      type: 'success',
      title: '保存成功',
      message: '图表已作为 PNG 图片保存到系统相册',
      toastOnly: true,
    });
  } catch (err) {
    if (submitted || !controller.signal.aborted) {
      console.error('Export image to gallery failed:', err);
      notificationStore.addNotification({
        type: 'error',
        title: '保存失败',
        message: '保存失败：' + String(err),
        toastOnly: true,
      });
    }
  } finally {
    if (img) {
      img.onload = null;
      img.onerror = null;
      img.src = '';
    }
    if (canvas) {
      canvas.width = 0;
      canvas.height = 0;
    }
    if (exportController === controller) {
      exportController = null;
      isExporting.value = false;
    }
  }
};
</script>

<template>
  <Teleport to="body">
    <Transition
      enter-active-class="transition duration-300 ease-out"
      enter-from-class="translate-y-10 opacity-0"
      enter-to-class="translate-y-0 opacity-100"
      leave-active-class="transition duration-200 ease-in"
      leave-from-class="translate-y-0 opacity-100"
      leave-to-class="translate-y-10 opacity-0"
    >
      <div v-if="visible" class="vcp-safe-inline fixed inset-0 z-viewer flex flex-col select-none overflow-hidden pb-[var(--vcp-safe-bottom,0px)]"
        :class="themeStore.isDarkResolved ? 'bg-[#0d1117] text-gray-200' : 'bg-[#f8fafc] text-gray-800'">
        
        <!-- Header -->
        <div class="h-14 flex items-center justify-between px-4 border-b pt-[var(--vcp-safe-top,0px)] box-content"
          :class="themeStore.isDarkResolved ? 'border-white/5 bg-[#0d1117]' : 'border-black/5 bg-[#f8fafc]'">
          <div class="flex items-center gap-3">
            <button @click="emit('close')" class="p-2 -ml-2 active:scale-95 transition-transform">
              <div class="i-ph:caret-left-bold w-5 h-5" :class="themeStore.isDarkResolved ? 'text-gray-400' : 'text-gray-600'"></div>
            </button>
            <span class="text-sm font-bold">图表预览</span>
          </div>
          
          <div class="flex items-center gap-2">
            <!-- 复制二级下拉菜单容器 -->
            <div class="relative">
              <button @click="toggleCopyDropdown" class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg border text-xs active:scale-95 transition-all"
                :class="themeStore.isDarkResolved ? 'border-white/10 bg-white/5 text-gray-400' : 'border-black/10 bg-black/5 text-gray-600'">
                <div class="i-ph:copy-bold w-3.5 h-3.5"></div>
                <span>复制</span>
                <div class="i-ph:caret-down-bold w-3 h-3 opacity-60"></div>
              </button>
              
              <!-- 下拉菜单 (Dropdown) -->
              <div v-if="isCopyDropdownVisible" 
                class="absolute right-0 mt-1.5 w-36 rounded-lg border shadow-lg z-30 py-1"
                :class="themeStore.isDarkResolved ? 'border-white/10 bg-[#1e293b] text-gray-200' : 'border-black/10 bg-white text-gray-800'"
                @click.stop
              >
                <button @click="copySource" class="w-full text-left px-3 py-2 text-xs hover:bg-black/5 dark:hover:bg-white/5 flex items-center gap-2">
                  <div class="i-ph:code-bold w-3.5 h-3.5 opacity-70"></div>
                  <span>复制代码</span>
                </button>
                <button @click="copySvg" class="w-full text-left px-3 py-2 text-xs hover:bg-black/5 dark:hover:bg-white/5 flex items-center gap-2 border-t"
                  :class="themeStore.isDarkResolved ? 'border-white/5' : 'border-black/5'">
                  <div class="i-ph:file-code-bold w-3.5 h-3.5 opacity-70"></div>
                  <span>复制 SVG 代码</span>
                </button>
              </div>
            </div>

            <!-- 保存到相册按钮（改用下载图标，文字按钮样式） -->
            <button v-if="isAndroid" @click="exportToGallery" :disabled="isExporting" :aria-busy="isExporting" class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg border text-xs active:scale-95 transition-all disabled:opacity-50"
              :class="themeStore.isDarkResolved ? 'border-white/10 bg-white/5 text-gray-400' : 'border-black/10 bg-black/5 text-gray-600'">
              <div class="i-ph:download-simple-bold w-3.5 h-3.5"></div>
              <span>{{ isExporting ? '正在保存…' : '保存到相册' }}</span>
            </button>
          </div>
        </div>
        
        <!-- Viewport Grid Background (No Backdrop Blur) -->
        <div 
          ref="fullscreenViewportRef"
          class="flex-1 relative w-full overflow-hidden cursor-grab active:cursor-grabbing touch-none"
          :style="{
            backgroundImage: themeStore.isDarkResolved 
              ? 'radial-gradient(rgba(255, 255, 255, 0.08) 1px, transparent 1px)' 
              : 'radial-gradient(rgba(0, 0, 0, 0.06) 1px, transparent 1px)',
            backgroundSize: '20px 20px'
          }"
          @pointerdown="handlePointerDown"
          @pointermove="handlePointerMove"
          @pointerup="handlePointerUp"
          @pointercancel="handlePointerUp"
          @wheel="handleWheel"
          @dblclick="handleDblClick"
        >
          <div 
            ref="fullscreenCanvasRef"
            class="inline-flex items-center justify-center min-w-full min-h-full p-8 box-border origin-center"
            :style="{
              transform: `translate(${translateX}px, ${translateY}px) scale(${scale})`,
              transition: isDragging ? 'none' : 'transform 0.1s ease-out'
            }"
            v-html="svgHtml"
          ></div>
        </div>
        
        <!-- Floating Control Bar (No Backdrop Blur, Solid background) -->
        <div class="absolute left-1/2 -translate-x-1/2 flex items-center gap-1 px-3 py-2 rounded-xl border shadow-xl z-20"
          :style="{ bottom: `calc(var(--vcp-safe-bottom, 48px) + 24px)` }"
          :class="themeStore.isDarkResolved 
            ? 'border-white/10 bg-[#1e293b] text-gray-200' 
            : 'border-black/10 bg-white text-gray-800'">
          
          <button @click="zoomOut" class="p-2 active:scale-90 transition-all hover:opacity-80" title="缩小">
            <div class="i-ph:minus-bold w-4.5 h-4.5"></div>
          </button>
          
          <button @click="resetView" class="px-2.5 py-1 text-xs font-semibold font-mono rounded-lg active:bg-black/5 dark:active:bg-white/5 transition-all hover:opacity-80" title="重置 100%">
            {{ Math.round(scale * 100) }}%
          </button>
          
          <button @click="zoomIn" class="p-2 active:scale-90 transition-all hover:opacity-80" title="放大">
            <div class="i-ph:plus-bold w-4.5 h-4.5"></div>
          </button>
          
          <div class="w-[1px] h-4 mx-1"
            :class="themeStore.isDarkResolved ? 'bg-white/15' : 'bg-black/15'"></div>
          
          <button @click="fitToWidth" class="p-2 active:scale-90 transition-all hover:opacity-80" title="适应屏幕">
            <div class="i-ph:arrows-in-line-horizontal-bold w-4.5 h-4.5"></div>
          </button>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
/* Ensure custom SVGs are responsive inside canvas */
:deep(svg) {
  max-width: none !important;
  height: auto !important;
}
</style>
