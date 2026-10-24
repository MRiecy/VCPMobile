<script setup lang="ts">
import { ref, computed, onUnmounted, watch } from "vue";
import Sortable from "sortablejs";
import { useAssistantStore } from "../../core/stores/assistant";
import { useChatSessionStore } from "../../core/stores/chatSessionStore";
import { useChatStreamStore } from "../../core/stores/chatStreamStore";
import { useLayoutStore } from "../../core/stores/layout";
import { useSettingsStore } from "../../core/stores/settings";
import { useOverlayStore } from "../../core/stores/overlay";
import VcpAvatar from "../../components/ui/VcpAvatar.vue";
import { sortByOrder } from "./agentOrder";
import { activeSwipeId, currentSwipeX, MAX_SWIPE } from "./swipeController";
import type {
  AgentListItemDto,
  AssistantListItem,
  GroupListItemDto,
} from "../../core/types/assistant";

const props = defineProps<{
  searchQuery: string;
}>();

const emit = defineEmits<{
  (e: "select-agent", item: AgentListItemDto & { type: "agent" }): void;
  (e: "select-group", item: GroupListItemDto & { type: "group" }): void;
}>();

const assistantStore = useAssistantStore();
const sessionStore = useChatSessionStore();
const streamStore = useChatStreamStore();
const layoutStore = useLayoutStore();
const settingsStore = useSettingsStore();
const overlayStore = useOverlayStore();

// --- Sorting Logic ---
const groupListRef = ref<HTMLElement | null>(null);
const agentListRef = ref<HTMLElement | null>(null);

const orderedGroups = computed(() => {
  const groups = assistantStore.groups;
  const order = settingsStore.settings?.navigation.groupOrder || [];
  return sortByOrder(groups, order);
});

const orderedAgents = computed(() => {
  const agents = assistantStore.agents;
  const order = settingsStore.settings?.navigation.agentOrder || [];
  return sortByOrder(agents, order);
});

// --- Shared Swipe State ---
const isSorting = ref(false);
let startX = 0;
let startY = 0;
const isDragging = ref(false);

// 拖动排序开始时的轻触觉反馈（可选增强，不支持震动的设备静默降级）
const triggerDragHaptic = () => {
  try {
    navigator.vibrate?.(20);
  } catch {
    // Haptics are an optional enhancement; unsupported devices stay silent.
  }
};

let groupSortable: Sortable | null = null;
let agentSortable: Sortable | null = null;

const createSortable = (
  element: HTMLElement,
  ownerType: "agent" | "group",
) => Sortable.create(element, {
  animation: 150,
  handle: ".drag-handle",
  delay: 200,
  delayOnTouchOnly: true,
  touchStartThreshold: 3,
  direction: "vertical",
  forceFallback: true,
  fallbackOnBody: true,
  disabled: Boolean(props.searchQuery.trim()),
  ghostClass: "opacity-50",
  onChoose: () => {
    isSorting.value = true;
    isDragging.value = false;
    activeSwipeId.value = null;
    currentSwipeX.value = 0;
  },
  onUnchoose: () => {
    isSorting.value = false;
  },
  onStart: () => {
    isSorting.value = true;
    triggerDragHaptic();
  },
  onEnd: (evt) => {
    isSorting.value = false;
    if (
      evt.oldIndex === undefined ||
      evt.newIndex === undefined ||
      evt.oldIndex === evt.newIndex
    ) return;

    const newOrder = (ownerType === "group"
      ? orderedGroups.value
      : orderedAgents.value
    ).map((item) => item.id);
    const [movedItem] = newOrder.splice(evt.oldIndex, 1);
    if (!movedItem) return;
    newOrder.splice(evt.newIndex, 0, movedItem);
    void settingsStore.updateFields(
      ownerType === "group"
        ? { 'navigation.groupOrder': newOrder }
        : { 'navigation.agentOrder': newOrder },
    ).catch((error) => {
      console.error(`[AgentList] Failed to persist ${ownerType} order:`, error);
    });
  },
});

const destroyGroupSortable = () => {
  groupSortable?.destroy();
  groupSortable = null;
};

