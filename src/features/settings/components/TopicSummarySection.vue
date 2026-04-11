<script setup lang="ts">
import type { AppSettings } from "../../../core/stores/settings";
import SettingsTextField from "../../../components/settings/SettingsTextField.vue";
import SettingsActionButton from "../../../components/settings/SettingsActionButton.vue";

defineProps<{
  settings: AppSettings;
}>();

const emit = defineEmits<{
  (e: "open-model-selector"): void;
}>();
</script>

<template>
  <div class="flex gap-3 items-end px-1">
    <div class="flex-1">
      <SettingsTextField v-model="settings.topicSummaryModel" label="总结专用模型"
        placeholder="默认: gemini-3.1-flash-lite-preview" mono />
    </div>
    <SettingsActionButton variant="secondary" @click="emit('open-model-selector')" class="!p-3.5 !rounded-2xl">
      <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
        stroke-linecap="round" stroke-linejoin="round">
        <path d="M8.25 15L12 18.75 15.75 15m-7.5-6L12 5.25 15.75 9"></path>
      </svg>
    </SettingsActionButton>
  </div>
</template>
