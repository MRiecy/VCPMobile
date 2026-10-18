<script setup lang="ts">
import { useId } from 'vue';
import SettingsCard from './SettingsCard.vue';

withDefaults(defineProps<{
  title?: string;
  titleId?: string;
  description?: string;
  accentColor?: string;
  /** 自带卡片或多个独立卡片的内容可关闭默认外壳。 */
  card?: boolean;
}>(), { card: true });

const generatedTitleId = useId();
</script>

<template>
  <!-- 内部间距归区块；组间距归页面 gap，滚动末尾留白归宿主。 -->
  <section class="settings-section flex flex-col gap-2 min-w-0" :aria-labelledby="title ? (titleId || generatedTitleId) : undefined">
    <div v-if="title || description || $slots.action" class="flex items-center gap-2 px-1">
      <div v-if="accentColor" class="w-1 h-4 rounded-full shrink-0" :class="accentColor" aria-hidden="true"></div>
      <div class="flex flex-col min-w-0 flex-1">
        <h3 v-if="title" :id="titleId || generatedTitleId" class="text-[13px] font-bold text-[var(--secondary-text)] break-words">
          {{ title }}
        </h3>
        <p v-if="description" class="text-[11px] text-[var(--secondary-text)] leading-relaxed mt-1 break-words">
          {{ description }}
        </p>
      </div>
      <!-- 区块标题右侧操作位（如帮助问号按钮） -->
      <slot name="action"></slot>
    </div>
    <SettingsCard v-if="card">
      <slot></slot>
    </SettingsCard>
    <slot v-else></slot>
    <div v-if="$slots.footer" class="flex flex-col gap-2 px-1 text-[11px] text-[var(--secondary-text)] leading-relaxed">
      <slot name="footer"></slot>
    </div>
  </section>
</template>
