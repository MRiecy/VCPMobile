import { defineStore } from 'pinia';
import { ref, computed } from 'vue';
import { invoke, Channel } from '@tauri-apps/api/core';
import { useNotificationStore } from './notification';

export interface ModelInfo {
  id: string;
  object: string;
  created: number;
  owned_by: string;
}

export interface TestResult {
  status: 'idle' | 'testing' | 'success' | 'failed';
  latency?: number;
  error?: string;
}

interface ModelTestProgressDto {
  modelId: string;
  status: 'testing' | 'success' | 'failed' | 'completed';
  latency: number | null;
  error: string | null;
}

export const useModelStore = defineStore('model', () => {
  // --- State ---
  const models = ref<ModelInfo[]>([]);
  const hotModels = ref<string[]>([]);
  const favorites = ref<string[]>([]);
  const isLoading = ref(false);
  const lastRefreshed = ref(0);
  const testResults = ref<Record<string, TestResult>>({});
  
  const notificationStore = useNotificationStore();

  // --- Getters ---
  const sortedModels = computed(() => {
    // 排序优先级：收藏 > 热门 > 其他
    return [...models.value].sort((a, b) => {
      const aFav = favorites.value.includes(a.id) ? 1 : 0;
      const bFav = favorites.value.includes(b.id) ? 1 : 0;
      if (aFav !== bFav) return bFav - aFav;

      const aHot = hotModels.value.indexOf(a.id);
      const bHot = hotModels.value.indexOf(b.id);
      if (aHot !== -1 && bHot !== -1) return aHot - bHot;
      if (aHot !== -1) return -1;
      if (bHot !== -1) return 1;

      return a.id.localeCompare(b.id);
    });
  });

  const isFavorite = computed(() => (modelId: string) => favorites.value.includes(modelId));

  // --- Actions ---
  // 提取公共的过期缓存清理函数
  const cleanupOldTestResults = () => {
    const activeModelIds = new Set(models.value.map(m => m.id));
    for (const modelId in testResults.value) {
      if (!activeModelIds.has(modelId)) {
        delete testResults.value[modelId];
      }
    }
  };

  // 提取公共的静默后台同步函数
  const triggerSilentSync = async () => {
    try {
      const freshModels = await invoke<ModelInfo[]>('refresh_models');
      models.value = freshModels;
      lastRefreshed.value = Date.now();
      await Promise.all([fetchHotModels(), fetchFavorites()]);
      cleanupOldTestResults();
      console.log(`[SWR/Self-Healing] Silent sync completed. Total models: ${freshModels.length}`);
    } catch (error) {
      console.warn('[SWR/Self-Healing] Silent sync failed:', error);
    }
  };

  const fetchModels = async (force = false) => {
    // 1. 如果没有任何内存模型，优先极速加载本地 SQLite 数据库缓存，让 UI 开屏瞬间呈现！
    if (models.value.length === 0) {
      try {
        models.value = await invoke<ModelInfo[]>('get_cached_models');
        await Promise.all([fetchHotModels(), fetchFavorites()]);
      } catch (e) {
        console.error('Failed to load cached models:', e);
      }
    }

    // 2. 判定是否需要触发同步：
    // - 手动强制同步 (force = true)
    // - 或者本地完全没有任何模型
    // - 或者距离上次网络同步已经超过了 5 分钟 (SWR 智能同步周期)
    const shouldSync = force || models.value.length === 0 || (Date.now() - lastRefreshed.value > 1000 * 60 * 5);

    if (!shouldSync) {
      return;
    }

    // 3. 执行同步逻辑
    if (force) {
      // 强制手动同步：显示转圈 loading，成功后弹出 Toast 提示
      if (isLoading.value) return;
      const startTime = Date.now();
      isLoading.value = true;
      try {
        models.value = await invoke<ModelInfo[]>('refresh_models');
        lastRefreshed.value = Date.now();
        await Promise.all([fetchHotModels(), fetchFavorites()]);
        cleanupOldTestResults();

        notificationStore.addNotification({
          type: 'success',
          title: '模型同步成功',
          message: `已成功同步最新模型列表，共 ${models.value.length} 个可用模型`,
          toastOnly: true,
        });
      } catch (error: any) {
        console.error('Failed to force sync models:', error);
        notificationStore.addNotification({
          type: 'error',
          title: '模型同步失败',
          message: error?.toString() || '请检查网络连接或 API 配置',
          toastOnly: true,
        });
      } finally {
        const elapsed = Date.now() - startTime;
        const minDuration = 800;
        if (elapsed < minDuration) {
          await new Promise((resolve) => setTimeout(resolve, minDuration - elapsed));
        }
        isLoading.value = false;
      }
    } else {
      // SWR 静默同步：在后台静默拉取，不展示 loading，不弹窗，UI 零开销，用户无感知
      triggerSilentSync();
    }
  };

  const fetchHotModels = async () => {
    try {
      hotModels.value = await invoke<string[]>('get_hot_models', { limit: 10 });
    } catch (error) {
      console.error('Failed to fetch hot models:', error);
    }
  };

  const fetchFavorites = async () => {
    try {
      favorites.value = await invoke<string[]>('get_favorite_models');
    } catch (error) {
      console.error('Failed to fetch favorite models:', error);
    }
  };

  const toggleFavorite = async (modelId: string) => {
    try {
      const isFav = await invoke<boolean>('toggle_favorite_model', { modelId });
      if (isFav) {
        if (!favorites.value.includes(modelId)) favorites.value.push(modelId);
      } else {
        favorites.value = favorites.value.filter(id => id !== modelId);
      }
    } catch (error) {
      console.error('Failed to toggle favorite:', error);
    }
  };

  const recordUsage = async (modelId: string) => {
    try {
      await invoke('record_model_usage', { modelId });
      // 更新本地热门列表（可选，或者等待下次 fetch）
      fetchHotModels();
    } catch (error) {
      console.error('Failed to record usage:', error);
    }
  };

  const isTestingAll = ref(false);
  const activeSessionId = ref(0); // 仅用于单模型测试的快速轻量级版本隔离

  const stopTestAll = async () => {
    isTestingAll.value = false;
    activeSessionId.value++; // 自增单模型会话版本，使任何未完成的单模型测试失效
    
    // 1. 物理级硬中断：通知 Rust 后端瞬间强行杀死正在运行的所有批量测试网络进程并释放连接
    try {
      await invoke('stop_all_model_tests');
    } catch (e) {
      console.error('[ModelStore] Failed to stop model tests on backend:', e);
    }

    // 2. 将所有处于测试中的模型彻底从前端结果表中剔除（UI 瞬间恢复 Zap 图标，绝不残留 0.0s 脏数据）
    for (const modelId in testResults.value) {
      if (testResults.value[modelId]?.status === 'testing') {
        delete testResults.value[modelId];
      }
    }
  };

  const testModel = async (modelId: string) => {
    const sessionId = activeSessionId.value; // 捕获当前会话版本
    testResults.value[modelId] = { status: 'testing' };
    try {
      const latency = await invoke<number>('test_model_connectivity', { modelId });
      // 仅在会话版本未发生改变时写回结果，防止后台连接越界写入已关闭的面板
      if (activeSessionId.value === sessionId) {
        testResults.value[modelId] = {
          status: 'success',
          latency,
        };
      } else {
        delete testResults.value[modelId]; // 被丢弃的请求彻底从哈希表中擦除
      }
    } catch (error: any) {
      console.error(`Failed to test connectivity for ${modelId}:`, error);
      const errStr = error?.toString() || '连接失败';
      if (activeSessionId.value === sessionId) {
        testResults.value[modelId] = {
          status: 'failed',
          error: errStr,
        };

        // 🚨 核心自愈逻辑：若返回 404 (代表模型在远端已下架或删除)
        if (errStr.includes("404")) {
          models.value = models.value.filter(m => m.id !== modelId);
          triggerSilentSync();
        }
      } else {
        delete testResults.value[modelId];
      }
    }
  };

  const testAllModels = async (modelIds: string[]) => {
    if (isTestingAll.value) return; // 防重入门禁

    const targets = modelIds.filter(
      id => !testResults.value[id] || testResults.value[id].status !== 'testing'
    );
    if (targets.length === 0) return;

    isTestingAll.value = true;

    try {
      // 1. 创建流式推送 Channel
      const progressChannel = new Channel<ModelTestProgressDto>();
      progressChannel.onmessage = (progress) => {
        const { modelId, status, latency, error } = progress;

        if (status === 'completed') {
          isTestingAll.value = false;
          return;
        }

        if (status === 'testing') {
          testResults.value[modelId] = { status: 'testing' };
        } else if (status === 'success') {
          testResults.value[modelId] = {
            status: 'success',
            latency: latency ?? undefined,
          };
        } else if (status === 'failed') {
          testResults.value[modelId] = {
            status: 'failed',
            error: error || '连接失败',
          };

          // 🚨 核心自愈逻辑：若返回 404 错误
          if (error?.includes("404")) {
            models.value = models.value.filter(m => m.id !== modelId);
            triggerSilentSync();
          }
        }
      };

      // 2. 调用后端异步任务分发，将繁重并发测试全量托管给 Rust 高性能执行
      await invoke('start_batch_model_test', {
        modelIds: targets,
        progressChannel,
      });
    } catch (e) {
      console.error('[ModelStore] Failed to start batch model test:', e);
      isTestingAll.value = false;
    }
  };

  return {
    models,
    hotModels,
    favorites,
    isLoading,
    lastRefreshed,
    testResults,
    isTestingAll,
    sortedModels,
    isFavorite,
    fetchModels,
    fetchHotModels,
    fetchFavorites,
    toggleFavorite,
    recordUsage,
    testModel,
    testAllModels,
    stopTestAll,
  };
});
