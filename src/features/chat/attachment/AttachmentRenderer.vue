<template>
  <component
    :is="currentComponent"
    :file="file"
    :index="index"
    :show-remove="showRemove"
    @remove="emit('remove', index)"
  />
</template>

<script setup lang="ts">
import { computed } from 'vue';
import { AttachmentRegistry } from './AttachmentRegistry';
import { AttachmentType } from './types/AttachmentType';
import { classifyAttachment } from './utils/AttachmentClassifier';
import type { Attachment } from '../../../core/types/chat';

interface Props {
  file: Attachment;
  index: number;
  showRemove?: boolean;
}

const props = withDefaults(defineProps<Props>(), {
  showRemove: false
});
const emit = defineEmits<{ (e: "remove", index: number): void }>();

// Classify the attachment type
const attachmentType = computed(() => 
  classifyAttachment(props.file.type, props.file.name)
);

// Get the appropriate component for the attachment type
const currentComponent = computed(() => {
  const component = AttachmentRegistry.getComponent(attachmentType.value);
  return component || AttachmentRegistry.getComponent(AttachmentType.OTHER);
});
</script>