const destroyAgentSortable = () => {
  agentSortable?.destroy();
  agentSortable = null;
};

watch(
  groupListRef,
  (element) => {
    destroyGroupSortable();
    if (element) groupSortable = createSortable(element, "group");
  },
  { flush: "post" },
);

watch(
  agentListRef,
  (element) => {
    destroyAgentSortable();
    if (element) agentSortable = createSortable(element, "agent");
  },
  { flush: "post" },
);

watch(
  () => props.searchQuery.trim(),
  (query) => {
    const disabled = Boolean(query);
    groupSortable?.option("disabled", disabled);
    agentSortable?.option("disabled", disabled);
  },
);

onUnmounted(() => {
  destroyGroupSortable();
  destroyAgentSortable();
  isSorting.value = false;
});

// --- Swipe Action Logic (Right Swipe) ---
let isVerticalScroll = false;
let hasDeterminedDirection = false;
let isStartedAsSwiped = false;
const SWIPE_THRESHOLD = 50;
const ownerSwipeKey = (type: "agent" | "group", id: string) => `${type}:${id}`;

const onTouchStart = (e: TouchEvent, id: string) => {
  if (isSorting.value) return;
  if (activeSwipeId.value && activeSwipeId.value !== id) {
    activeSwipeId.value = null;
    currentSwipeX.value = 0;
  }
  startX = e.touches[0].clientX;
  startY = e.touches[0].clientY;
  isDragging.value = true;
  isVerticalScroll = false;
  
  isStartedAsSwiped = activeSwipeId.value === id;
  
  if (!isStartedAsSwiped) {
    // 核心修复：如果是从折叠状态开始滑动，必须强制把 currentSwipeX 重置为 0，防止残留历史滑动值！
    currentSwipeX.value = 0;
  }
  
  if (isStartedAsSwiped) {
    // 核心修复：如果是已展开的卡片被二次触摸，我们强制锁定水平手势，绝不进入垂直滚动，保证能顺滑收回折叠
    hasDeterminedDirection = true;
  } else {
    hasDeterminedDirection = false;
  }
};

const onTouchMove = (e: TouchEvent, id: string) => {
  if (isSorting.value) {
    // 排序期间直接放行 touchmove 事件，以确保 SortableJS 能接收到手势进行拖拽位置计算
    return;
  }
  if (!isDragging.value || isVerticalScroll) return;

  const currentX = e.touches[0].clientX;
  const currentY = e.touches[0].clientY;
  const deltaX = currentX - startX;
  const deltaY = currentY - startY;

  // Determine direction once per gesture
  if (!hasDeterminedDirection) {
    const absX = Math.abs(deltaX);
    const absY = Math.abs(deltaY);

    if (absX > 3 || absY > 3) {
      hasDeterminedDirection = true;
      // If slope is greater than tan(30deg) (~0.577), it's vertical
      if (absY / absX > 0.577) {
        isVerticalScroll = true;
        isDragging.value = false;
        return;
      }
    } else {
      return;
    }
  }

  // 核心手势状态分流与防穿透逻辑：使用整个手势周期内固定不变的 isStartedAsSwiped 状态
  if (!isStartedAsSwiped) {
    // 1. 卡片初始状态处于折叠时：
    if (deltaX < 0) {
      // 向左滑：用户试图左滑关闭侧边栏。
      // 我们不响应卡片滑动，不阻止默认滚动（允许侧边栏滑动），不阻止冒泡
      // 让 touch 事件流完整传给外层侧边栏以收起侧边栏
      isDragging.value = false;
      return;
    } else {
      // 向右滑：用户意图展开卡片。
      // 我们响应卡片滑动，阻止默认行为（防垂直滚动震颤）并阻止冒泡防止外层触发其他手势
      if (e.cancelable) {
        e.preventDefault();
      }
      e.stopPropagation();
      activeSwipeId.value = id;
      currentSwipeX.value = Math.min(deltaX, MAX_SWIPE + 20); // 弹性阻尼展开
    }
  } else {
    // 2. 卡片初始状态处于已展开时：
    // 无论左滑还是右滑，都是用户在此卡片上的精细调节操作
    // 必须阻止默认行为防止垂直滚动震颤，并阻止事件冒泡以防误触发侧边栏关闭
    if (e.cancelable) {
      e.preventDefault();
    }
    e.stopPropagation();

    if (deltaX < 0) {
      // 向左滑：让卡片位移跟随手指做“阻尼平滑缩回渐变”（MAX_SWIPE 加上负的 deltaX，拒绝生硬跳变与震颤！）
      currentSwipeX.value = Math.max(0, MAX_SWIPE + deltaX);
    } else {
      // 向右滑：在 MAX_SWIPE 之上做微阻尼延伸
      currentSwipeX.value = MAX_SWIPE + Math.min(deltaX, 20);
    }
  }
};

