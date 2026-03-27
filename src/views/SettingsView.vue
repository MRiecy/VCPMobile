<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useRouter } from 'vue-router';
import ThemePicker from '../components/ThemePicker.vue';
import ModelSelector from '../components/ModelSelector.vue';
import { syncService } from '../services/syncService';
import { useSettingsStore, type AppSettings } from '../stores/settings';

const settingsStore = useSettingsStore();
const router = useRouter();

const settings = ref<AppSettings>({
  userName: 'User',
  vcpServerUrl: '',
  vcpApiKey: '',
  vcpLogUrl: '',
  vcpLogKey: '',
  enableAgentBubbleTheme: false,
  enableSmoothStreaming: false,
  enableDistributedServer: true,
  enableDistributedServerLogs: false,
  enableVcpToolInjection: false,
  syncServerIp: '',
  syncServerPort: 5974,
  syncToken: '',
  sidebarWidth: 260,
  notificationsSidebarWidth: 300,
  networkNotesPaths: [],
  minChunkBufferSize: 1,
  smoothStreamIntervalMs: 25,
  assistantAgent: '',
  agentMusicControl: false,
  combinedItemOrder: [],
  agentOrder: [],
  flowlockContinueDelay: 5,
  topicSummaryModel: 'gemini-2.5-flash',
  topicSummaryModelTemperature: 0.7
});

const loading = ref(true);
const pingStatus = ref<{ type: 'success' | 'error' | 'loading' | null; message: string }>({ type: null, message: '' });
const vcpPingStatus = ref<{ type: 'success' | 'error' | 'loading' | null; message: string }>({ type: null, message: '' });
const emoticonStatus = ref<{ type: 'success' | 'error' | 'loading' | null; message: string }>({ type: null, message: '' });
const cleanupStatus = ref<{ type: 'success' | 'error' | 'loading' | null; message: string }>({ type: null, message: '' });

const showSummaryModelSelector = ref(false);
const onSummaryModelSelect = (modelId: string) => {
  settings.value.topicSummaryModel = modelId;
};

const emit = defineEmits(['close', 'open-sync']);

const closeSettings = () => {
  if (window.history.length > 1) {
    router.back();
    return;
  }
  router.replace('/chat');
};

const loadSettings = async () => {
  try {
    await settingsStore.fetchSettings();
    if (settingsStore.settings) {
      // 深度拷贝到组件的 ref，解耦双向绑定导致 Store 的即时污染
      settings.value = JSON.parse(JSON.stringify(settingsStore.settings));
    }
  } catch (e) {
    console.error('[SettingsView] Failed to load settings:', e);
  } finally {
    loading.value = false;
  }
};

const saveSettings = async () => {
  try {
    await invoke('write_app_settings', { settings: settings.value });
    // 同步更新 Pinia Store，以便 SyncService 和 头像渲染 能立即拿到最新配置
    await settingsStore.fetchSettings();
    // TODO: Toast notification
    console.log('Settings saved!');
  } catch (e) {
    console.error('Failed to save settings:', e);
  }
};

const rebuildEmoticonLibrary = async () => {
  emoticonStatus.value = { type: 'loading', message: '正在扫描表情包...' };
  try {
    const count = await invoke<number>('regenerate_emoticon_library');
    emoticonStatus.value = { type: 'success', message: `成功重载 ${count} 个表情包` };
    setTimeout(() => { emoticonStatus.value = { type: null, message: '' }; }, 3000);
  } catch (e: any) {
    emoticonStatus.value = { type: 'error', message: `重载失败: ${e}` };
  }
};

const cleanupAttachments = async () => {
  cleanupStatus.value = { type: 'loading', message: '正在深度扫描孤儿附件...' };
  try {
    const result = await invoke<string>('cleanup_orphaned_attachments');
    cleanupStatus.value = { type: 'success', message: result };
    setTimeout(() => { cleanupStatus.value = { type: null, message: '' }; }, 5000);
  } catch (e: any) {
    cleanupStatus.value = { type: 'error', message: `清理失败: ${e}` };
  }
};

const testSyncConnection = async () => {
  // 先保存当前设置
  await saveSettings();
  
  pingStatus.value = { type: 'loading', message: '正在连接桌面端...' };
  try {
    // 直接传入当前输入框的值，避免 Pinia 状态未及时更新导致请求旧地址
    const res = await syncService.pingServer(
      settings.value.syncServerIp,
      settings.value.syncServerPort,
      settings.value.syncToken
    );
    pingStatus.value = { type: 'success', message: `连接成功！设备: ${res.deviceName}` };
  } catch (e: any) {
    pingStatus.value = { type: 'error', message: `连接失败: ${e.message}` };
  }
};

