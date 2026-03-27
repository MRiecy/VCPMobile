<script setup lang="ts">
import { computed } from 'vue';
import { useUIStore } from '../stores/ui';
import { useNotificationStore } from '../stores/notification';
import { showExitToast } from '../composables/useModalHistory';
import SettingsView from '../views/SettingsView.vue';
import SyncView from '../views/SyncView.vue';
import BottomSheet, { type ActionItem } from './BottomSheet.vue';
import VcpPrompt from './VcpPrompt.vue';
import NotificationDrawer from './NotificationDrawer.vue';
import ToastManager from './ToastManager.vue';

const uiStore = useUIStore();
const notificationStore = useNotificationStore();

const settingsActions = computed<ActionItem[]>(() => [
  {
    label: '关闭',
    handler: () => {
      uiStore.closeModal();
    }
  }
]);

const syncActions = computed<ActionItem[]>(() => [
  {
    label: '关闭',
    handler: () => {
      uiStore.closeModal();
    }
  }
]);

const handleContextMenuBackdropClick = () => {
  showExitToast.value = true;
};

const handleRightDrawerClose = () => {
  uiStore.setRightDrawer(false);
};

const handlePromptConfirm = (val: string) => {
  if (uiStore.promptConfig?.onConfirm) {
    uiStore.promptConfig.onConfirm(val);
  }
  uiStore.closePrompt();
};
</script>

<template>
  <!-- 1. 全局遮罩 (z-index 提高) -->
  <Transition name="fade">
    <div v-if="uiStore.leftDrawerOpen || uiStore.rightDrawerOpen" 
         class="vcp-overlay fixed inset-0 bg-black/30 z-[60] backdrop-blur-[1px] md:hidden"
         @click="uiStore.leftDrawerOpen = false; uiStore.rightDrawerOpen = false">
    </div>
  </Transition>

  <!-- 2. 右侧通知抽屉 -->
  <NotificationDrawer :is-open="uiStore.rightDrawerOpen" @close="handleRightDrawerClose" />

  <!-- 3. 底部弹层 -->
  <BottomSheet 
    :model-value="uiStore.activeModal === 'settings'" 
    :actions="settingsActions" 
    title="设置" 
    @update:modelValue="uiStore.closeModal()"
  >
    <SettingsView @close="uiStore.closeModal()" @open-sync="uiStore.openModal('sync')" />
  </BottomSheet>

  <BottomSheet 
    :model-value="uiStore.activeModal === 'sync'" 
    :actions="syncActions" 
    title="同步" 
    @update:modelValue="uiStore.closeModal()"
  >
    <SyncView />
  </BottomSheet>

  <!-- 4. 全局 Prompt -->
  <VcpPrompt
    v-if="uiStore.promptConfig"
    :is-open="!!uiStore.promptConfig"
    :title="uiStore.promptConfig.title"
    :initial-value="uiStore.promptConfig.initialValue"
    :placeholder="uiStore.promptConfig.placeholder"
    @confirm="handlePromptConfirm"
    @cancel="uiStore.closePrompt()"
    @update:isOpen="!$event && uiStore.closePrompt()"
  />

  <!-- 5. 全局 Context Menu -->
  <Transition name="fade">
    <div v-if="uiStore.contextMenuConfig" class="fixed inset-0 z-[200] bg-black/20 backdrop-blur-[1px]" @click="handleContextMenuBackdropClick">
      <div class="absolute left-1/2 bottom-6 -translate-x-1/2 w-[calc(100%-24px)] max-w-sm rounded-3xl border border-black/5 dark:border-white/10 bg-white/92 dark:bg-[#111827]/92 backdrop-blur-xl shadow-2xl overflow-hidden"
           @click.stop>
        <div class="px-5 pt-5 pb-3 border-b border-black/5 dark:border-white/10">
          <h3 class="text-sm font-black tracking-wide">{{ uiStore.contextMenuConfig.title }}</h3>
        </div>
        <div class="p-2">
          <button v-for="action in uiStore.contextMenuConfig.actions" :key="action.label"
                  @click="action.handler(); uiStore.closeContextMenu()"
                  class="w-full flex items-center gap-3 px-4 py-3 rounded-2xl text-left transition-all"
                  :class="action.danger ? 'text-red-500 hover:bg-red-500/10' : 'hover:bg-black/5 dark:hover:bg-white/5'">
            <component :is="action.icon" class="w-4 h-4 shrink-0" />
            <span class="text-sm font-semibold">{{ action.label }}</span>
          </button>
        </div>
      </div>
    </div>
  </Transition>

  <!-- 6. Toast -->
  <ToastManager />
</template>

<style scoped>
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.25s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>
