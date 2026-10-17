<script setup lang="ts">
import { computed } from "vue";
import AttachmentRenderer from "./attachment/AttachmentRenderer.vue";
import { classifyAttachment } from "./attachment/utils/AttachmentClassifier";
import { AttachmentType } from "./attachment/types/AttachmentType";
import type { Attachment } from "../../core/types/chat";

const props = defineProps<{ file: Attachment; index: number }>();
const emit = defineEmits<{ (e: "remove", index: number): void }>();

const attachmentType = computed(() => 
  classifyAttachment(props.file.type, props.file.name)
);

const isImage = computed(() => attachmentType.value === AttachmentType.IMAGE);

const isLoading = computed(() => props.file.status === "loading" || props.file.status === "processing");
</script>

<template>
  <!-- 注意：本层严禁加 overflow-hidden——删除按钮（AttachmentPreviewBase 内
       -top-1.5 -right-1.5 的悬浮徽章）会被卡片框裁掉。圆角裁剪已由
       ImageAttachment 内层 rounded-xl overflow-hidden 与下方 Loading
       Overlay 自带的 rounded-xl 各自承担，无需本层代劳。 -->
  <div
    class="staged-attachment relative shrink-0 rounded-xl"
    :class="[isImage ? (isLoading ? 'w-36 h-14' : 'w-14 h-14') : 'max-w-[180px]']"
  >
    <!-- Use new component system -->
    <AttachmentRenderer 
      :style="isImage && isLoading ? { width: '100%' } : undefined"
      :file="file" 
      :index="index"
      :show-remove="true"
      @remove="emit('remove', index)"
    />

    <!-- Loading Overlay -->
    <div
      v-if="isLoading"
      class="absolute inset-0 bg-black/60 rounded-xl flex flex-col items-center justify-center z-10 gap-0.5 pointer-events-none"
    >
      <span class="max-w-full px-2 truncate text-[10px] text-white" :title="file.name">{{ file.name }}</span>
      <div
        class="w-4 h-4 border-2 border-white/30 border-t-white rounded-full animate-spin"
      ></div>
      <span
        v-if="file.status === 'processing'"
        class="text-[9px] text-white"
      >正在处理</span>
      <span
        v-else-if="file.progress !== undefined"
        class="text-[9px] text-white font-bold tabular-nums"
        >{{ file.progress }}%</span
      >
      <span v-else-if="file.transferredBytes !== undefined" class="text-[9px] text-white font-mono">
        已复制 {{ (file.transferredBytes / 1024 / 1024).toFixed(1) }} MiB
      </span>
    </div>
  </div>
</template>

<style scoped>
/* This staged card owns the progress label, including acquisition byte counts. */
.staged-attachment :deep(.attachment-progress) {
  display: none;
}

.list-enter-active,
.list-leave-active {
  transition: all 0.3s cubic-bezier(0.4, 0, 0.2, 1);
}
.list-enter-from {
  opacity: 0;
  transform: translateX(20px) scale(0.8);
}
.list-leave-to {
  opacity: 0;
  transform: translateY(-20px) scale(0.8);
}
</style>
