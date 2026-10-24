<script setup lang="ts">
import { inject, useId } from 'vue';
import SettingsCard from './SettingsCard.vue';

withDefaults(defineProps<{
  title?: string;
  titleId?: string;
  description?: string;
  index?: string;
  divider?: boolean;
  accentColor?: string;
  /** 自带卡片或多个独立卡片的内容可关闭默认外壳。 */
  card?: boolean;
}>(), { card: undefined, divider: undefined });

const generatedTitleId = useId();
const canvas = inject<boolean>('vcp-settings-canvas', false);
</script>

<template>
  <!-- 内部间距归区块；组间距归页面 gap，滚动末尾留白归宿主。 -->
  <section
    class="settings-section flex flex-col gap-2 min-w-0"
    :class="{
      'settings-section--ruled': canvas && (divider ?? Boolean(title || description || $slots.description || $slots.action)),
    }"
    :aria-labelledby="title ? (titleId || generatedTitleId) : undefined"
  >
    <div v-if="title || description || $slots.description || $slots.action" class="settings-section-header flex items-center gap-2 px-1">
      <div v-if="accentColor && !canvas" class="w-1 h-4 rounded-full shrink-0" :class="accentColor" aria-hidden="true"></div>
      <div class="flex flex-col min-w-0 flex-1">
        <div v-if="title || index" class="settings-section-heading flex items-baseline gap-2 min-w-0">
          <span v-if="index" class="settings-section-index font-mono shrink-0" aria-hidden="true">{{ index }}</span>
          <h3 v-if="title" :id="titleId || generatedTitleId" class="settings-section-title text-[13px] font-bold text-[var(--secondary-text)] break-words">
            {{ title }}
          </h3>
        </div>
        <p v-if="description || $slots.description" class="settings-section-description text-[11px] text-[var(--secondary-text)] leading-relaxed mt-1 break-words">
          <slot name="description">{{ description }}</slot>
        </p>
      </div>
      <!-- 区块标题右侧操作位（如帮助问号按钮） -->
      <slot name="action"></slot>
    </div>
    <SettingsCard v-if="card ?? !canvas">
      <slot></slot>
    </SettingsCard>
    <slot v-else></slot>
    <div v-if="$slots.footer" class="settings-section-footer flex flex-col gap-2 px-1 text-[11px] text-[var(--secondary-text)] leading-relaxed">
      <slot name="footer"></slot>
    </div>
  </section>
</template>
