<script setup lang="ts">
import { ref, computed, reactive } from 'vue';
import { syncService, type SyncTreeNode } from '../../core/utils/syncService';
import SyncFileTree from './SyncFileTree.vue';

const props = withDefaults(defineProps<{
  isOpen?: boolean;
}>(), {
  isOpen: false
});

const emit = defineEmits<{
  close: [];
}>();

const loading = ref(true);
const error = ref<string | null>(null);
const fileTree = ref<SyncTreeNode | null>(null);
const manifestMap = ref<Record<string, { size: number, mtimeMs: number }>>({});
const localManifestMap = ref<Record<string, { size: number, mtimeMs: number }>>({});

// 使用 reactive Set 保证响应式
const selectedPaths = reactive<Set<string>>(new Set());

// 同步状态
const isSyncing = ref(false);
const syncProgress = ref({ current: 0, total: 0, file: '' });

const closeSync = () => {
  emit('close');
};

const loadManifest = async () => {
  loading.value = true;
  error.value = null;
  try {
    const res = await syncService.fetchManifest();
    fileTree.value = res.tree;
    manifestMap.value = res.manifest;

    // 获取本地文件的清单进行对比
    const allPaths = Object.keys(res.manifest);
    localManifestMap.value = await syncService.getLocalManifest(allPaths);

    // 智能选择：默认选中所有 “新增” 或 “已修改” 的文件
    selectedPaths.clear();
    allPaths.forEach(path => {
      const remote = manifestMap.value[path];
      const local = localManifestMap.value[path];

      // 如果本地不存在，或者 mtime 不一致（桌面端通常比移动端新），则默认选中
      if (!local || Math.abs(remote.mtimeMs - local.mtimeMs) > 2000) {
        selectedPaths.add(path);
      }
    });

    // 始终确保 settings.json 被检查（如果是强制的话）
    if (res.manifest['settings.json']) {
      selectedPaths.add('settings.json');
    }
  } catch (e: any) {
    error.value = e.message || '获取同步清单失败';
  } finally {
    loading.value = false;
  }
};

const getFileStatus = (path: string) => {
  const remote = manifestMap.value[path];
  const local = localManifestMap.value[path];

  if (!local) return { label: '新增', color: 'text-green-400', icon: 'M12 5v14M5 12h14' };

  // 允许 2s 的误差（由于文件系统精度差异）
  if (Math.abs(remote.mtimeMs - local.mtimeMs) > 2000) {
    return { label: '已修改', color: 'text-blue-400', icon: 'M23 4v6h-6M1 20v-6h6M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15' };
  }

  return { label: '已同步', color: 'text-white/30', icon: 'M20 6L9 17l-5-5' };
};

const handleToggleFiles = (paths: string[], checked: boolean) => {
  if (checked) {
    paths.forEach(p => selectedPaths.add(p));
  } else {
    paths.forEach(p => selectedPaths.delete(p));
  }
};

const formatBytes = (bytes: number) => {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
};

const truncatePath = (path: string) => {
  if (!path) return '';
  const parts = path.split('/');
  if (parts.length > 2) {
    return `.../${parts[parts.length - 2]}/${parts[parts.length - 1]}`;
  }
  return path;
};

const selectedStats = computed(() => {
  let size = 0;
  selectedPaths.forEach(path => {
    if (manifestMap.value[path]) {
      size += manifestMap.value[path].size;
    }
  });
  return { count: selectedPaths.size, size };
});

const startSync = async () => {
  if (selectedPaths.size === 0) return;

  isSyncing.value = true;
  syncProgress.value = { current: 0, total: 0, file: '准备同步...' };

  try {
    const filesToDownload = Array.from(selectedPaths);
    await syncService.startSync(filesToDownload, (current: number, total: number, file: string) => {
      syncProgress.value = { current, total, file };
    });

    // 同步完成后，触发状态热重载
    syncProgress.value.file = '正在重载本地数据...';

    setTimeout(() => {
      alert('同步完成！部分数据可能需要重启应用才能完全生效。');
      // 因为底层配置文件已经被替换，强制刷新页面是最安全的做法
      window.location.replace('/');
    }, 1000);

  } catch (e: any) {
    alert(`同步过程中发生错误: ${e.message}`);
  } finally {
    isSyncing.value = false;
  }
};

import { watch } from 'vue';

watch(() => props.isOpen, (val) => {
  if (val) {
    loadManifest();
  }
});
</script>

