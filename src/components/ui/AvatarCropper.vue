<script setup lang="ts">
import { ref, reactive, watch } from 'vue';
import 'vue-cropper/dist/index.css';
import { VueCropper } from 'vue-cropper';

const props = defineProps<{
  img: string;
}>();

const emit = defineEmits(['cancel', 'confirm']);

const cropper = ref<any>(null);

const handleConfirm = () => {
  if (!cropper.value) {
    alert('裁剪器尚未就绪');
    return;
  }
  
  // 针对 vue-cropper-next 的多种可能的 Ref 结构进行探测
  const realCropper = cropper.value.getCropBlob ? cropper.value : cropper.value.value;
  
  if (!realCropper || typeof realCropper.getCropBlob !== 'function') {
    console.error('AvatarCropper: Cannot find getCropBlob on ref', cropper.value);
    alert('裁剪器组件异常，请重试');
    return;
  }

  realCropper.getCropBlob((data: Blob) => {
    emit('confirm', data);
  });
};

const handleRotate = () => {
  if (!cropper.value) return;
  const realCropper = cropper.value.rotateLeft ? cropper.value : cropper.value.value;
  if (realCropper && typeof realCropper.rotateLeft === 'function') {
    realCropper.rotateLeft();
  }
};

const handleScale = (num: number) => {
  if (!cropper.value) return;
  const realCropper = cropper.value.changeScale ? cropper.value : cropper.value.value;
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
  fixedBox: true, // 锁定比例
  original: false,
  canMoveBox: false,
  autoCrop: true,
  autoCropWidth: 360, // 桌面端标准宽度
  autoCropHeight: 360, // 桌面端标准高度
  centerBox: true,
  high: true,
  cropData: {},
  enlarge: 1, // 关键：禁止根据屏幕 DPR 放大输出，确保输出 360 像素
  mode: 'contain'
});

watch(() => props.img, (newImg) => {
  options.img = newImg;
});
</script>

<template>
  <Teleport to="#vcp-feature-overlays">
    <div class="avatar-cropper-overlay fixed inset-0 z-[2000] flex flex-col bg-black text-white animate-in fade-in duration-300 pointer-events-auto">
      <header class="p-4 flex items-center justify-between border-b border-white/10 shrink-0 pt-[calc(var(--vcp-safe-top,24px)+10px)]">
        <button @click="emit('cancel')" class="px-4 py-2 text-sm font-bold text-white/60 active:scale-95 transition-all">
          取消
        </button>
        <h3 class="text-sm font-black uppercase tracking-[0.2em] text-white/90">裁剪头像</h3>
        <button @click="handleConfirm" class="px-4 py-2 text-sm font-bold text-blue-400 active:scale-95 transition-all">
          完成
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
            :fixedBox="options.fixedBox"
            :centerBox="options.centerBox"
            :high="options.high"
            :infoTrue="true"
            :enlarge="options.enlarge"
            :mode="options.mode"
          />
        </div>
      </div>

      <footer class="p-10 flex flex-col items-center gap-6 shrink-0 pb-[calc(var(--vcp-safe-bottom,20px)+20px)] bg-black">
        <div class="flex items-center gap-8">
           <button @click="handleScale(1)" class="p-3 bg-white/10 rounded-full text-white active:bg-white/20">
              <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"></circle><line x1="21" y1="21" x2="16.65" y2="16.65"></line><line x1="11" y1="8" x2="11" y2="14"></line><line x1="8" y1="11" x2="14" y2="11"></line></svg>
           </button>
           <button @click="handleScale(-1)" class="p-3 bg-white/10 rounded-full text-white active:bg-white/20">
              <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"></circle><line x1="21" y1="21" x2="16.65" y2="16.65"></line><line x1="8" y1="11" x2="14" y2="11"></line></svg>
           </button>
           <button @click="handleRotate()" class="p-3 bg-white/10 rounded-full text-white active:bg-white/20">
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
