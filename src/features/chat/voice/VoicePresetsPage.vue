<script setup lang="ts">
import { ListChecks } from 'lucide-vue-next';
import type { SettingsPageProps } from '../../settings/contracts';

defineProps<SettingsPageProps>();
</script>

<template>
  <section class="voice-presets-page flex flex-col gap-4 text-primary-text" :inert="!active" aria-label="语音预设">
    <div class="flex items-center gap-3">
      <ListChecks :size="22" :stroke-width="1.8" class="shrink-0 opacity-60" aria-hidden="true" />
      <h3 class="text-base font-semibold m-0">尚无语音预设</h3>
    </div>
    <p class="m-0 text-sm leading-relaxed opacity-70">
      语音预设用于约定回复长度、说话风格与提示词。目前仅提供入口，预设编辑与应用尚未接入。
    </p>
  </section>
</template>
