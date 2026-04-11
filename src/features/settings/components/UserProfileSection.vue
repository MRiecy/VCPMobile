<script setup lang="ts">
import { computed } from "vue";
import type { AppSettings } from "../../../core/stores/settings";
import SettingsCard from "../../../components/settings/SettingsCard.vue";
import SettingsTextField from "../../../components/settings/SettingsTextField.vue";

const props = defineProps<{
  settings: AppSettings;
}>();

const fallbackStyle = computed(() => ({
  backgroundColor: "rgb(226,54,56)",
}));

const fallbackInitial = computed(() => {
  const text = (props.settings.userName || "U").trim();
  return text.charAt(0).toUpperCase() || "U";
});
</script>

<template>
  <SettingsCard variant="glass">
    <div class="flex items-center gap-5">
      <div
        class="w-16 h-16 rounded-2xl bg-black/5 dark:bg-white/5 flex items-center justify-center relative overflow-hidden border-2 border-black/5 dark:border-white/10 shadow-inner shrink-0">
        <img :src="`vcp-avatar://user/default?t=${Date.now()}`" class="w-full h-full object-cover relative z-10" />
        <div class="absolute inset-0 flex items-center justify-center text-white text-xl font-bold"
          :style="fallbackStyle">
          {{ fallbackInitial }}
        </div>
      </div>
      <div class="flex-1 min-w-0">
        <SettingsTextField v-model="settings.userName" label="用户名" placeholder="输入你的名字..." />
      </div>
    </div>
  </SettingsCard>
</template>
