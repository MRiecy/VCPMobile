<script setup lang="ts">
import { computed } from 'vue';
import { useChatSessionStore } from '../../../core/stores/chatSessionStore';
import { useChatStreamStore } from '../../../core/stores/chatStreamStore';
import { Octagon } from 'lucide-vue-next';

const sessionStore = useChatSessionStore();
const streamStore = useChatStreamStore();

const isVisible = computed(() => {
  return streamStore.isGroupGenerating && streamStore.activeStreamingIds.size > 0;
});

const activeCount = computed(() => streamStore.activeStreamingIds.size);

const handleStopAll = () => {
  if (sessionStore.currentTopicId) {
    streamStore.stopGroupTurn(sessionStore.currentTopicId);
  }
};
</script>

<template>
  <Transition name="slide-fade">
    <div v-if="isVisible" 
         class="absolute -top-12 left-1/2 -translate-x-1/2 z-50">
      <button 
        @click="handleStopAll"
        class="flex items-center gap-2 px-4 py-2 bg-red-500/80 hover:bg-red-600 backdrop-blur-xl border border-white/20 rounded-full text-white text-xs font-bold shadow-lg shadow-red-500/20 active:scale-95 transition-all"
      >
        <Octagon :size="14" class="animate-pulse" />
        <span>停止整个群组发言 ({{ activeCount }})</span>
      </button>
    </div>
  </Transition>
</template>

<style scoped>
.slide-fade-enter-active,
.slide-fade-leave-active {
  transition: all 0.3s cubic-bezier(0.16, 1, 0.3, 1);
}

.slide-fade-enter-from,
.slide-fade-leave-to {
  transform: translate(-50%, 20px) scale(0.9);
  opacity: 0;
}
</style>
