<script setup lang="ts">
import { useOverlayStore } from '../core/stores/overlay';
import { showExitToast } from '../core/composables/useModalHistory';
import VcpPrompt from './ui/VcpPrompt.vue';
import ToastManager from './ui/ToastManager.vue';
import SettingsView from '../features/settings/SettingsView.vue';
import SyncView from '../features/settings/SyncView.vue';

const overlayStore = useOverlayStore();

const handleContextMenuBackdropClick = () => {
  showExitToast.value = true;
};

const handlePromptConfirm = (val: string) => {
  if (overlayStore.promptConfig?.onConfirm) {
    overlayStore.promptConfig.onConfirm(val);
  }
  overlayStore.closePrompt();
};
</script>

<template>
  <div class="fixed inset-0 pointer-events-none z-[120]">
    <!-- 全局 Prompt -->
    <VcpPrompt
      v-if="overlayStore.promptConfig"
      class="pointer-events-auto"
      :is-open="!!overlayStore.promptConfig"
      :title="overlayStore.promptConfig.title"
      :initial-value="overlayStore.promptConfig.initialValue"
      :placeholder="overlayStore.promptConfig.placeholder"
      @confirm="handlePromptConfirm"
      @cancel="overlayStore.closePrompt()"
      @update:isOpen="!$event && overlayStore.closePrompt()"
    />

    <!-- 全局 Context Menu -->
    <Transition name="fade">
      <div v-if="overlayStore.contextMenuConfig" class="fixed inset-0 z-[200] bg-black/20 backdrop-blur-[1px] pointer-events-auto" @click="handleContextMenuBackdropClick">
        <div class="absolute left-1/2 bottom-6 -translate-x-1/2 w-[calc(100%-24px)] max-w-sm rounded-3xl border border-black/5 dark:border-white/10 bg-white/92 dark:bg-[#111827]/92 backdrop-blur-xl shadow-2xl overflow-hidden"
             @click.stop>
          <div class="px-5 pt-5 pb-3 border-b border-black/5 dark:border-white/10">
            <h3 class="text-sm font-black tracking-wide">{{ overlayStore.contextMenuConfig.title }}</h3>
          </div>
          <div class="p-2">
            <button v-for="action in overlayStore.contextMenuConfig.actions" :key="action.label"
                    @click="action.handler(); overlayStore.closeContextMenu()"
                    class="w-full flex items-center gap-3 px-4 py-3 rounded-2xl text-left transition-all"
                    :class="action.danger ? 'text-red-500 hover:bg-red-500/10' : 'hover:bg-black/5 dark:hover:bg-white/5'">
              <component :is="action.icon" class="w-4 h-4 shrink-0" />
              <span class="text-sm font-semibold">{{ action.label }}</span>
            </button>
          </div>
        </div>
      </div>
    </Transition>

    <!-- Toast -->
    <ToastManager class="pointer-events-auto" />

    <!-- Settings & Sync -->
    <SettingsView :is-open="overlayStore.isSettingsOpen" @close="overlayStore.closeSettings()" @open-sync="overlayStore.openSync()" />
    <SyncView :is-open="overlayStore.isSyncOpen" @close="overlayStore.closeSync()" />
  </div>
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
