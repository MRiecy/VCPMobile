<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { useSidebarSwipe } from '../../core/composables/useSidebarSwipe';
import { useLayoutStore } from '../../core/stores/layout';
import { useOverlayStore } from '../../core/stores/overlay';
import { useChatSessionStore } from '../../core/stores/chatSessionStore';
import { useTopicStore } from '../../core/stores/topicListManager';
import SidebarTabs from '../../features/agent/SidebarTabs.vue';
import SidebarSearch from '../../features/agent/SidebarSearch.vue';
import AgentList from '../../features/agent/AgentList.vue';
import TopicList from '../../features/topic/TopicList.vue';
import AgentsCreator from '../../features/agent/AgentsCreator.vue';
import TopicCreator from '../../features/topic/TopicCreator.vue';
import { sidebarTab } from '../../features/agent/sidebarTab';
import type { AssistantListItem } from '../../core/types/assistant';

const layoutStore = useLayoutStore();
const overlayStore = useOverlayStore();
const sessionStore = useChatSessionStore();
const topicStore = useTopicStore();

const activeTab = sidebarTab;
const searchQuery = ref('');
const topicSortMode = computed({
  get: () => topicStore.effectiveSortMode,
  set: (mode) => topicStore.setSortMode(mode),
});

// 切换 Tab 时清空搜索框
watch(activeTab, () => {
  searchQuery.value = '';
});

const sidebarRef = ref<HTMLElement | null>(null);

// 侧边栏内部监听左滑以关闭或 Tab 切换
useSidebarSwipe(sidebarRef, {
  type: 'left',
  onTabSwitch: () => {
    if (activeTab.value === 'topics') {
      activeTab.value = 'agents';
    }
  }
});

const handleSelectItem = async (item: AssistantListItem) => {
  activeTab.value = 'topics';
  if (item) {
    // selectItem 负责加载列表并恢复该 Owner 的上次活跃话题。
    try {
      await sessionStore.selectItem(item);
    } catch {
      // Topic Store 已统一显示加载失败提示。
    }
  }
};

const handleSelectTopic = () => {
  // 移动端选择话题后自动关闭侧边栏的逻辑已在 TopicList 中处理
};

const openSettings = () => {
  overlayStore.openSettings();
};

// 打开全局搜索页：抽屉层级（drawer, 20）低于页面栈（page, 40+），必须先收起抽屉
const openGlobalSearch = () => {
  layoutStore.setLeftDrawer(false);
  overlayStore.openGlobalSearch();
};
</script>

<template>
  <aside
    id="agent-sidebar"
    ref="sidebarRef"
    class="vcp-drawer vcp-drawer-left flex flex-col min-w-0 min-h-0 overflow-hidden"
    :class="{ 'is-open': layoutStore.leftDrawerOpen }"
    aria-label="助手与话题侧栏"
  >

    <!-- 顶部 Tabs -->
    <div class="vcp-drawer-header px-4 pb-2 shrink-0 border-b border-black/5 dark:border-white/5">
      <div class="flex items-center justify-between mb-4 px-2">
        <h2 class="text-xl font-black opacity-90 tracking-tighter text-blue-500 dark:text-blue-400">VCP MOBILE
        </h2>
        <button
          type="button"
          class="p-1.5 rounded-lg text-primary-text opacity-60 hover:opacity-100 active:scale-95 transition-all"
          aria-label="全局搜索"
          title="全局搜索（搜索全部消息内容）"
          @click="openGlobalSearch"
        >
          <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
            stroke-linecap="round" stroke-linejoin="round">
            <circle cx="11" cy="11" r="8"></circle>
            <line x1="21" y1="21" x2="16.65" y2="16.65"></line>
          </svg>
        </button>
      </div>

      <SidebarTabs v-model:activeTab="activeTab" />
      <SidebarSearch v-model="searchQuery" v-model:sort-mode="topicSortMode" :activeTab="activeTab" />
    </div>

    <!-- 内容区 -->
    <div class="flex-1 min-h-0 overflow-hidden">
      <template v-if="activeTab === 'agents'">
        <div class="h-full overflow-y-auto px-4 py-4 space-y-2 vcp-scrollable no-rubber-band">
          <AgentList :searchQuery="searchQuery" @select-agent="handleSelectItem" @select-group="handleSelectItem" />
        </div>
      </template>

      <template v-if="activeTab === 'topics'">
        <TopicList :searchQuery="searchQuery" @select-topic="handleSelectTopic" />
      </template>
    </div>

    <!-- 底部: 动作区与设置 -->
    <div
      class="p-4 border-t border-black/5 dark:border-white/5 glass-panel shrink-0 space-y-3 pb-[calc(var(--vcp-safe-bottom,48px)+8px)]">
      <template v-if="activeTab === 'agents'">
        <AgentsCreator />
      </template>
      <template v-if="activeTab === 'topics'">
        <TopicCreator />
      </template>

      <button
        class="w-full flex items-center justify-between p-3 bg-black/5 dark:bg-white/5 hover:bg-black/10 dark:hover:bg-white/10 active:scale-95 rounded-xl transition-all border border-black/5 dark:border-white/5 text-primary-text"
        @click="openSettings">
        <div class="flex items-center gap-3">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
            stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="3"></circle>
            <path
              d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z">
            </path>
          </svg>
          <span class="font-bold text-sm">全局设置</span>
        </div>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
          class="opacity-30">
          <polyline points="9 18 15 12 9 6"></polyline>
        </svg>
      </button>
    </div>

  </aside>
</template>

<style scoped>
.vcp-drawer {
  position: absolute;
  top: 0;
  bottom: 0;
  box-sizing: border-box;
  height: 100%;
  min-height: 0;
  width: 82vw;
  max-width: 340px;
  visibility: hidden;
  pointer-events: none;
  background-color: var(--vcp-panel-bg-97, var(--secondary-bg));
  transition:
    transform 0.4s cubic-bezier(0.16, 1, 0.3, 1),
    visibility 0s linear 0.4s;
  z-index: var(--layer-drawer);
}

.vcp-drawer-left {
  left: 0;
  padding-left: var(--vcp-workspace-safe-left, 0px);
  transform: translateX(-100%);
  border-right: 1px solid transparent;
}

.vcp-drawer-left.is-open {
  visibility: visible;
  pointer-events: auto;
  transform: translateX(0);
  transition-delay: 0s;
}

.vcp-drawer-header {
  padding-top: calc(var(--vcp-safe-top, 24px) + 1.5rem);
}

@media (min-width: 1024px) {
  .vcp-drawer {
    position: relative;
    top: auto;
    bottom: auto;
    left: auto;
    flex: 0 0 280px;
    transform: translateX(0) !important;
    width: 280px;
    max-width: 280px;
    visibility: visible;
    pointer-events: auto;
    z-index: var(--layer-local);
    transition: none;
  }
}

/* 隐藏滚动条 */
.overflow-y-auto {
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.overflow-y-auto::-webkit-scrollbar {
  display: none;
}

@media (hover: none) and (pointer: coarse) {
}
</style>
