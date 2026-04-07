<script setup lang="ts">
import { ref, computed } from 'vue';
import { useRouter } from 'vue-router';
import { useAssistantStore } from '../../core/stores/assistant';
import { useChatManagerStore } from '../../core/stores/chatManager';
import { useTopicStore } from '../../core/stores/topicListManager';
import { useLayoutStore } from '../../core/stores/layout';
import { Users } from 'lucide-vue-next';

const props = defineProps<{
  searchQuery: string;
}>();

const emit = defineEmits<{
  (e: 'select-agent'): void;
}>();

const assistantStore = useAssistantStore();
const chatStore = useChatManagerStore();
const topicListStore = useTopicStore();
const layoutStore = useLayoutStore();
const router = useRouter();

// --- Swipe Action Logic (Right Swipe) ---
const activeSwipeId = ref<string | null>(null);
const currentSwipeX = ref(0);
let startX = 0;
let isDragging = false;
const SWIPE_THRESHOLD = 50;
const MAX_SWIPE = 80;

const onTouchStart = (e: TouchEvent, id: string) => {
  if (activeSwipeId.value && activeSwipeId.value !== id) {
    activeSwipeId.value = null;
    currentSwipeX.value = 0;
  }
  startX = e.touches[0].clientX;
  isDragging = true;
};

const onTouchMove = (e: TouchEvent, id: string) => {
  if (!isDragging) return;
  const currentX = e.touches[0].clientX;
  const deltaX = currentX - startX;

  // Only allow rightward swipe (deltaX > 0)
  if (deltaX > 0) {
    activeSwipeId.value = id;
    currentSwipeX.value = Math.min(deltaX, MAX_SWIPE + 20); // Elastic resistance
  } else if (activeSwipeId.value === id) {
    currentSwipeX.value = 0;
  }
};

const onTouchEnd = (id: string) => {
  if (!isDragging) return;
  isDragging = false;
  
  if (activeSwipeId.value === id && currentSwipeX.value > SWIPE_THRESHOLD) {
    currentSwipeX.value = MAX_SWIPE; // Snap open
  } else {
    activeSwipeId.value = null;
    currentSwipeX.value = 0; // Snap closed
  }
};

const goToSettings = (id: string) => {
  activeSwipeId.value = null;
  currentSwipeX.value = 0;
  layoutStore.setLeftDrawer(false);
  router.push('/agents/' + id);
};

const selectAgent = async (agentId: string) => {
  const agent = assistantStore.agents.find((a: any) => a.id === agentId);
  if (agent) {
    chatStore.currentSelectedItem = { id: agent.id, name: agent.name, type: 'agent' };
  }
  await topicListStore.loadTopicList(agentId, 'agent');
  emit('select-agent');
};

const selectGroup = async (groupId: string) => {
  const group = assistantStore.groups.find(g => g.id === groupId);
  if (group) {
    chatStore.currentSelectedItem = { id: group.id, name: group.name, type: 'group' };
  }
  await topicListStore.loadTopicList(groupId, 'group');
  emit('select-agent');
};

const filteredCombinedItems = computed(() => {
  const query = props.searchQuery.toLowerCase().trim();
  if (!query) return assistantStore.combinedItems;
  return assistantStore.combinedItems.filter(item => item.name.toLowerCase().includes(query));
});
</script>

