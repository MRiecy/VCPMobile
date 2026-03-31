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
        <img v-if="settings.resolvedUserAvatarUrl || settings.userAvatarUrl" :src="settings.resolvedUserAvatarUrl || settings.userAvatarUrl" class="w-full h-full object-cover" />
        <svg v-else width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round" class="opacity-20"><path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"></path><circle cx="12" cy="7" r="4"></circle></svg>
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
