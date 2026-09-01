<script setup lang="ts">
import { onUnmounted, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { AppSettings, ChatEndpointMode } from '../../../core/stores/settings';
import SettingsTextField from '../../../components/settings/SettingsTextField.vue';
import SettingsActionWithStatus from '../../../components/settings/SettingsActionWithStatus.vue';

const props = defineProps<{
  settings: AppSettings;
}>();

const emit = defineEmits<{
  (e: 'save-request'): void;
}>();

const vcpPingStatus = ref<{ type: 'success' | 'error' | 'loading' | 'info' | null; message: string }>({ type: null, message: '' });

const endpointModes: ReadonlyArray<{
  value: ChatEndpointMode;
  title: string;
  description: string;
}> = [
  { value: 'standard', title: '标准 Chat', description: '兼容 VCPToolBox 标准路由与 NewAPI / OpenAI 上游' },
  { value: 'vcpTools', title: 'VCP 增强 Chat', description: '使用 ChatVCP 路由，显示 VCP 工具执行过程' },
  { value: 'raw', title: '原始 URL', description: '完整端点原样请求，不补全或替换路径' },
];

interface ChatEndpointPreview {
  finalUrl: string;
  modelDiscoveryUrl: string | null;
}

const endpointPreview = ref<ChatEndpointPreview | null>(null);
const endpointPreviewError = ref('');
let previewTimer: ReturnType<typeof setTimeout> | null = null;
let previewGeneration = 0;

const scheduleEndpointPreview = () => {
  if (previewTimer) clearTimeout(previewTimer);
  const generation = ++previewGeneration;
  const vcpUrl = props.settings.vcpServerUrl;
  const chatEndpointMode = props.settings.chatEndpointMode;
  endpointPreview.value = null;
  endpointPreviewError.value = '';
  if (!vcpUrl) return;

  previewTimer = setTimeout(async () => {
    try {
      const preview = await invoke<ChatEndpointPreview>('preview_chat_endpoint', {
        vcpUrl,
        chatEndpointMode,
      });
      if (generation !== previewGeneration) return;
      endpointPreview.value = preview;
    } catch (error) {
      if (generation !== previewGeneration) return;
      endpointPreviewError.value = String(error);
    }
  }, 120);
};

watch(
  [() => props.settings.vcpServerUrl, () => props.settings.chatEndpointMode],
  scheduleEndpointPreview,
  { immediate: true },
);

onUnmounted(() => {
  if (previewTimer) clearTimeout(previewTimer);
  previewGeneration += 1;
});

interface VcpConnectionTestResult {
  success: boolean;
  status: number;
  modelCount: number;
  models: unknown;
  modelDiscoveryAvailable: boolean;
}

const testVcpConnection = async () => {
  emit('save-request');

  if (!props.settings.vcpServerUrl) {
    vcpPingStatus.value = { type: 'error', message: '请先输入 Chat 服务器 URL' };
    return;
  }

  vcpPingStatus.value = { type: 'loading', message: '正在检查模型列表...' };
  try {
    const res = await invoke<VcpConnectionTestResult>('test_vcp_connection', {
      vcpUrl: props.settings.vcpServerUrl,
      vcpApiKey: props.settings.vcpApiKey,
      chatEndpointMode: props.settings.chatEndpointMode,
    });

    if (!res.modelDiscoveryAvailable) {
      vcpPingStatus.value = {
        type: 'info',
        message: '原始 URL 无法安全推导模型列表；已跳过模型发现，主聊天仍会使用原地址。',
      };
    } else if (res.success) {
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
    <SettingsTextField v-model="settings.vcpServerUrl" label="Chat 服务器 URL (HTTP/HTTPS)"
      placeholder="https://vcp-endpoint.com" />

    <fieldset class="border-t border-black/5 dark:border-white/5 pt-3">
      <legend class="text-[11px] font-black uppercase tracking-[0.12em] opacity-50 px-0">Chat 请求入口</legend>
      <div class="mt-2 divide-y divide-black/5 dark:divide-white/5" role="radiogroup" aria-label="Chat 请求入口">
        <button
          v-for="option in endpointModes"
          :key="option.value"
          type="button"
          role="radio"
          :aria-checked="settings.chatEndpointMode === option.value"
          class="relative w-full py-3 pl-3 pr-2 text-left transition-colors active:bg-black/5 dark:active:bg-white/5"
          @click="settings.chatEndpointMode = option.value"
        >
          <span class="absolute left-0 top-2 bottom-2 w-0.5 bg-blue-500 transition-opacity"
            :class="settings.chatEndpointMode === option.value ? 'opacity-100' : 'opacity-0'"></span>
          <span class="flex items-center justify-between gap-3">
            <span class="min-w-0">
              <span class="block text-sm font-semibold">{{ option.title }}</span>
              <span class="mt-0.5 block text-[11px] leading-4 opacity-50">{{ option.description }}</span>
            </span>
            <span class="h-3.5 w-3.5 shrink-0 rounded-full border"
              :class="settings.chatEndpointMode === option.value
                ? 'border-blue-500 bg-blue-500'
                : 'border-black/20 dark:border-white/25'"></span>
          </span>
        </button>
      </div>
    </fieldset>

    <div class="border-l-2 border-blue-500/70 bg-black/[0.025] dark:bg-white/[0.025] px-3 py-2.5">
      <div class="text-[10px] font-black uppercase tracking-[0.12em] opacity-45">最终请求 URL</div>
      <code v-if="endpointPreview" class="mt-1 block break-all text-[11px] leading-4 opacity-80">
        {{ endpointPreview.finalUrl }}
      </code>
      <p v-else-if="endpointPreviewError" class="mt-1 break-all text-[11px] leading-4 text-red-500">
        {{ endpointPreviewError }}
      </p>
      <p v-else class="mt-1 text-[11px] opacity-35">输入服务器地址后由 Rust 解析并显示</p>
      <p v-if="endpointPreview && !endpointPreview.modelDiscoveryUrl" class="mt-1 text-[10px] leading-4 opacity-45">
        此原始地址不提供可安全推导的模型发现端点
      </p>
    </div>

    <SettingsTextField v-model="settings.vcpApiKey" label="API Key" placeholder="输入 API Key" />

    <div class="border-t border-black/5 dark:border-white/5 pt-2"></div>

    <SettingsTextField v-model="settings.vcpLogUrl" label="VCP WebSocket 服务器 URL" placeholder="ws://localhost:6005"
      mono />
    <SettingsTextField v-model="settings.vcpLogKey" label="VCP WebSocket 鉴权 Key"
      placeholder="输入 WebSocket Key" mono />

    <SettingsActionWithStatus
      button-variant="primary"
      button-label="检查模型发现"
      :button-loading="vcpPingStatus.type === 'loading'"
      :status-type="vcpPingStatus.type"
      :status-message="vcpPingStatus.message"
      status-multiline
      @action-click="testVcpConnection"
    />
  </div>
</template>
