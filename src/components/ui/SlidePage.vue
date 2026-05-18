<script setup lang="ts">
import { LAYER_PAGE_BASE } from '../../core/constants/layers';

interface Props {
  isOpen: boolean;
  zIndex?: number;
}

const props = withDefaults(defineProps<Props>(), {
  zIndex: LAYER_PAGE_BASE,
});
</script>

<template>
  <Transition name="slide-page">
    <div
      v-show="props.isOpen"
      class="fixed inset-0 pointer-events-auto"
      :style="{ zIndex: props.zIndex }"
    >
      <slot />
    </div>
  </Transition>
</template>

<style scoped>
.slide-page-enter-active {
  transition: transform 0.35s cubic-bezier(0.32, 0.72, 0, 1);
}

.slide-page-leave-active {
  transition: transform 0.3s cubic-bezier(0.32, 0.72, 0, 1);
}

.slide-page-enter-from,
.slide-page-leave-to {
  transform: translateX(100%);
}
</style>