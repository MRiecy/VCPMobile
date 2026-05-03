<script setup lang="ts">
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { AppSettings } from '../../../core/stores/settings';
import SettingsTextField from '../../../components/settings/SettingsTextField.vue';
import SettingsActionWithStatus from '../../../components/settings/SettingsActionWithStatus.vue';

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
    const res = await invoke<{ success: boolean, status: number, modelCount: number, models: any }>('test_vcp_connection', {
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
  <div class="space-y-5 px-1">
    <SettingsTextField v-model="settings.vcpServerUrl" label="VCP 服务器 URL (HTTP/HTTPS)"
      placeholder="https://vcp-endpoint.com" />
    <SettingsTextField v-model="settings.vcpApiKey" is-secure label="VCP API Key" placeholder="输入 API Key" />

    <div class="border-t border-black/5 dark:border-white/5 pt-2"></div>

    <SettingsTextField v-model="settings.vcpLogUrl" label="VCP WebSocket 服务器 URL" placeholder="ws://localhost:8024"
      mono />
    <SettingsTextField v-model="settings.vcpLogKey" is-secure label="VCP WebSocket 鉴权 Key"
      placeholder="输入 WebSocket Key" mono />

    <SettingsActionWithStatus
      button-variant="primary"
      button-label="验证连接"
      :button-loading="vcpPingStatus.type === 'loading'"
      :status-type="vcpPingStatus.type"
      :status-message="vcpPingStatus.message"
      status-multiline
      @action-click="testVcpConnection"
    />
  </div>
</template>
