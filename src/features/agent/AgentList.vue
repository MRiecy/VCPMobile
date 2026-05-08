<script setup lang="ts">
import { ref, computed, onMounted } from "vue";
import Sortable from "sortablejs";
import { useAssistantStore } from "../../core/stores/assistant";
import { useChatSessionStore } from "../../core/stores/chatSessionStore";
import { useLayoutStore } from "../../core/stores/layout";
import { useSettingsStore } from "../../core/stores/settings";
import { useOverlayStore } from "../../core/stores/overlay";
import VcpAvatar from "../../components/ui/VcpAvatar.vue";

const props = defineProps<{
  searchQuery: string;
}>();

const emit = defineEmits<{
  (e: "select-agent", item: any): void;
  (e: "select-group", item: any): void;
}>();

const assistantStore = useAssistantStore();
const sessionStore = useChatSessionStore();
const layoutStore = useLayoutStore();
const settingsStore = useSettingsStore();
const overlayStore = useOverlayStore();

// --- Sorting Logic ---
const groupListRef = ref<HTMLElement | null>(null);
const agentListRef = ref<HTMLElement | null>(null);

const orderedGroups = computed(() => {
  const groups = assistantStore.groups;
  const order = settingsStore.settings?.groupOrder || [];
  if (order.length === 0) return groups;

  const sorted = [...groups].sort((a, b) => {
    const indexA = order.indexOf(a.id);
    const indexB = order.indexOf(b.id);
    if (indexA === -1 && indexB === -1) return 0;
    if (indexA === -1) return 1;
    if (indexB === -1) return -1;
    return indexA - indexB;
  });
  return sorted;
});

const orderedAgents = computed(() => {
  const agents = assistantStore.agents;
  const order = settingsStore.settings?.agentOrder || [];
  if (order.length === 0) return agents;

  const sorted = [...agents].sort((a, b) => {
    const indexA = order.indexOf(a.id);
    const indexB = order.indexOf(b.id);
    if (indexA === -1 && indexB === -1) return 0;
    if (indexA === -1) return 1;
    if (indexB === -1) return -1;
    return indexA - indexB;
  });
  return sorted;
});

const initSortable = () => {
  if (groupListRef.value) {
    Sortable.create(groupListRef.value, {
      animation: 150,
      handle: ".drag-handle",
      delay: 200,
      delayOnTouchOnly: true,
      onEnd: (evt) => {
        const newOrder = orderedGroups.value.map((g) => g.id);
        const [movedItem] = newOrder.splice(evt.oldIndex!, 1);
        newOrder.splice(evt.newIndex!, 0, movedItem);
        settingsStore.updateSettings({ groupOrder: newOrder });
      },
    });
  }

  if (agentListRef.value) {
    Sortable.create(agentListRef.value, {
      animation: 150,
      handle: ".drag-handle",
      delay: 200, // Important for mobile: delay so swipe isn't blocked by sort
      delayOnTouchOnly: true,
      onEnd: (evt) => {
        const newOrder = orderedAgents.value.map((a) => a.id);
        const [movedItem] = newOrder.splice(evt.oldIndex!, 1);
        newOrder.splice(evt.newIndex!, 0, movedItem);
        settingsStore.updateSettings({ agentOrder: newOrder });
      },
    });
  }
};

onMounted(() => {
  initSortable();
});

// --- Swipe Action Logic (Right Swipe) ---
const activeSwipeId = ref<string | null>(null);
const currentSwipeX = ref(0);
let startX = 0;
let startY = 0;
let isDragging = false;
let isVerticalScroll = false;
let hasDeterminedDirection = false;
const SWIPE_THRESHOLD = 50;
const MAX_SWIPE = 80;

const onTouchStart = (e: TouchEvent, id: string) => {
  if (activeSwipeId.value && activeSwipeId.value !== id) {
    activeSwipeId.value = null;
    currentSwipeX.value = 0;
  }
  startX = e.touches[0].clientX;
  startY = e.touches[0].clientY;
  isDragging = true;
  isVerticalScroll = false;
  hasDeterminedDirection = false;
};

