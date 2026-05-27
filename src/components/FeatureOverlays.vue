<script setup lang="ts">
/**
 * FeatureOverlays.vue
 *
 * 职责：作为所有全局业务 Feature 视图的统一挂载点。
 *
 * 架构说明：
 * 1. Settings/Agent/Group 设置页保持常驻 DOM（isMounted），以保留组件本地状态（表单草稿等）
 *    和确保 SlidePage 的 leave 动画正常完成。
 * 2. SyncSessionView 使用 v-if 按需渲染，因其状态完全由 syncSessionStore 管理，
 *    且已纳入 OverlayStore pageStack 统一管控。
 *
 * 注意：此组件内的视图通过 SlidePage 管理滑入/滑出动画，
 * 物理上它们会渲染在 GlobalOverlayManager 提供的容器中。
 */
import { ref, onMounted, defineAsyncComponent } from 'vue';
import { useOverlayStore } from '../core/stores/overlay';
import SettingsView from '../features/settings/SettingsView.vue';
import AgentSettingsView from '../features/agent/AgentSettingsView.vue';
import GroupSettingsView from '../features/agent/GroupSettingsView.vue';
import TarvenSettingsView from '../features/chat/components/TarvenSettings.vue';
import SensorCollector from '../features/distributed/SensorCollector.vue';
import ToolInteractionOverlay from '../features/distributed/ToolInteractionOverlay.vue';

// SyncSessionView / RebuildSessionView 按需异步加载，状态由 Store 完全托管
const SyncSessionView = defineAsyncComponent(() => import('../features/sync/SyncSessionView.vue'));
const RebuildSessionView = defineAsyncComponent(() => import('../features/settings/components/RebuildSessionView.vue'));

const overlayStore = useOverlayStore();
const isMounted = ref(false);

onMounted(() => {
  isMounted.value = true;
});
</script>

<template>
  <div v-if="isMounted">
    <SettingsView
      :is-open="overlayStore.isSettingsOpen"
      :z-index="overlayStore.getPageZIndex('settings')"
      @close="overlayStore.closeSettings()"
    />

    <AgentSettingsView
      :is-open="overlayStore.isAgentSettingsOpen"
      :id="overlayStore.agentSettingsId"
      :z-index="overlayStore.getPageZIndex('agentSettings')"
      @close="overlayStore.closeAgentSettings()"
    />

    <GroupSettingsView
      :is-open="overlayStore.isGroupSettingsOpen"
      :id="overlayStore.groupSettingsId"
      :z-index="overlayStore.getPageZIndex('groupSettings')"
      @close="overlayStore.closeGroupSettings()"
    />

    <TarvenSettingsView
      :is-open="overlayStore.isTarvenSettingsOpen"
      :z-index="overlayStore.getPageZIndex('tarvenSettings')"
      @close="overlayStore.closeTarvenSettings()"
    />

    <SyncSessionView
      v-if="overlayStore.isSyncSessionOpen"
      :z-index="overlayStore.getPageZIndex('syncSession')"
    />
    <RebuildSessionView
      v-if="overlayStore.isRebuildSessionOpen"
      :z-index="overlayStore.getPageZIndex('rebuildSession')"
    />

    <SensorCollector />
    <ToolInteractionOverlay />
  </div>
</template>