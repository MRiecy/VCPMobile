<script setup lang="ts">
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { syncService } from '../../../core/utils/syncService';
import type { AppSettings } from '../../../core/stores/settings';

const props = defineProps<{
  settings: AppSettings;
}>();

const emit = defineEmits<{
  (e: 'save-request'): void;
  (e: 'open-sync'): void;
}>();

const pingStatus = ref<{ type: 'success' | 'error' | 'loading' | null; message: string }>({ type: null, message: '' });
const emoticonStatus = ref<{ type: 'success' | 'error' | 'loading' | null; message: string }>({ type: null, message: '' });

const testSyncConnection = async () => {
  emit('save-request');

  pingStatus.value = { type: 'loading', message: '正在连接桌面端...' };
  try {
    const res = await syncService.pingServer(
      props.settings.syncServerIp,
      props.settings.syncServerPort,
      props.settings.syncToken
    );
    pingStatus.value = { type: 'success', message: `连接成功！设备: ${res.deviceName}` };
  } catch (e: any) {
    pingStatus.value = { type: 'error', message: `连接失败: ${e.message}` };
  }
};

const openSyncCenter = () => {
  emit('open-sync');
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
</script>

<template>
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
          <button @click="openSyncCenter" class="px-4 py-2 bg-blue-600 hover:bg-blue-500 text-white active:scale-95 transition-all rounded-xl text-xs font-bold tracking-wider shadow-lg shadow-blue-900/20">
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
</template>
