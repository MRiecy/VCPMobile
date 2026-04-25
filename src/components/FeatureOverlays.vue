<script setup lang="ts">
/**
 * FeatureOverlays.vue
 * 
 * 职责：作为所有全局业务 Feature 视图的统一挂载点。
 * 
 * 为什么需要这个组件？
 * 1. 逻辑挂载：Vue 组件必须被实例化，其内部的 Teleport 逻辑才会运行。
 * 2. 架构解耦：App.vue 只需引用此组件，无需直接管理众多的业务视图。
 * 3. 状态持久：挂载在应用根部，确保 Feature 状态在 UI 切换时不会丢失。
 * 
 * 注意：此组件内的视图通常都包含 <Teleport to="#vcp-feature-overlays">，
 * 物理上它们会渲染在 GlobalOverlayManager 提供的容器中。
 */
import { ref, onMounted } from 'vue';
import { useOverlayStore } from '../core/stores/overlay';
import SettingsView from '../features/settings/SettingsView.vue';
import AgentSettingsView from '../features/agent/AgentSettingsView.vue';
import GroupSettingsView from '../features/agent/GroupSettingsView.vue';

const overlayStore = useOverlayStore();
const isMounted = ref(false);

onMounted(() => {
  isMounted.value = true;
});
</script>

<template>
  <!-- 这里的组件虽然声明在此，但会通过 Teleport 渲染到 GlobalOverlayManager 中 -->
  <div v-if="isMounted">
    <SettingsView :is-open="overlayStore.isSettingsOpen" @close="overlayStore.closeSettings()" />

    <AgentSettingsView :is-open="overlayStore.isAgentSettingsOpen" :id="overlayStore.agentSettingsId"
      @close="overlayStore.closeAgentSettings()" />

    <GroupSettingsView :is-open="overlayStore.isGroupSettingsOpen" :id="overlayStore.groupSettingsId"
      @close="overlayStore.closeGroupSettings()" />
  </div>
</template>
