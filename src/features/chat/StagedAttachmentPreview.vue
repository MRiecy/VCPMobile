<script setup lang="ts">
import { computed } from "vue";
import AttachmentRenderer from "./attachment/AttachmentRenderer.vue";
import { classifyAttachment } from "./attachment/utils/AttachmentClassifier";
import { AttachmentType } from "./attachment/types/AttachmentType";
import type { Attachment } from "../../core/stores/chatManager";

const props = defineProps<{ file: Attachment; index: number }>();
const emit = defineEmits<{ (e: "remove", index: number): void }>();

const attachmentType = computed(() => 
  classifyAttachment(props.file.type, props.file.name)
);

const isImage = computed(() => attachmentType.value === AttachmentType.IMAGE);

const isLoading = computed(() => props.file.status === "loading");
</script>

<template>
  <div
    class="relative shrink-0 flex items-center bg-[var(--secondary-bg)] border border-[var(--border-color)] rounded-xl"
    :class="[isImage ? 'w-14 h-14' : 'px-2 py-1.5 max-w-[180px] h-12 gap-2']"
  >
    <!-- Use new component system -->
    <AttachmentRenderer 
      :file="file" 
      :index="index"
      :show-remove="true"
      @remove="emit('remove', index)"
    />

    <!-- Loading Overlay -->
    <div
      v-if="isLoading"
      class="absolute inset-0 bg-black/60 backdrop-blur-[2px] rounded-xl flex flex-col items-center justify-center z-10 gap-1"
    >
      <div
        class="w-5 h-5 border-2 border-white/30 border-t-white rounded-full animate-spin"
      ></div>
      <span
        v-if="file.progress"
        class="text-[9px] text-white font-bold tabular-nums"
        >{{ file.progress }}%</span
      >
    </div>
  </div>
</template>

<style scoped>
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
