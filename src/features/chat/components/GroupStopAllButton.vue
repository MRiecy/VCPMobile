<script setup lang="ts">
import { computed } from 'vue';
import { useChatSessionStore } from '../../../core/stores/chatSessionStore';
import { useChatStreamStore } from '../../../core/stores/chatStreamStore';
import { Octagon } from 'lucide-vue-next';

const sessionStore = useChatSessionStore();
const streamStore = useChatStreamStore();

const isVisible = computed(() => {
  return streamStore.isGroupGenerating;
});

const activeCount = computed(() => streamStore.activeStreamingIds.size);

const handleStopAll = () => {
  if (sessionStore.currentTopicId) {
    const key = sessionStore.currentConversationKey;
    if (key?.ownerType === "group") {
      streamStore.stopGroupTurn(key.ownerId, key.topicId);
    }
  }
};
</script>

<template>
  <Transition name="slide-fade">
    <div v-if="isVisible" 
         class="absolute -top-12 left-1/2 -translate-x-1/2 z-local">
      <button 
        @click="handleStopAll"
        aria-label="中断全部群聊发言"
        class="flex items-center gap-2 min-h-9 px-3.5 py-1.5 bg-zinc-900 dark:bg-black border border-red-500/30 rounded text-red-400 text-xs font-semibold tracking-wide active:opacity-60 transition-opacity duration-200"
      >
        <Octagon :size="12" class="text-red-500" />
        <span class="tracking-wider font-medium">中断群聊</span>
        <span v-if="activeCount > 0" class="px-1.5 py-0.5 rounded bg-red-950/60 text-[10px] font-mono border border-red-900/50 text-red-300 leading-none">{{ activeCount }}</span>
      </button>
    </div>
  </Transition>
</template>

<style scoped>
.slide-fade-enter-active,
.slide-fade-leave-active {
  transition: opacity 0.15s;
}

.slide-fade-enter-from,
.slide-fade-leave-to {
  opacity: 0;
}
</style>
