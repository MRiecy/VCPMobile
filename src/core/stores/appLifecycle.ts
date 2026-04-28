import { defineStore } from 'pinia';
import { computed, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useAssistantStore } from './assistant';
import { useSettingsStore } from './settings';
import { useThemeStore } from './theme';
import { useNotificationStore } from './notification';

export type AppState = 'BOOTING' | 'CONNECTING' | 'PRELOADING' | 'INITIAL_SYNCING' | 'READY' | 'ERROR';

export interface CoreStatus {
  status: 'initializing' | 'ready' | 'error' | 'none';
  message: string;
}

type PreloadTask = {
  label: string;
  run: () => Promise<void>;
};

const CONNECT_TIMEOUT_MS = 15000;

export const useAppLifecycleStore = defineStore('appLifecycle', () => {
  const state = ref<AppState>('BOOTING');
  const errorMsg = ref<string | null>(null);
  const isBootstrapping = ref(false);
  const hasBootstrapped = ref(false);
  const currentPhaseLabel = ref('准备启动...');
  const lastTransitionAt = ref<number | null>(null);

  const assistantStore = useAssistantStore();
  const settingsStore = useSettingsStore();
  const themeStore = useThemeStore();
  const notificationStore = useNotificationStore();

  let bootstrapPromise: Promise<void> | null = null;
  let coreReadyUnlisten: (() => void) | null = null;
  let connectTimeoutId: ReturnType<typeof setTimeout> | null = null;

  const statusText = computed(() => {
    switch (state.value) {
      case 'BOOTING':
        return '正在初始化界面资源...';
      case 'CONNECTING':
        return '正在连接核心服务...';
      case 'PRELOADING':
        return currentPhaseLabel.value || '正在预加载核心数据...';
      case 'ERROR':
        return errorMsg.value || '启动失败';
      case 'READY':
      default:
        return '应用已就绪';
    }
  });

  const setState = (nextState: AppState, reason: string) => {
    state.value = nextState;
    lastTransitionAt.value = Date.now();
    console.log(`[Lifecycle] -> ${nextState} | ${reason}`);
  };

  const updatePhaseLabel = (label: string) => {
    currentPhaseLabel.value = label;
    console.log(`[Lifecycle] ${label}`);
  };

  const cleanupConnectionWaiters = () => {
    if (connectTimeoutId) {
      clearTimeout(connectTimeoutId);
      connectTimeoutId = null;
    }

    if (coreReadyUnlisten) {
      coreReadyUnlisten();
      coreReadyUnlisten = null;
    }
  };

  const fail = (message: string) => {
    cleanupConnectionWaiters();
    errorMsg.value = message;
    isBootstrapping.value = false;
    bootstrapPromise = null;
    setState('ERROR', message);
    // 统一更新通知系统的核心状态槽
    notificationStore.updateCoreStatus({ 
      status: 'error', 
      message, 
      source: 'Core' 
    });
    console.error('[Lifecycle] FATAL:', message);
  };

  const runSequentialTask = async (task: PreloadTask) => {
    updatePhaseLabel(`预加载 ${task.label}...`);
    console.log(`[Lifecycle] [Sequential] START ${task.label}`);
    try {
      await task.run();
      console.log(`[Lifecycle] [Sequential] DONE ${task.label}`);
    } catch (error) {
      console.error(`[Lifecycle] [Sequential] FAILED ${task.label}:`, error);
      throw error;
    }
  };

  const runParallelTasks = async (tasks: PreloadTask[]) => {
    if (!tasks.length) return;

    const labels = tasks.map(task => task.label).join(' / ');
    updatePhaseLabel(`并发预加载 ${labels}...`);
    console.log(`[Lifecycle] [Parallel] START ${labels}`);

    // 为并发任务添加硬超时保护 (20秒)
    const PRELOAD_TIMEOUT = 20000;
    
    try {
      await Promise.race([
        Promise.all(tasks.map(async (task) => {
          console.log(`[Lifecycle] [Parallel] -> ${task.label} (Starting)`);
          const startTime = Date.now();
          try {
            await task.run();
            const duration = Date.now() - startTime;
            console.log(`[Lifecycle] [Parallel] <- ${task.label} (Success in ${duration}ms)`);
          } catch (error) {
            const duration = Date.now() - startTime;
            console.error(`[Lifecycle] [Parallel] !! ${task.label} (Failed after ${duration}ms):`, error);
            throw error;
          }
        })),
        new Promise((_, reject) => 
          setTimeout(() => reject(new Error(`并发预加载任务超时 (${labels})`)), PRELOAD_TIMEOUT)
        )
      ]);
      console.log('[Lifecycle] [Parallel] ALL DONE');
    } catch (error) {
      console.error('[Lifecycle] [Parallel] One or more tasks failed or timed out');
      throw error;
    }
  };

  const checkAndTriggerInitialSync = async () => {
    // 如果已经在同步中则忽略
    if (state.value === 'INITIAL_SYNCING') return;

    // 简单以 agent + group 的数量为判断标准，全新安装通常 < 10
    const isSparse = assistantStore.combinedItems.length < 10;
    const isSyncConnected = notificationStore.vcpStatus.status === 'connected';

    if (isSparse && isSyncConnected) {
      setState('INITIAL_SYNCING', '检测到本地数据稀疏且同步已连接，触发神经同步...');
      
      // 等待同步完成事件，或者超时 (60秒)
      await new Promise<void>(async (resolve) => {
        let eventUnlisten: (() => void) | null = null;
        
        const timeoutId = setTimeout(() => {
          console.warn('[Lifecycle] Initial sync wait timeout, proceeding anyway');
          cleanup();
          resolve();
        }, 60000);

        const cleanup = () => {
          clearTimeout(timeoutId);
          unwatch();
          if (eventUnlisten) eventUnlisten();
        };

        const unwatch = watch(
          () => notificationStore.vcpStatus.message,
          (msg) => {
            if (msg.includes('同步任务已全部完成')) {
              console.log('[Lifecycle] Initial sync detected completed via message');
              cleanup();
              resolve();
            }
          }
        );
        
        // 同时也监听原生的完成事件
        const { listen } = await import('@tauri-apps/api/event');
        eventUnlisten = await listen('vcp-sync-completed', () => {
          console.log('[Lifecycle] Initial sync detected via event');
          cleanup();
          resolve();
        });
      });
      
      // 同步完成后重新拉取一次数据
      await assistantStore.fetchAgents();
      await assistantStore.fetchGroups();

      setState('READY', '神经同步完成，恢复就绪状态');
    }
  };

  // 全局监听同步状态变化，动态触发初始同步（针对重装后用户去设置填了信息才连上的情况）
  watch(
    () => notificationStore.vcpStatus.status,
    async (newStatus, oldStatus) => {
      if (newStatus === 'connected' && oldStatus !== 'connected' && state.value === 'READY') {
        // 先确保本地数据是最新的，然后再判断是否少于10
        await assistantStore.fetchAgents();
        await assistantStore.fetchGroups();
        await checkAndTriggerInitialSync();
      }
    }
  );

  const startPreloading = async () => {
    if (state.value === 'PRELOADING' || state.value === 'READY') {
      console.log(`[Lifecycle] Skip preloading in state: ${state.value}`);
      return;
    }

    cleanupConnectionWaiters();
    setState('PRELOADING', '开始预加载核心业务数据');

    const settingsTask: PreloadTask = {
      label: 'Settings',
      run: async () => {
        await settingsStore.fetchSettings();
      }
    };

    const assistantParallelTasks: PreloadTask[] = [
      {
        label: 'Agents',
        run: async () => {
          await assistantStore.fetchAgents();
        }
      },
      {
        label: 'Groups',
        run: async () => {
          await assistantStore.fetchGroups();
        }
      }
    ];

    try {
      await runSequentialTask(settingsTask);
      await runParallelTasks(assistantParallelTasks);

      updatePhaseLabel('核心数据预加载完成');

      // --- 首次大数据量同步判定 (Neural Sync 触发点) ---
      await checkAndTriggerInitialSync();

      hasBootstrapped.value = true;
      isBootstrapping.value = false;
      bootstrapPromise = null;
      setState('READY', '应用就绪');
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      fail(`预加载失败: ${message}`);
      throw error;
    }
  };

  const waitForCoreReady = async () => {
    updatePhaseLabel('检查核心服务状态...');
    
    // 1. 同步一次当前状态
    const currentStatus = await invoke<string>('get_core_status');
    console.log(`[Lifecycle] Initial core status -> ${currentStatus}`);

    if (currentStatus === 'ready') {
      notificationStore.updateCoreStatus({ status: 'ready', message: '核心引擎已就绪', source: 'Core' });
      return;
    }

    if (currentStatus === 'error') {
      const lastError = await invoke<string | null>('get_last_error');
      const msg = lastError || '核心服务在初始化阶段发生崩溃';
      notificationStore.updateCoreStatus({ status: 'error', message: msg, source: 'Core' });
      throw new Error(msg);
    }

    // 2. 等待状态变为 ready (由 useNotificationProcessor 触发)
    updatePhaseLabel('等待核心就绪...');

    await new Promise<void>((resolve, reject) => {
      // 仅作为极端挂死的兜底
      const timeoutId = setTimeout(() => {
        unwatch();
        reject(new Error(`等待核心引擎就绪超时（${CONNECT_TIMEOUT_MS}ms）`));
      }, CONNECT_TIMEOUT_MS);

      const unwatch = watch(
        () => notificationStore.vcpCoreStatus.status,
        (newStatus) => {
          if (newStatus === 'ready') {
            clearTimeout(timeoutId);
            unwatch();
            resolve();
          } else if (newStatus === 'error') {
            clearTimeout(timeoutId);
            unwatch();
            reject(new Error(notificationStore.vcpCoreStatus.message || '核心引擎启动失败'));
          }
        },
        { immediate: true }
      );
    });
  };

  const hydrateSystemStatus = async () => {
    try {
      console.log('[Lifecycle] Fetching system status snapshot...');
      const snapshot = await invoke<{ core: string; log: string; sync: string }>('get_system_snapshot');
      
      // 同步到 Notification Store (唯一真相源)
      notificationStore.updateCoreStatus({
        status: snapshot.core as any,
        message: snapshot.core === 'ready' ? '核心引擎已就绪' : '核心引擎初始化中...',
        source: 'Core'
      });

      notificationStore.updateStatus({
        status: snapshot.log as any,
        message: snapshot.log === 'open' ? '已连接' : '正在连接...',
        source: 'VCPLog'
      });

      // 还需要一个 updateSyncStatus? 目前暂用 updateStatus 逻辑手动适配
      // 实际上我们可以给 notificationStore 加一个更通用的 updateIndicator(source, data)
      // 但为了快速上线，我们先手动调一下
      notificationStore.updateStatus({
        status: snapshot.sync as any,
        message: snapshot.sync === 'open' ? '已开启同步' : '同步未绪',
        source: 'Sync'
      });

      console.log('[Lifecycle] Snapshot hydrated:', snapshot);
    } catch (e) {
      console.error('[Lifecycle] Failed to hydrate status snapshot:', e);
    }
  };

  const bootstrap = async () => {
    if (bootstrapPromise) {
      console.log('[Lifecycle] Reusing existing bootstrap promise');
      return bootstrapPromise;
    }

    bootstrapPromise = (async () => {
      try {
        isBootstrapping.value = true;
        errorMsg.value = null;
        hasBootstrapped.value = false;

        setState('BOOTING', '开始前端主线程启动编排');
        
        // --- 核心优化：先拿快照，再跑流程 ---
        await hydrateSystemStatus();

        updatePhaseLabel('初始化主题资源...');
        await themeStore.initTheme();
        console.log('[Lifecycle] Theme initialization complete');

        setState('CONNECTING', '等待后端核心服务就绪');
        await waitForCoreReady();
        await startPreloading();
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        fail(message);
        throw error;
      }
    })();

    return bootstrapPromise;
  };

  return {
    state,
    errorMsg,
    statusText,
    currentPhaseLabel,
    isBootstrapping,
    hasBootstrapped,
    lastTransitionAt,
    coreStatus: computed(() => notificationStore.vcpCoreStatus),
    bootstrap
  };
});
