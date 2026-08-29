<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { onClickOutside } from '@vueuse/core';
import { ArrowUpDown, CalendarDays, Check, RefreshCw } from 'lucide-vue-next';
import type { TopicSortMode } from '../../core/stores/topicListManager';

const props = defineProps<{
  modelValue: string;
  activeTab: 'agents' | 'topics';
  sortMode: TopicSortMode;
}>();

const emit = defineEmits<{
  (e: 'update:modelValue', value: string): void;
  (e: 'update:sortMode', value: TopicSortMode): void;
}>();

const rootRef = ref<HTMLElement | null>(null);
const sortMenuOpen = ref(false);

const placeholderText = computed(() => {
  return props.activeTab === 'agents' ? '搜索助手...' : '搜索话题...';
});

const selectSortMode = (mode: TopicSortMode) => {
  emit('update:sortMode', mode);
  sortMenuOpen.value = false;
};

onClickOutside(rootRef, () => {
  sortMenuOpen.value = false;
});

watch(
  () => props.activeTab,
  () => {
    sortMenuOpen.value = false;
  },
);
</script>

<template>
  <div ref="rootRef" class="relative group" @keydown.esc="sortMenuOpen = false">
    <svg class="absolute left-3 top-1/2 -translate-y-1/2 opacity-50 w-4 h-4 text-primary-text transition-opacity group-focus-within:opacity-100" viewBox="0 0 24 24"
      fill="none" stroke="currentColor" stroke-width="2">
      <circle cx="11" cy="11" r="8"></circle>
      <line x1="21" y1="21" x2="16.65" y2="16.65"></line>
    </svg>
    <input :value="modelValue" @input="emit('update:modelValue', ($event.target as HTMLInputElement).value)" type="text"
      :placeholder="placeholderText"
      class="w-full bg-black/5 dark:bg-white/5 text-primary-text placeholder:text-secondary-text/50 text-sm rounded-xl py-2.5 pl-10 outline-none border border-black/5 dark:border-white/10 focus:border-blue-500/50 dark:focus:border-blue-400/50 focus:bg-white/10 dark:focus:bg-white/10 transition-all shadow-inner"
      :class="activeTab === 'topics' ? (modelValue ? 'pr-18' : 'pr-11') : 'pr-9'" />
    <button v-if="modelValue" @click="emit('update:modelValue', '')" @mousedown.prevent
      class="absolute top-1/2 -translate-y-1/2 text-secondary-text hover:text-primary-text opacity-60 hover:opacity-100 transition-all p-1 flex items-center justify-center rounded-full active:scale-95"
      :class="activeTab === 'topics' ? 'right-10' : 'right-3'" aria-label="清空搜索">
      <svg class="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
        <line x1="18" y1="6" x2="6" y2="18"></line>
        <line x1="6" y1="6" x2="18" y2="18"></line>
      </svg>
    </button>

    <button
      v-if="activeTab === 'topics'"
      type="button"
      class="absolute right-1 top-1/2 -translate-y-1/2 w-8 h-8 flex items-center justify-center rounded-lg text-secondary-text hover:text-primary-text hover:bg-black/5 dark:hover:bg-white/5 transition-colors"
      aria-label="选择话题排序方式"
      aria-haspopup="menu"
      :aria-expanded="sortMenuOpen"
      @click="sortMenuOpen = !sortMenuOpen"
    >
      <ArrowUpDown :size="15" />
    </button>

    <Transition name="topic-sort-menu">
      <div
        v-if="activeTab === 'topics' && sortMenuOpen"
        class="topic-sort-menu-surface absolute right-0 top-[calc(100%+6px)] z-20 w-40 p-1.5 rounded-xl border border-black/10 dark:border-white/10"
        style="background-color: var(--secondary-bg)"
        role="menu"
        aria-label="话题排序方式"
      >
        <button
          type="button"
          class="w-full min-h-10 px-2.5 flex items-center gap-2 rounded-lg text-xs font-bold text-left text-primary-text transition-colors"
          :class="sortMode === 'created' ? 'text-[var(--highlight-text)] bg-[var(--accent-bg)]' : 'hover:bg-black/5 dark:hover:bg-white/5'"
          role="menuitemradio"
          :aria-checked="sortMode === 'created'"
          @click="selectSortMode('created')"
        >
          <CalendarDays :size="14" />
          <span>创建时间</span>
          <Check v-if="sortMode === 'created'" :size="14" class="ml-auto" aria-hidden="true" />
        </button>
        <button
          type="button"
          class="w-full min-h-10 px-2.5 flex items-center gap-2 rounded-lg text-xs font-bold text-left text-primary-text transition-colors"
          :class="sortMode === 'updated' ? 'text-[var(--highlight-text)] bg-[var(--accent-bg)]' : 'hover:bg-black/5 dark:hover:bg-white/5'"
          role="menuitemradio"
          :aria-checked="sortMode === 'updated'"
          @click="selectSortMode('updated')"
        >
          <RefreshCw :size="14" />
          <span>更新时间</span>
          <Check v-if="sortMode === 'updated'" :size="14" class="ml-auto" aria-hidden="true" />
        </button>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.topic-sort-menu-surface {
  box-shadow: 0 10px 24px rgb(0 0 0 / 24%);
}

.topic-sort-menu-enter-active,
.topic-sort-menu-leave-active {
  transition: opacity 0.14s ease, transform 0.14s ease;
}

.topic-sort-menu-enter-from,
.topic-sort-menu-leave-to {
  opacity: 0;
  transform: translateY(-0.25rem);
}
</style>