const onTouchMove = (e: TouchEvent, id: string) => {
  if (!isDragging || isVerticalScroll) return;

  const currentX = e.touches[0].clientX;
  const currentY = e.touches[0].clientY;
  const deltaX = currentX - startX;
  const deltaY = currentY - startY;

  // Determine direction once per gesture
  if (!hasDeterminedDirection) {
    const absX = Math.abs(deltaX);
    const absY = Math.abs(deltaY);

    if (absX > 5 || absY > 5) {
      hasDeterminedDirection = true;
      // If slope is greater than tan(30deg) (~0.577), it's vertical
      if (absY / absX > 0.577) {
        isVerticalScroll = true;
        isDragging = false;
        return;
      }
    } else {
      return;
    }
  }

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

const goToSettings = (id: string, type: 'agent' | 'group' = 'agent') => {
  activeSwipeId.value = null;
  currentSwipeX.value = 0;
  // 核心修复：跳转前强制关闭侧边栏
  layoutStore.setLeftDrawer(false);
  
  if (type === 'agent') {
    overlayStore.openAgentSettings(id);
  } else {
    overlayStore.openGroupSettings(id);
  }
};

const selectAgent = async (agentId: string) => {
  const agent = assistantStore.agents.find((a: any) => a.id === agentId);
  if (agent) {
    emit("select-agent", agent);
  }
};

const selectGroup = async (groupId: string) => {
  const group = assistantStore.groups.find((g) => g.id === groupId);
  if (group) {
    emit("select-group", group);
  }
};

const filteredCombinedItems = computed(() => {
  const query = props.searchQuery.toLowerCase().trim();
  if (!query) return assistantStore.combinedItems;
  return assistantStore.combinedItems.filter((item) =>
    item.name.toLowerCase().includes(query),
  );
});
</script>

<template>
  <div v-if="assistantStore.loading" class="flex justify-center p-8 opacity-50">
    <svg class="animate-spin h-6 w-6 text-primary-text" viewBox="0 0 24 24" fill="none">
      <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
      <path class="opacity-75" fill="currentColor"
        d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z">
      </path>
    </svg>
  </div>
  <div v-else-if="filteredCombinedItems.length === 0" class="text-center p-8 opacity-30 text-sm">
    未找到助手或群组
  </div>
  <div v-else class="space-y-4">
    <div v-if="assistantStore.groups.length > 0" class="space-y-2">
      <h3 class="px-2 text-[10px] font-black uppercase tracking-widest opacity-30">
        Agent Groups
      </h3>
      <div ref="groupListRef" class="space-y-2 px-1">
        <div v-for="group in orderedGroups.filter(
          (group) =>
            !searchQuery.trim() ||
            group.name
              .toLowerCase()
              .includes(searchQuery.toLowerCase().trim()),
        )" :key="group.id" class="relative w-full drag-handle mb-2">
          
          <!-- 背景设置按钮 -->
          <div class="absolute inset-0 rounded-xl overflow-hidden z-0 pointer-events-none">
            <div class="absolute inset-0 bg-black/10 dark:bg-white/10 flex items-center justify-start">
              <div
                class="w-[80px] h-full flex items-center justify-center text-purple-600/70 dark:text-purple-400/70 hover:text-purple-600 dark:hover:text-purple-400 transition-colors cursor-pointer active:bg-black/5 dark:active:bg-white/5 pointer-events-auto"
                @click.stop="goToSettings(group.id, 'group')"
                @touchstart.stop>
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"
                  stroke-linecap="round" stroke-linejoin="round">
                  <circle cx="12" cy="12" r="3"></circle>
                  <path
                    d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z">
                  </path>
                </svg>
              </div>
            </div>
          </div>

          <!-- 滑动与玻璃层 -->
          <div @click="selectGroup(group.id)" @touchstart="onTouchStart($event, group.id)"
            @touchmove="onTouchMove($event, group.id)" @touchend="onTouchEnd(group.id)"
            class="relative p-3 glass-panel rounded-xl flex items-center gap-3 border shadow-sm cursor-pointer z-10 w-full active:scale-[0.98] origin-center transition-all duration-300"
            :class="[
                sessionStore.currentSelectedItem?.id === group.id
                  ? 'glass-panel-active'
                  : 'border-transparent hover:bg-black/5 dark:hover:bg-white/5',
                activeSwipeId === group.id ? 'transition-none' : '',
              ]" :style="{
                transform: `translateX(${activeSwipeId === group.id ? currentSwipeX : 0}px)`,
              }">
            <VcpAvatar owner-type="group" :owner-id="group.id" :fallback-name="group.name" size="w-10 h-10"
              rounded="rounded-full" dominant-color="var(--highlight-text)" />
            <div class="flex flex-col overflow-hidden flex-1">
              <span class="font-bold text-sm truncate text-primary-text">{{
                group.name
              }}</span>
              <span class="text-[9px] text-secondary-text opacity-80 truncate uppercase tracking-tighter">{{ group.members.length }} Members
                • {{ group.mode }}</span>
            </div>
          </div>
        </div>
      </div>
    </div>

    <div v-if="assistantStore.agents.length > 0" class="space-y-2">
      <h3 class="px-2 text-[10px] font-black uppercase tracking-widest opacity-30">
        Individual Agents
      </h3>
      <div ref="agentListRef" class="space-y-2 px-1">
        <div v-for="agent in orderedAgents.filter(
          (agent) =>
            !searchQuery.trim() ||
            agent.name
              .toLowerCase()
              .includes(searchQuery.toLowerCase().trim()),
        )" :key="agent.id" class="relative w-full drag-handle mb-2">
          
          <!-- 背景设置按钮 -->
          <div class="absolute inset-0 rounded-xl overflow-hidden z-0 pointer-events-none">
            <div class="absolute inset-0 bg-black/10 dark:bg-white/10 flex items-center justify-start">
              <div
                class="w-[80px] h-full flex items-center justify-center text-blue-600/70 dark:text-blue-400/70 hover:text-blue-600 dark:hover:text-blue-400 transition-colors cursor-pointer active:bg-black/5 dark:active:bg-white/5 pointer-events-auto"
                @click.stop="goToSettings(agent.id, 'agent')"
                @touchstart.stop>
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"
                  stroke-linecap="round" stroke-linejoin="round">
                  <circle cx="12" cy="12" r="3"></circle>
                  <path
                    d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z">
                  </path>
                </svg>
              </div>
            </div>
          </div>

          <!-- 滑动与玻璃层 -->
          <div @click="selectAgent(agent.id)" @touchstart="onTouchStart($event, agent.id)"
            @touchmove="onTouchMove($event, agent.id)" @touchend="onTouchEnd(agent.id)"
            class="relative p-3 glass-panel rounded-xl flex items-center gap-3 border shadow-sm cursor-pointer z-10 w-full active:scale-[0.98] origin-center transition-all duration-300"
            :class="[
              sessionStore.currentSelectedItem?.id === agent.id
                ? 'glass-panel-active'
                : 'border-transparent hover:bg-black/5 dark:hover:bg-white/5',
              activeSwipeId === agent.id ? 'transition-none' : '',
            ]" :style="{
            transform: `translateX(${activeSwipeId === agent.id ? currentSwipeX : 0}px)`,
          }">
            <div v-if="
              assistantStore.unreadCounts[agent.id] === -1 ||
              assistantStore.unreadCounts[agent.id] > 0
            " class="absolute -top-1 -right-1 w-3 h-3 rounded-full border-2 border-white dark:border-gray-900 z-10 shadow-sm shrink-0"
              style="background: #ff6b6b"></div>

            <VcpAvatar
              owner-type="agent"
              :owner-id="agent.id"
              :fallback-name="agent.name"
              size="w-10 h-10"
              rounded="rounded-full"
              class="pointer-events-none"
              dominant-color="var(--highlight-text)"
            />
            <div class="flex flex-col overflow-hidden flex-1 pointer-events-none">
              <span class="font-bold text-sm truncate text-primary-text">{{
                agent.name
                }}</span>
              <span class="text-[10px] text-secondary-text opacity-80 truncate">{{
                agent.model
                }}</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
