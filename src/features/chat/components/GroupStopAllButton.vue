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
         class="absolute -top-12 left-1/2 -translate-x-1/2 z-local">
      <button 
        @click="handleStopAll"
        class="flex items-center gap-2 px-3.5 py-1.5 bg-zinc-900 dark:bg-black border border-red-500/30 rounded text-red-400 text-xs font-semibold tracking-wide shadow-[0_0_15px_rgba(239,68,68,0.12)] active:scale-[0.97] transition-all duration-200"
      >
        <Octagon :size="12" class="animate-pulse text-red-500" />
        <span class="uppercase tracking-wider font-medium">停止群组发言</span>
        <span class="px-1.5 py-0.5 rounded bg-red-950/60 text-[10px] font-mono border border-red-900/50 text-red-300 leading-none">{{ activeCount }}</span>
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
  transform: translate(-50%, 15px) scale(0.95);
  opacity: 0;
}
</style>
