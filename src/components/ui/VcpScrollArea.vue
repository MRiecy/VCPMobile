<script setup lang="ts">
import { computed } from 'vue';

const props = withDefaults(defineProps<{
  /**
   * If true, prevents the global swipe-to-close gestures from triggering
   * when scrolling horizontally within this area.
   */
  disableSwipe?: boolean;
}>(), {
  disableSwipe: false
});

const containerClasses = computed(() => {
  return [
    'vcp-scrollable',
    props.disableSwipe ? 'no-swipe' : ''
  ].filter(Boolean).join(' ');
});
</script>

<template>
  <div :class="containerClasses">
    <slot></slot>
  </div>
</template>
