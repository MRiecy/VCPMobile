<script setup lang="ts">
import { ref } from 'vue';
import { useSwipe } from '@vueuse/core';
import { useLayoutStore } from '../../core/stores/layout';
import { useOverlayStore } from '../../core/stores/overlay';
import { useChatSessionStore } from '../../core/stores/chatSessionStore';
import SidebarTabs from '../../features/agent/SidebarTabs.vue';
import SidebarSearch from '../../features/agent/SidebarSearch.vue';
import AgentList from '../../features/agent/AgentList.vue';
import TopicList from '../../features/topic/TopicList.vue';
import AgentsCreator from '../../features/agent/AgentsCreator.vue';
import TopicCreator from '../../features/topic/TopicCreator.vue';

const layoutStore = useLayoutStore();
const overlayStore = useOverlayStore();
const sessionStore = useChatSessionStore();

const activeTab = ref<'agents' | 'topics'>('agents');
const searchQuery = ref('');

const sidebarRef = ref<HTMLElement | null>(null);

// 侧边栏内部监听左滑以关闭
const { direction, lengthX, lengthY } = useSwipe(sidebarRef, {
  threshold: 15,
  onSwipeEnd: (e: TouchEvent | MouseEvent) => {
    // 排除特定不响应滑动的区域
    if (e.target instanceof Element && e.target.closest(".no-swipe")) return;

    const absX = Math.abs(lengthX.value);
    const absY = Math.abs(lengthY.value);

    // 如果是向左滑，且位移足够大，且角度符合水平滑动特征
    if (direction.value === 'left' && absX > 50) {
      if (absY / absX < 0.577) {
        layoutStore.setLeftDrawer(false);
      }
    }
  }
});

const handleSelectItem = async (item: any) => {
  activeTab.value = 'topics';
  if (item) {
    // 自动加载并渲染上次活跃话题（保留便利性）
    // 话题列表的加载由 TopicList.vue 中的 watch 响应式驱动，此处无需重复调用
    await sessionStore.selectItem(item);
  }
};

const handleSelectTopic = () => {
  // 移动端选择话题后自动关闭侧边栏的逻辑已在 TopicList 中处理
};

const openSettings = () => {
  overlayStore.openSettings();
};
</script>

<template>
  <aside ref="sidebarRef" class="vcp-drawer vcp-drawer-left flex flex-col" :class="{ 'is-open': layoutStore.leftDrawerOpen }">

    <!-- 顶部 Tabs -->
    <div class="pt-safe px-4 pt-6 pb-2 shrink-0 border-b border-black/5 dark:border-white/5">
      <h2 class="text-xl font-black opacity-90 mb-4 tracking-tighter text-blue-500 dark:text-blue-400 px-2">VCP MOBILE
      </h2>

      <SidebarTabs v-model:activeTab="activeTab" />
      <SidebarSearch v-model="searchQuery" :activeTab="activeTab" />
    </div>

    <!-- 内容区 -->
    <div class="flex-1 overflow-hidden">
      <template v-if="activeTab === 'agents'">
        <div class="h-full overflow-y-auto px-4 py-4 space-y-2 vcp-scrollable">
          <AgentList :searchQuery="searchQuery" @select-agent="handleSelectItem" @select-group="handleSelectItem" />
        </div>
      </template>

      <template v-if="activeTab === 'topics'">
        <TopicList @select-topic="handleSelectTopic" />
      </template>
    </div>

    <!-- 底部: 动作区与设置 -->
    <div
      class="p-4 border-t border-black/5 dark:border-white/5 glass-panel shrink-0 space-y-3 pb-[calc(var(--vcp-safe-bottom,16px)+8px)]">
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
              d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z">
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
  width: 82vw;
  max-width: 340px;
  background-color: color-mix(in srgb, var(--secondary-bg) 85%, transparent);
  backdrop-filter: blur(18px) saturate(165%);
  -webkit-backdrop-filter: blur(18px) saturate(165%);
  transition: transform 0.4s cubic-bezier(0.16, 1, 0.3, 1);
}

.vcp-drawer-left {
  left: 0;
  transform: translateX(-100%);
  border-right: 1px solid transparent;
}

.vcp-drawer-left.is-open {
  transform: translateX(0);
}

@media (min-width: 768px) {
  .vcp-drawer {
    position: relative;
    transform: translateX(0) !important;
    width: 280px;
    max-width: 280px;
    z-index: 10;
  }

  .vcp-drawer-left {
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

/* 确保子组件中的 glass-panel 样式生效，因为 scoped 限制，需要使用 :deep() 或者将样式移到全局/子组件中 */
:deep(.glass-panel) {
  background: rgba(255, 255, 255, 0.03);
  backdrop-filter: blur(10px);
  -webkit-backdrop-filter: blur(10px);
}

.dark :deep(.glass-panel) {
  background: rgba(0, 0, 0, 0.2);
}

@media (hover: none) and (pointer: coarse) {
  .vcp-drawer {
    backdrop-filter: blur(4px) saturate(165%);
    -webkit-backdrop-filter: blur(4px) saturate(165%);
  }
  :deep(.glass-panel) {
    backdrop-filter: blur(4px);
    -webkit-backdrop-filter: blur(4px);
  }
}
</style>
