<script setup lang="ts">
import type { AppSettings } from '../../../core/stores/settings';
import SettingsCard from '../../../components/settings/SettingsCard.vue';
import SettingsTextField from '../../../components/settings/SettingsTextField.vue';

defineProps<{
  settings: AppSettings;
}>();
</script>

<template>
  <SettingsCard variant="glass">
    <div class="flex items-center gap-5">
      <div class="w-16 h-16 rounded-2xl bg-black/5 dark:bg-white/5 flex items-center justify-center relative overflow-hidden border-2 border-black/5 dark:border-white/10 shadow-inner shrink-0">
        <img :src="`vcp-avatar://user/default?t=${Date.now()}`" class="w-full h-full object-cover" />
      </div>
      <div class="flex-1 min-w-0">
        <SettingsTextField 
          v-model="settings.userName" 
          label="用户名" 
          placeholder="输入你的名字..."
        />
      </div>
    </div>
  </SettingsCard>
</template>
