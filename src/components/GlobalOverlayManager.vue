<script setup lang="ts">
import { useOverlayStore } from '../core/stores/overlay';
import VcpPrompt from './ui/VcpPrompt.vue';
import ToastManager from './ui/ToastManager.vue';
import ContextMenuSheet from './ui/ContextMenuSheet.vue';
import FullScreenEditor from './ui/FullScreenEditor.vue';

const overlayStore = useOverlayStore();

const handlePromptConfirm = (val: string) => {
  if (overlayStore.promptConfig?.onConfirm) {
    overlayStore.promptConfig.onConfirm(val);
  }
  overlayStore.closePrompt();
};

const handleEditorSave = (newContent: string) => {
  if (overlayStore.editorConfig?.onSave) {
    overlayStore.editorConfig.onSave(newContent);
  }
  overlayStore.closeEditor();
};
</script>

<template>
  <div class="fixed inset-0 pointer-events-none z-[60]">
    <!-- 1. 全局基础 UI (Prompt/Toast) -->
    <VcpPrompt v-if="overlayStore.promptConfig" class="pointer-events-auto" :is-open="!!overlayStore.promptConfig"
      :title="overlayStore.promptConfig.title" :initial-value="overlayStore.promptConfig.initialValue"
      :placeholder="overlayStore.promptConfig.placeholder" @confirm="handlePromptConfirm"
      @cancel="overlayStore.closePrompt()" @update:isOpen="!$event && overlayStore.closePrompt()" />

    <!-- 全局 Context Menu -->
    <ContextMenuSheet v-if="overlayStore.contextMenuConfig" :is-open="!!overlayStore.contextMenuConfig"
      :title="overlayStore.contextMenuConfig.title" :actions="overlayStore.contextMenuConfig.actions"
      @close="overlayStore.closeContextMenu()" @action-click="overlayStore.closeContextMenu()" />

    <!-- 全局 FullScreenEditor -->
    <FullScreenEditor v-if="overlayStore.editorConfig" class="pointer-events-auto"
      :is-open="!!overlayStore.editorConfig" :initial-value="overlayStore.editorConfig.initialValue"
      @save="handleEditorSave" @cancel="overlayStore.closeEditor()"
      @update:isOpen="!$event && overlayStore.closeEditor()" />

    <ToastManager class="pointer-events-auto" />

    <!-- 2. 业务 Feature 投射目标 -->
    <!-- 各 Feature 组件通过 <Teleport to="#vcp-feature-overlays"> 投射到此处 -->
    <div id="vcp-feature-overlays" class="absolute inset-0 pointer-events-none"></div>
  </div>
</template>

<style scoped></style>
