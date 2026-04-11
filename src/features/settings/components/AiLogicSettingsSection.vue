<script setup lang="ts">
import SettingsRow from "../../../components/settings/SettingsRow.vue";

defineProps<{
  settings?: Record<string, unknown>;
}>();
</script>

<template>
  <div class="divide-y divide-black/5 dark:divide-white/5 px-1">
    <SettingsRow title="VCP 动态工具路由注入">
      <template #action>
        <span class="text-xs opacity-60">暂未启用</span>
      </template>
    </SettingsRow>

    <SettingsRow title="气泡主题 UI 规范注入">
      <template #action>
        <span class="text-xs opacity-60">暂未启用</span>
      </template>
    </SettingsRow>
  </div>
</template>