const onTouchEnd = (e: TouchEvent, id: string) => {
  if (isSorting.value) return;
  if (!isDragging.value) return;
  isDragging.value = false;

  const wasSwiped = activeSwipeId.value === id;
  const shouldKeepOpen = wasSwiped && currentSwipeX.value > SWIPE_THRESHOLD;

  if (shouldKeepOpen) {
    currentSwipeX.value = MAX_SWIPE; // Snap open
    e.stopPropagation(); // 仅当成功展开或保持展开时，才阻止冒泡以防触发侧边栏手势
  } else {
    activeSwipeId.value = null;
    currentSwipeX.value = 0; // Snap closed
    // 如果没有展开（折叠回去，或者本来就是折叠的），不阻止冒泡，让 touch 事件顺利传递，保证能左滑关闭侧边栏
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
  const agent = assistantStore.agents.find((a) => a.id === agentId);
  if (agent) {
    emit("select-agent", { ...agent, type: "agent" });
  }
};

const selectGroup = async (groupId: string) => {
  const group = assistantStore.groups.find((g) => g.id === groupId);
  if (group) {
    emit("select-group", { ...group, type: "group" });
  }
};

const filteredCombinedItems = computed<AssistantListItem[]>(() => {
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
      <div ref="groupListRef" class="space-y-2 px-1" :class="{ 'no-swipe': isSorting }">
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
          <div @click="selectGroup(group.id)" @touchstart="onTouchStart($event, ownerSwipeKey('group', group.id))"
            @touchmove="onTouchMove($event, ownerSwipeKey('group', group.id))" @touchend="onTouchEnd($event, ownerSwipeKey('group', group.id))"
            class="relative p-3 glass-panel rounded-xl flex items-center gap-3 border shadow-sm cursor-pointer z-10 w-full active:opacity-75 origin-center transition-all duration-300"
            :class="[
                sessionStore.currentSelectedItem?.type === 'group' && sessionStore.currentSelectedItem?.id === group.id
                  ? 'glass-panel-active'
                  : 'border-transparent hover:bg-black/5 dark:hover:bg-white/5',
                (activeSwipeId === ownerSwipeKey('group', group.id) && isDragging) ? 'transition-none' : '',
              ]" :style="{
                transform: `translateX(${activeSwipeId === ownerSwipeKey('group', group.id) ? currentSwipeX : 0}px)`,
              }">
            <div
              class="vcp-sidebar-avatar-slot relative w-10 h-10 shrink-0 rounded-full pointer-events-none"
              :class="{ 'is-generating': streamStore.isOwnerGenerating(group.id, 'group') }"
              :data-generating="streamStore.isOwnerGenerating(group.id, 'group') ? 'true' : 'false'"
            >
              <VcpAvatar owner-type="group" :owner-id="group.id" :fallback-name="group.name" size="w-10 h-10"
                rounded="rounded-full" class="vcp-sidebar-avatar-surface pointer-events-none" dominant-color="var(--highlight-text)" />
              <div
                class="vcp-sidebar-avatar-indicator absolute inset-0 rounded-full overflow-hidden flex items-center justify-center pointer-events-none"
                aria-hidden="true"
              >
                <div class="vignette-mask absolute inset-0"></div>
                <svg class="relative z-10 w-6 h-6" viewBox="0 0 24 24" fill="none">
                  <line class="axis-line" x1="2.5" y1="12" x2="21.5" y2="12" stroke="rgba(255,255,255,0.34)" stroke-width="0.9" stroke-linecap="round" />
                  <g transform="rotate(7 12 12)">
                    <rect class="interlace-up st-2" x="4.8" y="9.2" width="1.65" height="5.6" rx="0.82" fill="rgba(255,255,255,0.68)" />
                    <rect class="interlace-down st-1" x="8.1" y="6.0" width="1.85" height="12.0" rx="0.92" fill="rgba(255,255,255,0.9)" />
                    <rect class="interlace-up st-0" x="11.6" y="3.4" width="2.0" height="17.2" rx="1.0" fill="#ffffff" />
                    <rect class="interlace-down st-1" x="15.3" y="7.4" width="1.8" height="9.2" rx="0.9" fill="rgba(255,255,255,0.85)" />
                    <circle class="interlace-dot st-2" cx="19.3" cy="12" r="1.25" fill="rgba(255,255,255,0.72)" />
                  </g>
                </svg>
              </div>
            </div>
            <div class="flex flex-col overflow-hidden flex-1 pointer-events-none">
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
      <div ref="agentListRef" class="space-y-2 px-1" :class="{ 'no-swipe': isSorting }">
        <div v-for="agent in orderedAgents.filter(
          (agent) =>
            !searchQuery.trim() ||
            agent.name
              .toLowerCase()
              .includes(searchQuery.toLowerCase().trim()),
        )" :key="agent.id" v-guide="'agent-row-' + agent.id" class="relative w-full drag-handle mb-2">
          
          <!-- 背景设置按钮 -->
          <div class="absolute inset-0 rounded-xl overflow-hidden z-0 pointer-events-none">
            <div class="absolute inset-0 bg-black/10 dark:bg-white/10 flex items-center justify-start">
              <div
                v-guide="'agent-row-settings-' + agent.id"
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
          <div @click="selectAgent(agent.id)" @touchstart="onTouchStart($event, ownerSwipeKey('agent', agent.id))"
            @touchmove="onTouchMove($event, ownerSwipeKey('agent', agent.id))" @touchend="onTouchEnd($event, ownerSwipeKey('agent', agent.id))"
            class="relative p-3 glass-panel rounded-xl flex items-center gap-3 border shadow-sm cursor-pointer z-10 w-full active:opacity-75 origin-center transition-all duration-300"
            :class="[
              sessionStore.currentSelectedItem?.type === 'agent' && sessionStore.currentSelectedItem?.id === agent.id
                ? 'glass-panel-active'
                : 'border-transparent hover:bg-black/5 dark:hover:bg-white/5',
              (activeSwipeId === ownerSwipeKey('agent', agent.id) && isDragging) ? 'transition-none' : '',
            ]" :style="{
            transform: `translateX(${activeSwipeId === ownerSwipeKey('agent', agent.id) ? currentSwipeX : 0}px)`,
          }">
            <div v-if="
              assistantStore.unreadCounts[`agent:${agent.id}`] === -1 ||
              assistantStore.unreadCounts[`agent:${agent.id}`] > 0
            " class="absolute -top-1 -right-1 w-3 h-3 rounded-full border-2 border-white dark:border-gray-900 z-10 shadow-sm shrink-0"
              style="background: #ff6b6b"></div>

            <div
              class="vcp-sidebar-avatar-slot relative w-10 h-10 shrink-0 rounded-full pointer-events-none"
              :class="{ 'is-generating': streamStore.isOwnerGenerating(agent.id, 'agent') }"
              :data-generating="streamStore.isOwnerGenerating(agent.id, 'agent') ? 'true' : 'false'"
            >
              <VcpAvatar
                owner-type="agent"
                :owner-id="agent.id"
                :fallback-name="agent.name"
                size="w-10 h-10"
                rounded="rounded-full"
                class="vcp-sidebar-avatar-surface pointer-events-none"
                dominant-color="var(--highlight-text)"
              />
              <div
                class="vcp-sidebar-avatar-indicator absolute inset-0 rounded-full overflow-hidden flex items-center justify-center pointer-events-none"
                aria-hidden="true"
              >
                <div class="vignette-mask absolute inset-0"></div>
                <svg class="relative z-10 w-6 h-6" viewBox="0 0 24 24" fill="none">
                  <line class="axis-line" x1="2.5" y1="12" x2="21.5" y2="12" stroke="rgba(255,255,255,0.34)" stroke-width="0.9" stroke-linecap="round" />
                  <g transform="rotate(7 12 12)">
                    <rect class="interlace-up st-2" x="4.8" y="9.2" width="1.65" height="5.6" rx="0.82" fill="rgba(255,255,255,0.68)" />
                    <rect class="interlace-down st-1" x="8.1" y="6.0" width="1.85" height="12.0" rx="0.92" fill="rgba(255,255,255,0.9)" />
                    <rect class="interlace-up st-0" x="11.6" y="3.4" width="2.0" height="17.2" rx="1.0" fill="#ffffff" />
                    <rect class="interlace-down st-1" x="15.3" y="7.4" width="1.8" height="9.2" rx="0.9" fill="rgba(255,255,255,0.85)" />
                    <circle class="interlace-dot st-2" cx="19.3" cy="12" r="1.25" fill="rgba(255,255,255,0.72)" />
                  </g>
                </svg>
              </div>
            </div>
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

