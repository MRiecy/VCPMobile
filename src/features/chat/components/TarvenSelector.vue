<script setup lang="ts">
import { onMounted, watch } from 'vue';
import { useTarvenStore } from '../../../core/stores/tarvenStore';
import { useOverlayStore } from '../../../core/stores/overlay';
import { useModalHistory } from '../../../core/composables/useModalHistory';

const tarvenStore = useTarvenStore();
const overlayStore = useOverlayStore();
const { registerModal, unregisterModal } = useModalHistory();

const modalId = 'TarvenSelector';

watch(() => tarvenStore.isSelectorOpen, (newVal) => {
  if (newVal) {
    tarvenStore.fetchRules();
    registerModal(modalId, () => {
      tarvenStore.isSelectorOpen = false;
    });
  } else {
    unregisterModal(modalId);
  }
});

const close = () => {
  tarvenStore.isSelectorOpen = false;
};

const goToSettings = () => {
  close();
  // 延迟一丢丢，保证收起动画和滑入页面栈过渡顺滑
  setTimeout(() => {
    overlayStore.openTarvenSettings();
  }, 200);
};

const toggleRuleState = (id: string) => {
  tarvenStore.toggleRule(id);
};

onMounted(() => {
  if (tarvenStore.isSelectorOpen) {
    tarvenStore.fetchRules();
  }
});
</script>

<template>
  <Teleport to="body">
    <!-- 遮罩 -->
    <Transition name="fade">
      <div v-if="tarvenStore.isSelectorOpen" class="fixed inset-0 bg-black/40 z-sheet" @click="close"
        @touchmove.prevent>
      </div>
    </Transition>

    <!-- 抽屉体 (高颜值磨砂玻璃) -->
    <Transition name="slide-up">
      <div v-if="tarvenStore.isSelectorOpen"
        class="fixed bottom-0 left-0 right-0 z-sheet bg-white/95 dark:bg-zinc-900/95 backdrop-blur-xl rounded-t-[1.8rem] shadow-2xl p-5 flex flex-col border-t border-white/20 dark:border-white/5"
        style="padding-bottom: calc(env(safe-area-inset-bottom, 20px) + 12px);">
        
        <!-- 拖手线 -->
        <div class="w-10 h-1 bg-black/10 dark:bg-white/15 rounded-full mx-auto mb-4"></div>

        <!-- 头部导航 -->
        <div class="flex items-center justify-between mb-4 px-1">
          <div class="flex flex-col">
            <span class="text-[10px] font-bold text-zinc-400 uppercase tracking-widest leading-none">Context System</span>
            <span class="text-[17px] font-extrabold text-zinc-800 dark:text-zinc-100 mt-1">VCPChatTarven 规则仓</span>
          </div>
          <!-- 齿轮配置入口 -->
          <button @click="goToSettings"
            class="w-9 h-9 flex items-center justify-center rounded-full bg-black/5 dark:bg-white/5 text-zinc-600 dark:text-zinc-400 active:scale-90 transition-transform">
            <div class="i-heroicons-cog-6-tooth text-xl"></div>
          </button>
        </div>

        <!-- 内容区域 -->
        <div class="flex flex-col max-h-[300px] overflow-y-auto px-1 gap-2 scrollbar-none">
          <template v-if="tarvenStore.rules.length > 0">
            <div v-for="rule in tarvenStore.rules" :key="rule.id"
              class="flex items-center justify-between p-3.5 rounded-2xl bg-zinc-50 dark:bg-zinc-800/40 border border-zinc-100 dark:border-zinc-800 transition-all select-none"
              :class="{ 'opacity-90 border-blue-500/30 dark:border-blue-500/20': rule.isEnabled }"
              @click="toggleRuleState(rule.id)"
            >
              <div class="flex items-center gap-3 flex-1 min-w-0 pr-4">
                <!-- 预留图标，降级为简洁规则板 -->
                <div class="w-10 h-10 flex items-center justify-center rounded-xl bg-blue-500/10 text-blue-500 shrink-0">
                  <div class="i-heroicons-sparkles text-lg"></div>
                </div>
                <div class="flex flex-col min-w-0">
                  <span class="text-[14px] font-black text-zinc-800 dark:text-zinc-100 truncate">{{ rule.name }}</span>
                  <span class="text-[11px] text-zinc-400 dark:text-zinc-500 truncate mt-0.5">{{ rule.content }}</span>
                </div>
              </div>

              <!-- iOS 经典优雅 Switch -->
              <div class="relative shrink-0 w-[42px] h-[24px] bg-zinc-200 dark:bg-zinc-700 rounded-full transition-colors duration-200"
                :class="{ 'bg-blue-500 dark:bg-blue-600': rule.isEnabled }">
                <div class="absolute top-[2px] left-[2px] w-[20px] h-[20px] bg-white rounded-full shadow-sm transition-transform duration-200"
                  :class="{ 'translate-x-[18px]': rule.isEnabled }">
                </div>
              </div>
            </div>
          </template>

          <template v-else>
            <!-- 缺省状态 -->
            <div class="flex flex-col items-center justify-center py-10 px-4 text-center">
              <div class="w-14 h-14 flex items-center justify-center rounded-full bg-zinc-50 dark:bg-zinc-800/50 text-zinc-400 dark:text-zinc-600 mb-3 border border-zinc-100 dark:border-zinc-800">
                <div class="i-heroicons-sparkles text-2xl"></div>
              </div>
              <span class="text-[14px] font-extrabold text-zinc-700 dark:text-zinc-300">尚未配置任何规则</span>
              <span class="text-[11px] text-zinc-400 dark:text-zinc-500 mt-1 max-w-[200px]">长按扩展菜单，在此处能够开启你的自定义系统设定</span>
              <button @click="goToSettings"
                class="mt-4 px-5 py-2.5 rounded-full bg-blue-500 text-white text-[12px] font-black shadow-sm shadow-blue-500/10 active:scale-95 transition-transform">
                立即添加规则
              </button>
            </div>
          </template>
        </div>

        <!-- 底置关闭按钮 -->
        <button @click="close"
          class="mt-4 py-3 rounded-2xl text-[14px] font-extrabold bg-zinc-100 dark:bg-zinc-800 text-zinc-500 dark:text-zinc-400 active:scale-[0.98] transition-transform flex items-center justify-center">
          完成
        </button>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.25s cubic-bezier(0.16, 1, 0.3, 1);
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}

.slide-up-enter-active,
.slide-up-leave-active {
  transition: transform 0.35s cubic-bezier(0.16, 1, 0.3, 1);
}
.slide-up-enter-from,
.slide-up-leave-to {
  transform: translateY(100%);
}

.scrollbar-none::-webkit-scrollbar {
  display: none;
}
.scrollbar-none {
  scrollbar-width: none;
  -ms-overflow-style: none;
}
</style>