<template>
  <div v-if="assistantStore.loading" class="flex justify-center p-8 opacity-50">
    <svg class="animate-spin h-6 w-6 text-primary-text" viewBox="0 0 24 24" fill="none">
      <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
      <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
    </svg>
  </div>
  <div v-else-if="filteredCombinedItems.length === 0" class="text-center p-8 opacity-30 text-sm">
    未找到助手或群组
  </div>
  <div v-else class="space-y-4">
    <div v-if="assistantStore.groups.length > 0" class="space-y-2">
      <h3 class="px-2 text-[10px] font-black uppercase tracking-widest opacity-30">Agent Groups</h3>
      <div v-for="group in assistantStore.groups.filter(group => !searchQuery.trim() || group.name.toLowerCase().includes(searchQuery.toLowerCase().trim()))" :key="group.id" class="relative rounded-xl overflow-hidden w-full">
        <div @click="selectGroup(group.id)"
             class="relative p-3 glass-panel rounded-xl flex items-center gap-3 border shadow-sm cursor-pointer hover:bg-black/5 dark:hover:bg-white/5 z-10 w-full active:scale-[0.98] transition-all"
             :class="chatStore.currentSelectedItem?.id === group.id ? 'border-purple-500/50 bg-purple-500/10 dark:bg-purple-500/20' : 'border-black/5 dark:border-white/5'">
          
          <div class="w-10 h-10 rounded-xl bg-gradient-to-br from-purple-500/20 to-pink-500/20 flex items-center justify-center shrink-0 border border-black/10 dark:border-white/10 overflow-hidden">
            <img :src="`vcp-avatar://group/${group.id}`" class="w-full h-full object-cover" />
          </div>
          <div class="flex flex-col overflow-hidden flex-1">
            <span class="font-bold text-sm truncate text-primary-text">{{ group.name }}</span>
            <span class="text-[9px] opacity-40 truncate uppercase tracking-tighter">{{ group.members.length }} Members • {{ group.mode }}</span>
          </div>
        </div>
      </div>
    </div>

    <div v-if="assistantStore.agents.length > 0" class="space-y-2">
      <h3 class="px-2 text-[10px] font-black uppercase tracking-widest opacity-30">Individual Agents</h3>
      <div v-for="agent in assistantStore.agents.filter(agent => !searchQuery.trim() || agent.name.toLowerCase().includes(searchQuery.toLowerCase().trim()))" :key="agent.id" class="relative rounded-xl overflow-hidden w-full">
        <div class="absolute inset-0 bg-black/10 dark:bg-white/10 flex items-center justify-start z-0"
             @click.stop="goToSettings(agent.id)">
          <div class="w-[80px] h-full flex items-center justify-center text-blue-600/70 dark:text-blue-400/70 hover:text-blue-600 dark:hover:text-blue-400 transition-colors cursor-pointer active:bg-black/5 dark:active:bg-white/5">
            <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
              <circle cx="12" cy="12" r="3"></circle>
              <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
            </svg>
          </div>
        </div>

        <div @click="selectAgent(agent.id)"
             @touchstart="onTouchStart($event, agent.id)"
             @touchmove="onTouchMove($event, agent.id)"
             @touchend="onTouchEnd(agent.id)"
             class="relative p-3 glass-panel rounded-xl flex items-center gap-3 border shadow-sm cursor-pointer hover:bg-black/5 dark:hover:bg-white/5 z-10 w-full active:scale-[0.98] origin-center"
             :class="[
               chatStore.currentSelectedItem?.id === agent.id ? 'border-blue-500/50 bg-blue-500/10 dark:bg-blue-500/20' : 'border-black/5 dark:border-white/5',
               activeSwipeId === agent.id ? 'transition-none' : 'transition-transform duration-200 ease-out'
             ]"
             :style="{ transform: `translateX(${activeSwipeId === agent.id ? currentSwipeX : 0}px)` }">
             
          <div v-if="assistantStore.unreadCounts[agent.id] === -1 || assistantStore.unreadCounts[agent.id] > 0" class="absolute -top-1 -right-1 w-3 h-3 rounded-full border-2 border-white dark:border-gray-900 z-10 shadow-sm animate-pulse shrink-0" style="background: #ff6b6b;"></div>

          <div class="w-10 h-10 rounded-full bg-gradient-to-br from-blue-500/20 to-purple-500/20 flex items-center justify-center shrink-0 border border-black/10 dark:border-white/10 overflow-hidden pointer-events-none">
            <img :src="`vcp-avatar://agent/${agent.id}`" class="w-full h-full object-cover" />
          </div>
          <div class="flex flex-col overflow-hidden flex-1 pointer-events-none">
            <span class="font-bold text-sm truncate text-primary-text">{{ agent.name }}</span>
            <span class="text-[10px] opacity-40 truncate">{{ agent.model }}</span>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