<style scoped>
.vcp-sidebar-avatar-surface {
  transition:
    transform 0.44s cubic-bezier(0.22, 1, 0.36, 1),
    filter 0.44s cubic-bezier(0.22, 1, 0.36, 1);
}

.vcp-sidebar-avatar-slot.is-generating .vcp-sidebar-avatar-surface {
  filter: brightness(0.63) saturate(0.82) contrast(1.06);
  transform: scale(0.96);
}

.vignette-mask {
  opacity: 0;
  background: radial-gradient(
    circle at 50% 50%,
    rgba(0, 0, 0, 0.6) 0%,
    rgba(0, 0, 0, 0.24) 64%,
    transparent 100%
  );
  transition: opacity 0.38s ease;
}

.vcp-sidebar-avatar-slot.is-generating .vignette-mask {
  opacity: 1;
}

.axis-line {
  transform-origin: 55% 50%;
  transform: scaleX(0);
  opacity: 0;
  transition:
    transform 0.42s cubic-bezier(0.16, 1, 0.3, 1),
    opacity 0.3s ease;
}

.vcp-sidebar-avatar-slot.is-generating .axis-line {
  transform: scaleX(1);
  opacity: 1;
}

.interlace-up {
  transform-origin: center center;
  transform: translateY(6.5px) scaleY(0.18);
  opacity: 0;
  transition:
    transform 0.46s cubic-bezier(0.22, 1, 0.36, 1),
    opacity 0.32s ease;
}

