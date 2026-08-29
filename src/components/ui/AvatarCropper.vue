<script setup lang="ts">
import { onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue';
import 'vue-cropper/dist/index.css';
import { VueCropper } from 'vue-cropper';
import { useModalHistory } from '../../core/composables/useModalHistory';
import { useNotificationStore } from '../../core/stores/notification';

const props = defineProps<{
  img: string;
}>();

const emit = defineEmits(['cancel', 'confirm']);

const cropper = ref<any>(null);
const isConfirming = ref(false);
const notificationStore = useNotificationStore();
const { registerModal, unregisterModal } = useModalHistory();
const MODAL_ID = 'AvatarCropper';
let modalRegistered = false;
let disposed = false;

const resolveCropper = () => {
  if (!cropper.value) return null;
  return cropper.value.getCropBlob ? cropper.value : cropper.value.value;
};

const releaseModalHistory = () => {
  if (!modalRegistered) return;
  modalRegistered = false;
  unregisterModal(MODAL_ID);
};

const handleCancel = () => {
  if (isConfirming.value) return false;
  releaseModalHistory();
  emit('cancel');
  return true;
};

const toCircularPng = (source: Blob): Promise<Blob> => new Promise((resolve, reject) => {
  const sourceUrl = URL.createObjectURL(source);
  const image = new Image();

  const cleanup = () => URL.revokeObjectURL(sourceUrl);
  image.onload = () => {
    try {
      const sourceWidth = image.naturalWidth || image.width;
      const sourceHeight = image.naturalHeight || image.height;
      const diameter = Math.min(360, sourceWidth, sourceHeight);
      if (diameter <= 0) throw new Error('裁剪结果尺寸无效');

      const canvas = document.createElement('canvas');
      canvas.width = diameter;
      canvas.height = diameter;
      const context = canvas.getContext('2d');
      if (!context) throw new Error('无法创建头像画布');

      const sourceSize = Math.min(sourceWidth, sourceHeight);
      const sourceX = (sourceWidth - sourceSize) / 2;
      const sourceY = (sourceHeight - sourceSize) / 2;
      context.beginPath();
      context.arc(diameter / 2, diameter / 2, diameter / 2, 0, Math.PI * 2);
      context.clip();
      context.drawImage(
        image,
        sourceX,
        sourceY,
        sourceSize,
        sourceSize,
        0,
        0,
        diameter,
        diameter,
      );

      canvas.toBlob((blob) => {
        cleanup();
        if (blob) resolve(blob);
        else reject(new Error('生成圆形头像失败'));
      }, 'image/png', 1);
    } catch (error) {
      cleanup();
      reject(error);
    }
  };
  image.onerror = () => {
    cleanup();
    reject(new Error('读取裁剪结果失败'));
  };
  image.src = sourceUrl;
});

const handleConfirm = () => {
  if (isConfirming.value) return;
  if (!cropper.value) {
    notificationStore.addNotification({
      type: 'warning',
      message: '裁剪器尚未就绪',
      toastOnly: true
    });
    return;
  }
  
  // 针对 vue-cropper-next 的多种可能的 Ref 结构进行探测
  const realCropper = resolveCropper();
  
  if (!realCropper || typeof realCropper.getCropBlob !== 'function') {
    console.error('AvatarCropper: Cannot find getCropBlob on ref', cropper.value);
    notificationStore.addNotification({
      type: 'error',
      message: '裁剪器组件异常，请重试',
      toastOnly: true
    });
    return;
  }

  isConfirming.value = true;
  try {
    realCropper.getCropBlob((data: Blob | null) => {
      if (!data) {
        isConfirming.value = false;
        notificationStore.addNotification({
          type: 'error',
          message: '生成裁剪结果失败，请重试',
          toastOnly: true
        });
        return;
      }

      void toCircularPng(data)
        .then((circularBlob) => {
          if (disposed) return;
          releaseModalHistory();
          emit('confirm', circularBlob);
        })
        .catch((error) => {
          console.error('AvatarCropper: Failed to create circular avatar', error);
          if (!disposed) {
            notificationStore.addNotification({
              type: 'error',
              message: '生成圆形头像失败，请重试',
              toastOnly: true
            });
          }
        })
        .finally(() => {
          if (!disposed) isConfirming.value = false;
        });
    });
  } catch (error) {
    isConfirming.value = false;
    console.error('AvatarCropper: Failed to export crop', error);
    notificationStore.addNotification({
      type: 'error',
      message: '导出头像失败，请重试',
      toastOnly: true
    });
  }
};

const handleRotate = () => {
  if (isConfirming.value) return;
  const realCropper = resolveCropper();
  if (realCropper && typeof realCropper.rotateLeft === 'function') {
    realCropper.rotateLeft();
  }
};

const handleScale = (num: number) => {
  if (isConfirming.value) return;
  const realCropper = resolveCropper();
  if (realCropper && typeof realCropper.changeScale === 'function') {
    realCropper.changeScale(num);
  }
};

const options = reactive({
  img: props.img,
  size: 1,
  full: false,
  outputType: 'png',
  canMove: true,
  fixed: true,
  fixedNumber: [1, 1],
  fixedBox: true,
  original: false,
  canMoveBox: false,
  autoCrop: true,
  autoCropWidth: 360, // 头像输出基准宽度
  autoCropHeight: 360, // 头像输出基准高度
  centerBox: true,
  high: false,
  maxImgSize: 1120,
  cropData: {},
  enlarge: 1,
  mode: 'contain'
});

watch(() => props.img, (newImg) => {
  options.img = newImg;
});

onMounted(() => {
  modalRegistered = true;
  registerModal(MODAL_ID, handleCancel);
});

onBeforeUnmount(() => {
  disposed = true;
  const internalImageUrl = resolveCropper()?.imgs;
  if (typeof internalImageUrl === 'string' && internalImageUrl.startsWith('blob:')) {
    URL.revokeObjectURL(internalImageUrl);
  }
  releaseModalHistory();
});
</script>

<template>
  <Teleport to="#vcp-feature-overlays">
    <div class="avatar-cropper-overlay vcp-safe-inline fixed inset-0 z-viewer flex flex-col bg-black text-white animate-in fade-in duration-300 pointer-events-auto">
      <header class="p-4 flex items-center justify-between border-b border-white/10 shrink-0 pt-[calc(var(--vcp-safe-top,24px)+10px)]">
        <button :disabled="isConfirming" @click="handleCancel" class="px-4 py-2 text-sm font-bold text-white/60 active:scale-95 transition-all disabled:opacity-30">
          取消
        </button>
        <h3 class="text-sm font-black uppercase tracking-[0.2em] text-white/90">裁剪头像</h3>
        <button :disabled="isConfirming" @click="handleConfirm" class="px-4 py-2 text-sm font-bold text-blue-400 active:scale-95 transition-all disabled:opacity-30">
          {{ isConfirming ? '处理中…' : '完成' }}
        </button>
      </header>

      <div class="flex-1 relative bg-[#111] overflow-hidden flex items-center justify-center">
        <div class="w-full h-full">
          <VueCropper
            v-if="options.img"
            ref="cropper"
            :img="options.img"
            :outputSize="options.size"
            :outputType="options.outputType"
            :info="true"
            :full="options.full"
            :canMove="options.canMove"
            :canMoveBox="options.canMoveBox"
            :original="options.original"
            :autoCrop="options.autoCrop"
            :autoCropWidth="options.autoCropWidth"
            :autoCropHeight="options.autoCropHeight"
            :fixed="options.fixed"
            :fixedNumber="options.fixedNumber"
            :fixedBox="options.fixedBox"
            :centerBox="options.centerBox"
            :high="options.high"
            :maxImgSize="options.maxImgSize"
            :infoTrue="true"
            :enlarge="options.enlarge"
            :mode="options.mode"
          />
        </div>
      </div>

      <footer class="p-10 flex flex-col items-center gap-6 shrink-0 pb-[calc(var(--vcp-safe-bottom,48px)+20px)] bg-black">
        <div class="flex items-center gap-8">
           <button :disabled="isConfirming" @click="handleScale(1)" class="p-3 bg-white/10 rounded-full text-white active:bg-white/20 disabled:opacity-30">
              <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"></circle><line x1="21" y1="21" x2="16.65" y2="16.65"></line><line x1="11" y1="8" x2="11" y2="14"></line><line x1="8" y1="11" x2="14" y2="11"></line></svg>
           </button>
           <button :disabled="isConfirming" @click="handleScale(-1)" class="p-3 bg-white/10 rounded-full text-white active:bg-white/20 disabled:opacity-30">
              <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"></circle><line x1="21" y1="21" x2="16.65" y2="16.65"></line><line x1="8" y1="11" x2="14" y2="11"></line></svg>
           </button>
           <button :disabled="isConfirming" @click="handleRotate()" class="p-3 bg-white/10 rounded-full text-white active:bg-white/20 disabled:opacity-30">
              <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M2.5 2v6h6M2.66 15.57a10 10 0 1 0 .57-8.38"></path></svg>
           </button>
        </div>
        <p class="text-[10px] text-white/40 uppercase font-black tracking-[0.3em]">移动图片以调整裁剪区域</p>
      </footer>
    </div>
  </Teleport>
</template>

<style scoped>
.avatar-cropper-overlay {
  touch-action: none;
}

:deep(.cropper-view-box) {
  outline: 1px solid rgba(255, 255, 255, 0.5);
  border-radius: 50%; /* 圆形预览 */
}

:deep(.cropper-face) {
  background-color: transparent;
}
</style>