const testVcpConnection = async () => {
  // 先保存当前设置，确保后端读到的是最新值
  await saveSettings();
  
  if (!settings.value.vcpServerUrl) {
    vcpPingStatus.value = { type: 'error', message: '请先输入 VCP 服务器 URL' };
    return;
  }

  vcpPingStatus.value = { type: 'loading', message: '正在验证模型列表...' };
  try {
    const res = await invoke<{success: boolean, status: number, modelCount: number, models: any}>('test_vcp_connection', {
      vcpUrl: settings.value.vcpServerUrl,
      vcpApiKey: settings.value.vcpApiKey
    });
    
    if (res.success) {
      vcpPingStatus.value = { type: 'success', message: `连接成功！拉取到 ${res.modelCount} 个可用模型` };
      // TODO: 未来可将 res.models 存入 Pinia 供 Agent 设置下拉框使用
    } else {
      vcpPingStatus.value = { type: 'error', message: `验证失败, HTTP 状态码: ${res.status}` };
    }
  } catch (e: any) {
    vcpPingStatus.value = { type: 'error', message: `${e}` };
  }
};

onMounted(() => {
  loadSettings();
});

onUnmounted(() => {
  console.info('[SettingsView][Debug] component unmounted');
});
</script>

<template>
  <div class="settings-view h-full flex flex-col bg-secondary-bg text-primary-text">
    
    <!-- Header (仅在全屏模式下显示) -->
    <header class="p-4 flex items-center justify-between border-b border-white/10 pt-[calc(var(--vcp-safe-top,24px)+20px)] pb-6 shrink-0">
      <h2 class="text-xl font-bold">全局设置</h2>
      <button @click="closeSettings" class="p-2.5 bg-white/10 rounded-full active:scale-90 transition-all flex items-center justify-center">
        <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>
      </button>
    </header>

    <!-- Scrollable Form Area -->
    <div class="flex-1 overflow-y-auto p-5 space-y-8 pb-safe">
      
      <!-- 1. 用户信息区 (身份展示) -->
      <section class="card-modern">
        <div class="flex items-center gap-5">
          <div class="w-20 h-20 rounded-full bg-black/5 dark:bg-white/5 flex-center relative overflow-hidden border-2 border-black/5 dark:border-white/10 shadow-inner">
            <img v-if="settings.userAvatarUrl" :src="settings.userAvatarUrl" class="w-full h-full object-cover" />
            <svg v-else width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round" class="opacity-20"><path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"></path><circle cx="12" cy="7" r="4"></circle></svg>
          </div>
          <div class="flex-1">
            <label class="text-[11px] uppercase font-black tracking-widest opacity-40 dark:opacity-30 mb-1.5 block">用户名</label>
            <input v-model="settings.userName" class="bg-transparent border-b border-black/10 dark:border-white/10 w-full focus:border-blue-500 outline-none py-1.5 text-lg font-medium transition-colors text-primary-text" />
          </div>
        </div>
      </section>

      <!-- 2. 数据同步配置 (VCPMobileSync) -->
      <section class="space-y-4">
        <div class="flex items-center gap-2 px-1">
          <div class="w-1 h-4 bg-green-500 rounded-full"></div>
          <h3 class="text-xs font-black uppercase tracking-[0.2em] opacity-50 dark:opacity-40">桌面端数据同步</h3>
        </div>
        <div class="card-modern space-y-5">
          <div class="flex gap-4">
            <div class="flex-[2]">
              <label class="text-[11px] uppercase font-bold opacity-50 dark:opacity-40 mb-2 block">同步服务器 IP</label>
              <input v-model="settings.syncServerIp" placeholder="192.168.x.x" class="w-full bg-black/5 dark:bg-white/5 p-3.5 rounded-2xl outline-none border border-black/5 dark:border-white/5 focus:border-green-500/50 focus:bg-black/10 dark:focus:bg-white/10 transition-all font-mono text-sm" />
            </div>
            <div class="flex-1">
              <label class="text-[11px] uppercase font-bold opacity-50 dark:opacity-40 mb-2 block">端口</label>
              <input v-model.number="settings.syncServerPort" type="number" placeholder="5974" class="w-full bg-black/5 dark:bg-white/5 p-3.5 rounded-2xl outline-none border border-black/5 dark:border-white/5 focus:border-green-500/50 focus:bg-black/10 dark:focus:bg-white/10 transition-all font-mono text-sm text-center" />
            </div>
          </div>
          <div>
            <label class="text-[11px] uppercase font-bold opacity-50 dark:opacity-40 mb-2 block">Mobile Sync Token</label>
            <input v-model="settings.syncToken" type="text" placeholder="输入桌面端 config.env 中的 Token" class="w-full bg-black/5 dark:bg-white/5 p-3.5 rounded-2xl outline-none border border-black/5 dark:border-white/5 focus:border-green-500/50 focus:bg-black/10 dark:focus:bg-white/10 transition-all font-mono text-sm" />
          </div>
          
          <div class="pt-2 flex items-center justify-between">
            <div class="text-xs font-medium" :class="{
              'text-green-500 dark:text-green-400': pingStatus.type === 'success',
              'text-red-500 dark:text-red-400': pingStatus.type === 'error',
              'text-blue-500 dark:text-blue-400 animate-pulse': pingStatus.type === 'loading',
              'opacity-0': !pingStatus.type
            }">
              {{ pingStatus.message }}
            </div>
            <div class="flex gap-2">
              <button @click="testSyncConnection" :disabled="pingStatus.type === 'loading'" class="px-4 py-2 bg-black/5 dark:bg-white/10 hover:bg-black/10 dark:hover:bg-white/20 active:scale-95 transition-all rounded-xl text-xs font-bold tracking-wider disabled:opacity-50">
                测试连接
              </button>
              <button @click="emit('open-sync')" class="px-4 py-2 bg-blue-600 hover:bg-blue-500 text-white active:scale-95 transition-all rounded-xl text-xs font-bold tracking-wider shadow-lg shadow-blue-900/20">
                进入同步面板
              </button>
            </div>
          </div>

          <div class="border-t border-black/5 dark:border-white/5 pt-4 mt-2 flex items-center justify-between">
            <div class="flex flex-col">
              <span class="text-xs font-bold opacity-60">本地表情包修复库</span>
              <span class="text-[9px] opacity-30 uppercase font-mono">{{ emoticonStatus.message || 'IDLE' }}</span>
            </div>
            <button @click="rebuildEmoticonLibrary" :disabled="emoticonStatus.type === 'loading'" class="px-3 py-1.5 bg-black/5 dark:bg-white/10 hover:bg-black/10 dark:hover:bg-white/20 active:scale-95 transition-all rounded-lg text-[10px] font-bold tracking-tight disabled:opacity-50 flex items-center gap-2">
              <div v-if="emoticonStatus.type === 'loading'" class="w-2 h-2 rounded-full bg-blue-500 animate-ping"></div>
              RESCAN_LIBRARY
            </button>
          </div>
        </div>
      </section>

      <!-- 3. VCP 核心配置 (网络) -->
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

      <!-- 3. AI 交互逻辑 (下沉核心) -->
      <section class="space-y-4">
        <div class="flex items-center gap-2 px-1">
          <div class="w-1 h-4 bg-purple-500 rounded-full"></div>
          <h3 class="text-xs font-black uppercase tracking-[0.2em] opacity-50 dark:opacity-40">AI Engine Logic</h3>
        </div>
        <div class="card-modern divide-y divide-black/5 dark:divide-white/5">
          <div class="flex justify-between items-center py-3.5">
            <div class="flex flex-col">
              <span class="text-[15px] font-semibold">开启打字机流式输出 (Smooth Stream)</span>
              <span class="text-[10px] opacity-40 dark:opacity-30">以固定帧率平滑渲染接收到的文字</span>
            </div>
            <div class="relative inline-flex items-center cursor-pointer">
              <input type="checkbox" v-model="settings.enableSmoothStreaming" class="sr-only peer">
              <div class="w-11 h-6 bg-black/10 dark:bg-white/10 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-blue-500"></div>
            </div>
          </div>

          <div class="flex justify-between items-center py-3.5">
            <span class="text-[15px] font-semibold">VCP 动态工具路由注入</span>
            <div class="relative inline-flex items-center cursor-pointer">
              <input type="checkbox" v-model="settings.enableVcpToolInjection" class="sr-only peer">
              <div class="w-11 h-6 bg-black/10 dark:bg-white/10 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-blue-500"></div>
            </div>
          </div>

          <div class="flex justify-between items-center py-3.5">
            <span class="text-[15px] font-semibold">气泡主题 UI 规范注入</span>
            <div class="relative inline-flex items-center cursor-pointer">
              <input type="checkbox" v-model="settings.enableAgentBubbleTheme" class="sr-only peer">
              <div class="w-11 h-6 bg-black/10 dark:bg-white/10 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-blue-500"></div>
            </div>
          </div>
        </div>
      </section>

      <!-- 4. 话题总结配置 -->
      <section class="space-y-4">
        <div class="flex items-center gap-2 px-1">
          <div class="w-1 h-4 bg-yellow-500 rounded-full"></div>
          <h3 class="text-xs font-black uppercase tracking-[0.2em] opacity-50 dark:opacity-40">话题总结 (Topic Summary)</h3>
        </div>
        <div class="card-modern">
          <label class="text-[11px] uppercase font-bold opacity-50 dark:opacity-40 mb-2 block">总结专用模型</label>
          <div class="flex gap-2">
            <input v-model="settings.topicSummaryModel" placeholder="默认: gemini-2.5-flash" class="flex-1 bg-black/5 dark:bg-white/5 p-3.5 rounded-2xl outline-none border border-black/5 dark:border-white/5 focus:border-yellow-500/50 focus:bg-black/10 dark:focus:bg-white/10 transition-all font-mono text-sm" />
            <button @click="showSummaryModelSelector = true" class="w-12 h-12 bg-yellow-500/10 text-yellow-500 rounded-2xl flex-center active:scale-90 transition-all">
              <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M8.25 15L12 18.75 15.75 15m-7.5-6L12 5.25 15.75 9"></path></svg>
            </button>
          </div>
        </div>
      </section>

      <!-- 5. 数据维护 -->
      <section class="space-y-4">
        <div class="flex items-center gap-2 px-1">
          <div class="w-1 h-4 bg-red-500 rounded-full"></div>
          <h3 class="text-xs font-black uppercase tracking-[0.2em] opacity-50 dark:opacity-40">数据维护 (Maintenance)</h3>
        </div>
        <div class="card-modern space-y-4">
          <div class="flex items-center justify-between">
            <div class="flex flex-col">
              <span class="text-sm font-bold">附件库垃圾回收 (GC)</span>
              <span class="text-[10px] opacity-40">深度扫描并删除未被引用的孤立附件与缩略图</span>
            </div>
            <button @click="cleanupAttachments" :disabled="cleanupStatus.type === 'loading'" class="px-4 py-2 bg-red-500/10 text-red-500 hover:bg-red-500/20 active:scale-95 transition-all rounded-xl text-[11px] font-bold tracking-tight disabled:opacity-50">
              立即清理
            </button>
          </div>
          <div v-if="cleanupStatus.type" class="text-[10px] p-3 rounded-xl bg-black/5 dark:bg-white/5 border border-black/5 dark:border-white/10 font-mono" :class="{
            'text-blue-500': cleanupStatus.type === 'loading',
            'text-green-500': cleanupStatus.type === 'success',
            'text-red-500': cleanupStatus.type === 'error'
          }">
            {{ cleanupStatus.message }}
          </div>
        </div>
      </section>

      <!-- 6. 视觉主题 -->
      <section class="space-y-4">
        <div class="flex items-center gap-2 px-1">
          <div class="w-1 h-4 bg-orange-500 rounded-full"></div>
          <h3 class="text-xs font-black uppercase tracking-[0.2em] opacity-50 dark:opacity-40">视觉长廊</h3>
        </div>
        <ThemePicker />
      </section>

      <div class="h-4"></div>

      <!-- 保存按钮 -->
      <button @click="saveSettings" 
              class="w-full py-4.5 bg-blue-600 hover:bg-blue-500 text-white active:scale-95 transition-all rounded-[1.25rem] font-black uppercase tracking-widest text-xs shadow-xl shadow-blue-900/20">
              保存并应用变更
      </button>

      <!-- 版本信息 -->
      <div class="text-center opacity-10 text-[9px] py-8 pb-12 font-mono uppercase tracking-widest">
        VCP MOBILE · PROJECT AVATAR<br/>INTERNAL RELEASE 2026.03.13
      </div>
    </div>

    <!-- 话题总结模型选择器 -->
    <ModelSelector 
      v-model="showSummaryModelSelector" 
      :current-model="settings.topicSummaryModel"
      title="选择总结专用模型"
      @select="onSummaryModelSelect"
    />
  </div>
</template>


<style scoped>
.settings-view {
  background-color: color-mix(in srgb, var(--primary-bg) 85%, transparent);
  backdrop-filter: blur(30px) saturate(180%);
}

.card-modern {
  @apply bg-white/5 border border-white/10 rounded-[2rem] p-6 backdrop-blur-xl shadow-2xl;
}

input[type="number"]::-webkit-inner-spin-button,
input[type="number"]::-webkit-outer-spin-button {
  -webkit-appearance: none;
  margin: 0;
}
</style>