.interlace-down {
  transform-origin: center center;
  transform: translateY(-6.5px) scaleY(0.18);
  opacity: 0;
  transition:
    transform 0.46s cubic-bezier(0.22, 1, 0.36, 1),
    opacity 0.32s ease;
}

.interlace-dot {
  transform-origin: center center;
  transform: translateY(4.5px) scale(0.2);
  opacity: 0;
  transition:
    transform 0.46s cubic-bezier(0.34, 1.56, 0.64, 1),
    opacity 0.32s ease;
}

.vcp-sidebar-avatar-slot.is-generating .interlace-up,
.vcp-sidebar-avatar-slot.is-generating .interlace-down {
  transform: translateY(0) scaleY(1);
  opacity: 1;
}

.vcp-sidebar-avatar-slot.is-generating .interlace-dot {
  transform: translateY(0) scale(1);
  opacity: 1;
}

.st-0 {
  transition-delay: 0s;
}

.st-1 {
  transition-delay: 0.04s;
}

.st-2 {
  transition-delay: 0.085s;
}

@media (prefers-reduced-motion: reduce) {
  .vcp-sidebar-avatar-surface,
  .vignette-mask,
  .axis-line,
  .interlace-up,
  .interlace-down,
  .interlace-dot {
    transition: none !important;
  }
}
</style>
