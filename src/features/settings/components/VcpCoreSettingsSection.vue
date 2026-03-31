<script setup lang="ts">
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { AppSettings } from '../../../core/stores/settings';
import SettingsSection from '../../../components/settings/SettingsSection.vue';
import SettingsCard from '../../../components/settings/SettingsCard.vue';
import SettingsTextField from '../../../components/settings/SettingsTextField.vue';
import SettingsActionButton from '../../../components/settings/SettingsActionButton.vue';
import SettingsInlineStatus from '../../../components/settings/SettingsInlineStatus.vue';

const props = defineProps<{
  settings: AppSettings;
}>();

const emit = defineEmits<{
  (e: 'save-request'): void;
}>();

const vcpPingStatus = ref<{ type: 'success' | 'error' | 'loading' | null; message: string }>({ type: null, message: '' });

const testVcpConnection = async () => {
  emit('save-request');
  
  if (!props.settings.vcpServerUrl) {
    vcpPingStatus.value = { type: 'error', message: '请先输入 VCP 服务器 URL' };
    return;
  }

  vcpPingStatus.value = { type: 'loading', message: '正在验证模型列表...' };
  try {
    const res = await invoke<{success: boolean, status: number, modelCount: number, models: any}>('test_vcp_connection', {
      vcpUrl: props.settings.vcpServerUrl,
      vcpApiKey: props.settings.vcpApiKey
    });
    
    if (res.success) {
      vcpPingStatus.value = { type: 'success', message: `连接成功！拉取到 ${res.modelCount} 个可用模型` };
    } else {
      vcpPingStatus.value = { type: 'error', message: `验证失败, HTTP 状态码: ${res.status}` };
    }
  } catch (e: any) {
    vcpPingStatus.value = { type: 'error', message: `${e}` };
  }
};
</script>

<template>
  <SettingsSection title="核心连接" accent-color="bg-blue-500">
    <SettingsCard class="space-y-5">
      <SettingsTextField 
        v-model="settings.vcpServerUrl" 
        label="VCP 服务器 URL (HTTP/HTTPS)" 
        placeholder="https://vcp-endpoint.com" 
      />
      <SettingsTextField 
        v-model="settings.vcpApiKey" 
        type="password" 
        label="VCP API Key" 
        placeholder="••••••••" 
      />

      <div class="border-t border-black/5 dark:border-white/5 pt-2"></div>

      <SettingsTextField 
        v-model="settings.vcpLogUrl" 
        label="VCP WebSocket 服务器 URL" 
        placeholder="ws://localhost:8024" 
        mono
      />
      <SettingsTextField 
        v-model="settings.vcpLogKey" 
        type="password" 
        label="VCP WebSocket 鉴权 Key" 
        placeholder="输入 WebSocket Key" 
        mono
      />

      <div class="pt-2 flex items-center justify-between gap-4">
        <SettingsInlineStatus 
          v-if="vcpPingStatus.type"
          :type="vcpPingStatus.type" 
          :message="vcpPingStatus.message" 
          class="flex-1"
        />
        <SettingsActionButton 
          variant="primary" 
          :loading="vcpPingStatus.type === 'loading'"
          @click="testVcpConnection"
        >
          验证连接
        </SettingsActionButton>
      </div>
    </SettingsCard>
  </SettingsSection>
</template>
