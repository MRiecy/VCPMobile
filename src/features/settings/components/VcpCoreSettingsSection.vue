<script setup lang="ts">
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { AppSettings } from '../../../core/stores/settings';

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
  <section class="space-y-4">
    <div class="flex items-center gap-2 px-1">
      <div class="w-1 h-4 bg-blue-500 rounded-full"></div>
      <h3 class="text-xs font-black uppercase tracking-[0.2em] opacity-50 dark:opacity-40">核心连接</h3>
    </div>
    <div class="card-modern space-y-5">
      <div>
        <label class="text-[11px] uppercase font-bold opacity-50 dark:opacity-40 mb-2 block">VCP 服务器 URL (HTTP/HTTPS)</label>
        <input v-model="settings.vcpServerUrl" placeholder="https://vcp-endpoint.com" class="w-full bg-black/5 dark:bg-white/5 p-3.5 rounded-2xl outline-none border border-black/5 dark:border-white/5 focus:border-blue-500/50 focus:bg-black/10 dark:focus:bg-white/10 transition-all" />
      </div>
      <div>
        <label class="text-[11px] uppercase font-bold opacity-50 dark:opacity-40 mb-2 block">VCP API Key</label>
        <input v-model="settings.vcpApiKey" type="password" placeholder="••••••••" class="w-full bg-black/5 dark:bg-white/5 p-3.5 rounded-2xl outline-none border border-black/5 dark:border-white/5 focus:border-blue-500/50 focus:bg-black/10 dark:focus:bg-white/10 transition-all" />
      </div>

      <div class="border-t border-black/5 dark:border-white/5 pt-4 mt-2"></div>

      <div>
        <label class="text-[11px] uppercase font-bold opacity-50 dark:opacity-40 mb-2 block">VCP WebSocket 服务器 URL</label>
        <input v-model="settings.vcpLogUrl" placeholder="ws://localhost:8024" class="w-full bg-black/5 dark:bg-white/5 p-3.5 rounded-2xl outline-none border border-black/5 dark:border-white/5 focus:border-blue-500/50 focus:bg-black/10 dark:focus:bg-white/10 transition-all font-mono text-sm" />
      </div>
      <div>
        <label class="text-[11px] uppercase font-bold opacity-50 dark:opacity-40 mb-2 block">VCP WebSocket 鉴权 Key</label>
        <input v-model="settings.vcpLogKey" type="password" placeholder="输入 WebSocket Key" class="w-full bg-black/5 dark:bg-white/5 p-3.5 rounded-2xl outline-none border border-black/5 dark:border-white/5 focus:border-blue-500/50 focus:bg-black/10 dark:focus:bg-white/10 transition-all font-mono text-sm" />
      </div>

      <div class="pt-2 flex items-center justify-between">
        <div class="text-[10px] font-medium leading-tight max-w-[65%]" :class="{
          'text-blue-500 dark:text-blue-400': vcpPingStatus.type === 'success',
          'text-red-500 dark:text-red-400': vcpPingStatus.type === 'error',
          'text-purple-500 dark:text-purple-400 animate-pulse': vcpPingStatus.type === 'loading',
          'opacity-0': !vcpPingStatus.type
        }">
          {{ vcpPingStatus.message }}
        </div>
        <div class="flex gap-2">
          <button @click="testVcpConnection" :disabled="vcpPingStatus.type === 'loading'" class="px-5 py-2.5 bg-blue-600 hover:bg-blue-500 text-white active:scale-95 transition-all rounded-xl text-xs font-bold tracking-wider shadow-lg shadow-blue-900/20 disabled:opacity-50">
            验证连接
          </button>
        </div>
      </div>
    </div>
  </section>
</template>
