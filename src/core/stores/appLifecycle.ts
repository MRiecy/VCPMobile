import { defineStore } from 'pinia';
import { computed, onScopeDispose, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useAssistantStore } from './assistant';
import { useSettingsStore } from './settings';
import { useThemeStore } from './theme';
import { useNotificationStore } from './notification';

export type AppState = 'PERMISSIONS' | 'BOOTING' | 'CONNECTING' | 'PRELOADING' | 'READY' | 'ERROR';

export interface CoreStatus {
  status: 'initializing' | 'ready' | 'error' | 'none';
  message: string;
}



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
      case 'PERMISSIONS':
        return '正在检查系统权限...';
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




  // 全局监听同步状态变化 (移除自动触发神经同步逻辑)
  const unwatchVcpStatus = watch(
    () => notificationStore.vcpStatus.status,
    async (newStatus, oldStatus) => {
      if (newStatus === 'connected' && oldStatus !== 'connected' && state.value === 'READY') {
        // 仅在连接成功时重新拉取数据快照，但不触发耗时的 Manifest 全量同步
        await assistantStore.fetchAgents();
        await assistantStore.fetchGroups();
      }
    }
  );

  onScopeDispose(() => {
    unwatchVcpStatus();
  });

  const startPreloading = async () => {
    if (state.value === 'PRELOADING' || state.value === 'READY') {
      console.log(`[Lifecycle] Skip preloading in state: ${state.value}`);
      return;
    }

    cleanupConnectionWaiters();
    setState('PRELOADING', '开始预加载核心业务数据');
    const startTime = Date.now();

    try {
      updatePhaseLabel('正在并发预加载配置与助手数据...');
      console.log('[Lifecycle] [Concurrent] START Preloading Settings and AgentsAndGroups');

      await Promise.all([
        settingsStore.fetchSettings(),
        assistantStore.fetchAgentsAndGroups()
      ]);

      console.log(`[Lifecycle] [Concurrent] DONE Preloading in ${Date.now() - startTime}ms`);
      updatePhaseLabel('核心数据预加载完成');

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
    
    // 核心优化：直接读取第一步在 hydrateSystemStatus 中拉回并写入真相源的状态，免去重复 IPC 检测
    const currentStatus = notificationStore.vcpCoreStatus.status;
    console.log(`[Lifecycle] Checked core status from snapshot -> ${currentStatus}`);

    if (currentStatus === 'ready') {
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
      let settled = false;
      let timeoutId: ReturnType<typeof setTimeout>;
      let unwatch: (() => void);

      const cleanup = () => {
        clearTimeout(timeoutId);
        unwatch();
      };

      // 仅作为极端挂死的兜底
      timeoutId = setTimeout(() => {
        if (!settled) {
          settled = true;
          cleanup();
          reject(new Error(`等待核心引擎就绪超时（${CONNECT_TIMEOUT_MS}ms）`));
        }
      }, CONNECT_TIMEOUT_MS);

      unwatch = watch(
        () => notificationStore.vcpCoreStatus.status,
        (newStatus) => {
          if (settled) return;
          if (newStatus === 'ready') {
            settled = true;
            cleanup();
            resolve();
          } else if (newStatus === 'error') {
            settled = true;
            cleanup();
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

      // 同步状态不再渲染到全局状态栏（同步已改为完全手动触发）

      console.log('[Lifecycle] Snapshot hydrated:', JSON.stringify(snapshot));
    } catch (e) {
      console.error('[Lifecycle] Failed to hydrate status snapshot:', e);
    }
  };

  const bootstrap = async (force = false) => {
    if (isBootstrapping.value && force) {
      console.log('[Lifecycle] Reusing existing bootstrap promise (force-in-progress ignored)');
      return bootstrapPromise;
    }

    if (bootstrapPromise && !force) {
      console.log('[Lifecycle] Reusing existing bootstrap promise');
      return bootstrapPromise;
    }

    bootstrapPromise = (async () => {
      try {
        isBootstrapping.value = true;
        errorMsg.value = null;
        hasBootstrapped.value = false;

        setState('PERMISSIONS', '检查系统权限完整性');
        const pStatus = await invoke<{ notification: boolean; storage: boolean; battery: boolean }>('plugin:vcp-mobile|check_all_permissions');
        if (!pStatus.notification || !pStatus.storage || !pStatus.battery) {
          console.log('[Lifecycle] Missing permissions, waiting for user action');
          // 清除 Promise，以便下次点击“进入应用”时能重新触发
          bootstrapPromise = null;
          isBootstrapping.value = false;
          return;
        }

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
    bootstrap,
    hydrateSystemStatus
  };
});
