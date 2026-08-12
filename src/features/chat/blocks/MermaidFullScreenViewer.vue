<script setup lang="ts">
import { ref, watch, onUnmounted } from 'vue';
import { useModalHistory } from '../../../core/composables/useModalHistory';
import { useNotificationStore } from '../../../core/stores/notification';
import { useThemeStore } from '../../../core/stores/theme';
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
    unregisterModal(modalId);
    window.removeEventListener('click', closeCopyDropdown);
    isCopyDropdownVisible.value = false;
  }
});

onUnmounted(() => {
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


const exportToGallery = () => {
  if (!fullscreenCanvasRef.value) return;
  const svg = fullscreenCanvasRef.value.querySelector('svg');
  if (!svg) return;

  const clonedSvg = svg.cloneNode(true) as SVGElement;
  clonedSvg.setAttribute('xmlns', 'http://www.w3.org/2000/svg');
  
  const svgWidth = svg.getBBox?.().width || svg.viewBox?.baseVal?.width || svg.clientWidth || 600;
  const svgHeight = svg.getBBox?.().height || svg.viewBox?.baseVal?.height || svg.clientHeight || 400;
  
  clonedSvg.setAttribute('width', String(svgWidth));
  clonedSvg.setAttribute('height', String(svgHeight));

  const svgString = new XMLSerializer().serializeToString(clonedSvg);
  // 100% 规避 Android WebView 下 URL.createObjectURL(Blob) 造成的 Canvas 污损 (Tainted Canvas) 安全异常，
  // 使用 inline Base64 Data URL 方案。使用 unescape(encodeURIComponent) 兼容中文及特殊字符编码。
  const base64Svg = window.btoa(unescape(encodeURIComponent(svgString)));
  const dataUrlSrc = `data:image/svg+xml;base64,${base64Svg}`;

  const img = new Image();
  img.onload = () => {
    const canvas = document.createElement('canvas');
    const scaleFactor = Math.min(2.5, Math.max(1.0, 2048 / Math.max(svgWidth, svgHeight)));
    
    canvas.width = svgWidth * scaleFactor;
    canvas.height = svgHeight * scaleFactor;
    
    const ctx = canvas.getContext('2d');
    if (ctx) {
      ctx.fillStyle = themeStore.isDarkResolved ? '#0d1117' : '#ffffff';
      ctx.fillRect(0, 0, canvas.width, canvas.height);
      ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
      
      try {
        const dataUrl = canvas.toDataURL('image/png');
        saveImageToGallery(dataUrl, `mermaid_${Date.now()}.png`).then(() => {
          notificationStore.addNotification({
            type: 'success',
            title: '保存成功',
            message: '图表已作为高清 PNG 图片保存到系统相册',
            toastOnly: true
          });
        }).catch((err) => {
          console.error('Save image to gallery failed:', err);
          notificationStore.addNotification({
            type: 'error',
            title: '保存失败',
            message: '保存失败：' + String(err),
            toastOnly: true
          });
        });
      } catch (err) {
        console.error('Canvas serialization failed:', err);
        notificationStore.addNotification({
          type: 'error',
          title: '保存失败',
          message: '保存失败：Canvas 序列化受限',
          toastOnly: true
        });
      }
    }
  };
  
  img.onerror = (e) => {
    console.error('Image load failed', e);
    notificationStore.addNotification({
      type: 'error',
      title: '导出失败',
      message: '图表图像加载失败，无法渲染',
      toastOnly: true
    });
  };
  
  img.src = dataUrlSrc;
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
            <button @click="exportToGallery" class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg border text-xs active:scale-95 transition-all"
              :class="themeStore.isDarkResolved ? 'border-white/10 bg-white/5 text-gray-400' : 'border-black/10 bg-black/5 text-gray-600'">
              <div class="i-ph:download-simple-bold w-3.5 h-3.5"></div>
              <span>保存到相册</span>
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