<template>
  <Teleport to="#vcp-feature-overlays" :disabled="!props.isOpen">
    <Transition name="fade">
      <div v-if="props.isOpen" class="sync-view fixed inset-0 flex flex-col bg-secondary-bg/95 text-primary-text pointer-events-auto">
        <!-- Header -->
        <header class="p-4 flex items-center justify-between border-b border-white/10 pt-[calc(var(--vcp-safe-top,24px)+20px)] pb-6 shrink-0 bg-secondary-bg/80 backdrop-blur-md">
            <div class="flex items-center gap-3">
              <button @click="closeSync" class="p-2 bg-white/5 rounded-full active:scale-90 transition-all">
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m15 18-6-6 6-6"/></svg>
              </button>
            <h2 class="text-xl font-bold">数据同步中心</h2>
          </div>
        </header>

        <!-- Content -->
        <div class="flex-1 overflow-y-auto p-5 space-y-6 pb-[120px]">
          <div v-if="loading" class="flex-center h-40 flex-col gap-4 opacity-50">
              <div class="w-8 h-8 border-4 border-blue-500 border-t-transparent rounded-full animate-spin"></div>
            <span class="text-sm font-bold tracking-widest uppercase">正在分析桌面端数据...</span>
          </div>

          <div v-else-if="error" class="card-modern border-red-500/30 bg-red-500/10 text-red-200">
            <h3 class="font-bold mb-2">连接失败</h3>
            <p class="text-sm opacity-80 break-words">{{ error }}</p>
            <button @click="loadManifest" class="mt-4 px-4 py-2 bg-red-500/20 rounded-xl text-xs font-bold active:scale-95 transition-all">重试</button>
          </div>

          <template v-else-if="fileTree">
            <!-- 总体统计 -->
            <div class="card-modern flex justify-between items-center bg-gradient-to-br from-blue-900/40 to-purple-900/40 border-blue-500/30">
              <div class="flex flex-col min-w-0">
                <div class="text-[10px] uppercase font-black tracking-widest opacity-50 mb-1 truncate">桌面端总数据量</div>
                <div class="text-2xl font-bold truncate">{{ formatBytes(fileTree.sizeBytes) }}</div>
                <div class="text-xs opacity-60 mt-1 truncate">已选 {{ selectedStats.count }} 项，共 {{ formatBytes(selectedStats.size) }}</div>
              </div>
              <div class="w-12 h-12 rounded-full bg-blue-500/20 flex-center text-blue-400 shrink-0 ml-4">
                <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" x2="12" y1="15" y2="3"/></svg>
              </div>
            </div>

            <!-- 真实文件树 -->
            <div class="space-y-4">
              <div class="flex items-center gap-2 px-1">
                <div class="w-1 h-4 bg-green-500 rounded-full shrink-0"></div>
                <h3 class="text-xs font-black uppercase tracking-[0.2em] opacity-40 truncate">文件树选择 (按需同步)</h3>
              </div>

              <div class="card-modern space-y-4 divide-y divide-black/5 dark:divide-white/5 p-4">
                <!-- 全局配置 -->
                <div class="flex items-center justify-between pb-2">
                  <div class="flex flex-col min-w-0 flex-1 mr-3">
                    <span class="text-sm font-bold truncate">全局配置 (settings.json)</span>
                    <span class="text-[10px] opacity-50 truncate">{{ formatBytes(manifestMap['settings.json']?.size || 0) }}</span>
                  </div>
                  <input type="checkbox" :checked="selectedPaths.has('settings.json')" @change="handleToggleFiles(['settings.json'], !selectedPaths.has('settings.json'))" class="w-5 h-5 accent-blue-500 shrink-0" />
                </div>

                <!-- 文件树组件 -->
                <div class="pt-4">
                  <SyncFileTree
                    :node="fileTree"
                    :depth="0"
                    :selectedPaths="selectedPaths"
                    :getFileStatus="getFileStatus"
                    @toggleFiles="handleToggleFiles"
                  />
                </div>
              </div>
            </div>
          </template>
        </div>

        <!-- 底部同步控制栏 -->
        <div v-if="fileTree" class="absolute bottom-0 left-0 right-0 p-5 border-t border-white/10 bg-secondary-bg/95 backdrop-blur-xl pb-[calc(env(safe-area-inset-bottom,20px)+20px)]">
          <div v-if="isSyncing" class="space-y-3">
            <div class="flex justify-between text-xs font-bold">
              <span>正在同步...</span>
              <span>{{ syncProgress.current }} / {{ syncProgress.total }}</span>
            </div>
            <div class="w-full h-2 bg-white/10 rounded-full overflow-hidden">
              <div class="h-full bg-blue-500 transition-all duration-300 ease-out" :style="{ width: `${syncProgress.total ? (syncProgress.current / syncProgress.total) * 100 : 0}%` }"></div>
            </div>
            <div class="text-[10px] opacity-50 truncate" :title="syncProgress.file">{{ truncatePath(syncProgress.file) }}</div>
          </div>
          <button v-else @click="startSync" :disabled="selectedPaths.size === 0" class="w-full py-4 bg-blue-600 hover:bg-blue-500 disabled:bg-white/10 disabled:text-white/30 text-white active:scale-95 transition-all rounded-[1.25rem] font-black uppercase tracking-widest text-xs shadow-xl shadow-blue-900/20">
            开始同步 (已选 {{ selectedStats.count }} 项)
          </button>

          <!-- 防止同步时误触的透明遮罩 -->
          <div v-if="isSyncing" class="fixed inset-0 bg-transparent"></div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.sync-view {
  background-color: color-mix(in srgb, var(--primary-bg) 85%, transparent);
  backdrop-filter: blur(30px) saturate(180%);
}
.card-modern {
  @apply bg-white/5 border border-white/10 rounded-[2rem] p-5 backdrop-blur-xl shadow-2xl;
}
/* 隐藏复选框默认的不可点击感，并在内部树中保证良好对齐 */
:deep(.sync-tree-node) {
  @apply text-sm;
}
</style>
