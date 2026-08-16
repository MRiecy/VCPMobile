<script setup lang="ts">
/**
 * GuideCenterSection — 设置内「帮助与指引」子页
 *
 * 列出全部注册指引：标题 / 描述 / 完成状态 / 重播按钮；
 * 底部提供「重置全部指引进度」入口，供反复测试与重温。
 * 视觉沿用 BatteryOptimizationGuide 的线性列表风格。
 */
import { computed } from 'vue';
import { CircleCheck, Circle, Play, RotateCcw } from 'lucide-vue-next';
import { allGuides } from '../registry';
import { useGuideStore } from '../stores/guideStore';
import { useOverlayStore } from '../../../core/stores/overlay';
import { useNotificationStore } from '../../../core/stores/notification';

const guideStore = useGuideStore();

const entries = computed(() =>
  allGuides().map((def) => ({
    def,
    done: guideStore.isCompleted(def.id),
    playing: guideStore.isPlaying(def.id),
  })),
);

const replay = (id: string) => {
  guideStore.replay(id);
};

const resetAll = async () => {
  const confirmed = await useOverlayStore().showConfirm({
    title: '重置指引进度',
    message: '所有教学指引将恢复为未完成状态，可重新体验。',
    isDanger: true,
  });
  if (!confirmed) return;
  guideStore.resetProgress();
  useNotificationStore().addNotification({
    id: 'guide-reset-done',
    toastOnly: true,
    title: '已重置全部指引进度',
    message: '返回对应界面即可重新体验教学。',
    type: 'success',
    duration: 3000,
  });
};
</script>

<template>
  <div class="space-y-6">
    <!-- 顶部说明 -->
    <div class="flex gap-3 p-3.5 rounded-xl bg-blue-500/10 border border-blue-500/20 text-blue-600 dark:text-blue-400">
      <Play class="w-5 h-5 shrink-0 mt-0.5" />
      <div class="space-y-1">
        <h3 class="text-xs font-bold uppercase tracking-wider">交互教学指引</h3>
        <p class="text-[11px] leading-relaxed opacity-80">
          每场指引在满足条件时自动播放一次；可随时在这里重播。教学期间请跟随动画与说明操作。
        </p>
      </div>
    </div>

    <!-- 指引列表 -->
    <div class="space-y-2">
      <div
        v-for="entry in entries"
        :key="entry.def.id"
        class="guide-entry flex items-center gap-3 p-3 rounded-xl border"
        :class="entry.done
          ? 'border-black/5 dark:border-white/5 bg-black/5 dark:bg-white/5'
          : 'border-black/10 dark:border-white/10'"
      >
        <span class="shrink-0 flex items-center justify-center w-7 h-7">
          <CircleCheck v-if="entry.done" class="w-5 h-5 text-emerald-500" />
          <Circle v-else class="w-5 h-5 opacity-30" />
        </span>
        <div class="flex flex-col flex-1 min-w-0">
          <div class="flex items-baseline gap-2">
            <span class="text-xs font-bold truncate text-[var(--primary-text)]">
              {{ entry.def.title }}
            </span>
            <span class="text-[9px] font-mono opacity-50 uppercase shrink-0">
              {{ entry.done ? '已完成' : '未完成' }}
            </span>
          </div>
          <span class="text-[10px] text-[var(--secondary-text)] opacity-70 truncate">
            {{ entry.def.description }}
          </span>
        </div>
        <button
          :disabled="entry.playing"
          class="shrink-0 flex items-center gap-1 px-3 py-1.5 rounded-lg text-[11px] font-semibold border transition-opacity active:opacity-70 disabled:opacity-30"
          :class="entry.playing
            ? 'border-transparent text-[var(--secondary-text)]'
            : 'border-[var(--border-color)] text-[var(--primary-text)]'"
          @click="replay(entry.def.id)"
        >
          <Play class="w-3 h-3" />
          {{ entry.playing ? '播放中' : '重播' }}
        </button>
      </div>
    </div>

    <!-- 重置入口：恢复首次状态，供反复测试与重温 -->
    <button
      class="guide-reset w-full flex items-center justify-between p-3 rounded-xl border border-[var(--border-color)] hover:bg-black/5 dark:hover:bg-white/5 active:opacity-70 transition-opacity"
      @click="resetAll"
    >
      <span class="flex items-center gap-2 text-[11px] font-semibold text-[var(--primary-text)]">
        <RotateCcw class="w-3.5 h-3.5 opacity-70" />
        重置全部指引进度
      </span>
      <span class="text-[9px] opacity-50 font-mono uppercase tracking-wider">RESET</span>
    </button>
  </div>
</template>
