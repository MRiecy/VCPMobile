<script setup lang="ts">
import type { AppSettings } from '../../../core/stores/settings';
import SettingsSection from '../../../components/settings/SettingsSection.vue';
import SettingsCard from '../../../components/settings/SettingsCard.vue';
import SettingsRow from '../../../components/settings/SettingsRow.vue';
import SettingsSwitch from '../../../components/settings/SettingsSwitch.vue';

defineProps<{
  settings: AppSettings;
}>();
</script>

<template>
  <SettingsSection title="AI Engine Logic" accent-color="bg-purple-500">
    <SettingsCard no-padding>
      <div class="divide-y divide-black/5 dark:divide-white/5 px-3">
        <SettingsRow 
          title="开启打字机流式输出 (Smooth Stream)" 
          description="以固定帧率平滑渲染接收到的文字"
        >
          <template #action>
            <SettingsSwitch v-model="settings.enableSmoothStreaming" />
          </template>
        </SettingsRow>

        <SettingsRow title="VCP 动态工具路由注入">
          <template #action>
            <SettingsSwitch v-model="settings.enableVcpToolInjection" />
          </template>
        </SettingsRow>

        <SettingsRow title="气泡主题 UI 规范注入">
          <template #action>
            <SettingsSwitch v-model="settings.enableAgentBubbleTheme" />
          </template>
        </SettingsRow>
      </div>
    </SettingsCard>
  </SettingsSection>
</template>
