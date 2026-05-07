<script setup lang="ts">
import { computed } from 'vue';
import { X, Play } from 'lucide-vue-next';
import SlidePage from '../../../components/ui/SlidePage.vue';
import { useRebuildSessionStore } from '../../../core/stores/rebuildSession';
import { useOverlayStore } from '../../../core/stores/overlay';
import { useDataReload } from '../../../core/composables/useDataReload';

const store = useRebuildSessionStore();
const overlayStore = useOverlayStore();
const { performFullReload } = useDataReload();

const progressPercent = computed(() => {
  if (store.progress.total <= 0) return 0;
  return Math.min(100, Math.round((store.progress.current / store.progress.total) * 100));
});

const statusLabel = computed(() => {
  switch (store.status) {
    case 'idle': return '准备就绪';
    case 'running': return '重建中';
    case 'completed': return '已完成';
    case 'error': return '失败';
    default: return '等待';
  }
});

const statusDotClass = computed(() => {
  switch (store.status) {
    case 'idle': return 'bg-gray-400';
    case 'running': return 'bg-blue-400 animate-pulse';
    case 'completed': return 'bg-green-400';
    case 'error': return 'bg-red-400';
    default: return 'bg-gray-400';
  }
});

const progressBarClass = computed(() => {
  switch (store.status) {
    case 'error': return 'bg-red-500';
    case 'completed': return 'bg-green-500';
    default: return 'bg-blue-500';
  }
});

const handleClose = async () => {
  if (store.needsReload) {
    const confirmed = confirm('重建已完成，数据已更新。点击确认立即刷新以生效。');
    if (confirmed) {
      store.markReloaded();
      overlayStore.closeRebuildSession();
      await performFullReload();
      return;
    }
  }
  overlayStore.closeRebuildSession();
};
</script>

<template>
  <SlidePage :is-open="store.isOpen" :z-index="100">
    <div class="fixed inset-0 flex flex-col bg-[#0a0f14] text-white overflow-hidden"
         :class="{ 'pointer-events-none': !store.isOpen }">

      <!-- 顶部栏 -->
      <div class="flex items-center justify-between px-4 pt-[calc(env(safe-area-inset-top)+8px)] pb-3">
        <div class="flex items-center gap-2">
          <div class="w-2 h-2 rounded-full" :class="statusDotClass"></div>
          <span class="text-xs font-bold uppercase tracking-widest">{{ statusLabel }}</span>
        </div>
        <button v-if="store.canDismiss" @click="handleClose()"
          class="p-2 -mr-2 text-gray-400 hover:text-white transition-colors">
          <X :size="20" />
        </button>
      </div>

      <!-- 内容区 -->
      <div class="flex-1 flex flex-col items-center justify-center px-8">

        <!-- idle 状态 -->
        <template v-if="store.status === 'idle'">
          <div class="w-16 h-16 rounded-full bg-white/5 flex items-center justify-center mb-6">
            <Play :size="28" class="text-blue-400 ml-1" />
          </div>
          <div class="text-sm font-bold tracking-wider mb-2">全量预渲染重建</div>
          <div class="text-[11px] text-white/30 text-center mb-8 leading-relaxed">
            对数据库中所有历史消息进行<br>
            AST 重新解析与代码高亮固化
          </div>
          <button
            @click="store.startRebuild()"
            class="px-8 py-3 rounded-lg bg-blue-500/20 text-blue-400 text-xs font-bold tracking-widest uppercase active:bg-blue-500/30 transition-colors"
          >
            开始重建
          </button>
        </template>

        <!-- running / completed / error 状态 -->
        <template v-else>
          <!-- 进度条 -->
          <div class="w-full max-w-xs mb-4">
            <div class="h-1 bg-white/10 rounded-full overflow-hidden">
              <div class="h-full transition-all duration-500 rounded-full"
                   :class="progressBarClass"
                   :style="{ width: progressPercent + '%' }"></div>
            </div>
            <div class="flex justify-between text-[10px] mt-1 opacity-50">
              <span>预渲染重建</span>
              <span v-if="store.progress.total > 0">
                {{ store.progress.current }} / {{ store.progress.total }}
              </span>
            </div>
          </div>

          <!-- 状态文字 -->
          <div class="text-center mt-4">
            <div v-if="store.status === 'running'" class="text-xs text-white/60">
              正在重新解析消息...
            </div>
            <div v-else-if="store.status === 'completed'" class="text-xs text-green-400 font-bold">
              全量预渲染重建完成
            </div>
            <div v-else-if="store.status === 'error'" class="text-xs text-red-400 font-bold max-w-xs break-words">
              重建失败: {{ store.errorMessage }}
            </div>
          </div>

          <!-- completed / error 时的关闭按钮 -->
          <button
            v-if="store.status === 'completed' || store.status === 'error'"
            @click="handleClose()"
            class="mt-8 px-6 py-2 rounded-lg bg-white/10 text-white/80 text-xs font-bold tracking-wider hover:bg-white/20 transition-colors"
          >
            {{ store.status === 'completed' ? '完成' : '关闭' }}
          </button>
        </template>
      </div>

      <!-- 底部状态栏 -->
      <div class="flex items-center justify-center px-4 py-2 border-t border-white/5">
        <div class="text-[9px] opacity-30 font-bold tracking-[0.2em] uppercase">
          <span v-if="store.status === 'idle'">点击上方按钮以开始</span>
          <span v-else-if="store.status === 'running'">重建进行中</span>
          <span v-else-if="store.status === 'completed'">重建已完成</span>
          <span v-else-if="store.status === 'error'">重建失败</span>
        </div>
      </div>

      <!-- 全局遮罩层（running 时激活，阻止误触） -->
      <div v-if="store.status === 'running'"
           class="absolute inset-0 bg-black/20 z-10 flex flex-col justify-end pointer-events-auto"
           style="touch-action: none;">
        <div class="pb-8 text-center">
          <div class="inline-flex items-center gap-2 px-4 py-2 rounded-full bg-black/60 text-white/90 text-xs font-bold tracking-wider">
            <div class="w-1.5 h-1.5 rounded-full bg-blue-400 animate-pulse"></div>
            重建进行中 — 请勿退出
          </div>
        </div>
      </div>
    </div>
  </SlidePage>
</template>
