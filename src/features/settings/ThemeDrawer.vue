<script setup lang="ts">
import { ref } from 'vue';
import ThemePicker from './ThemePicker.vue';
import { X } from 'lucide-vue-next';

const isOpen = ref(false);

const open = () => {
  isOpen.value = true;
};

const close = () => {
  isOpen.value = false;
};

defineExpose({ open, close });
</script>

<template>
  <Teleport to="#vcp-overlays">
    <!-- Backdrop -->
    <Transition name="fade">
      <div v-if="isOpen" @click="close" class="fixed inset-0 bg-black/50 backdrop-blur-sm pointer-events-auto"></div>
    </Transition>

    <!-- Drawer -->
    <Transition name="slide-up">
      <div v-if="isOpen" class="fixed bottom-0 left-0 right-0 bg-white dark:bg-gray-800 rounded-t-3xl shadow-2xl max-h-[80vh] flex flex-col overflow-hidden pb-safe pointer-events-auto">
        <!-- Handle -->
        <div class="flex-center py-3">
          <div class="w-12 h-1.5 bg-gray-300 dark:bg-gray-600 rounded-full"></div>
        </div>

        <!-- Header -->
        <div class="flex items-center justify-between px-6 pb-4">
          <h3 class="text-xl font-bold">主题抽屉</h3>
          <button @click="close" class="p-2 bg-gray-100 dark:bg-gray-700 rounded-full text-gray-500">
            <X :size="20" />
          </button>
        </div>

        <!-- Content -->
        <div class="flex-1 overflow-y-auto px-6 pb-6">
          <ThemePicker layout="grid" />
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.slide-up-enter-active,
.slide-up-leave-active {
  transition: transform 0.3s cubic-bezier(0.32, 0.72, 0, 1);
}

.slide-up-enter-from,
.slide-up-leave-to {
  transform: translateY(100%);
}

.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.3s ease;
}

.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>
