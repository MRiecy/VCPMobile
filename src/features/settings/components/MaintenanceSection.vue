<script setup lang="ts">
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';

const cleanupStatus = ref<{ type: 'success' | 'error' | 'loading' | null; message: string }>({ type: null, message: '' });

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
</script>

<template>
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
</template>
