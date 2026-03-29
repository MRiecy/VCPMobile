import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useAssistantStore } from './assistant';
import { useSettingsStore } from './settings';
import { useThemeStore } from './theme';

export type AppState = 'BOOTING' | 'CONNECTING' | 'PRELOADING' | 'READY' | 'ERROR';

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
    console.error('[Lifecycle] FATAL:', message);
  };

  const runSequentialTask = async (task: PreloadTask) => {
    updatePhaseLabel(`预加载 ${task.label}...`);
    console.log(`[Lifecycle] [Sequential] START ${task.label}`);
    await task.run();
    console.log(`[Lifecycle] [Sequential] DONE ${task.label}`);
  };

  const runParallelTasks = async (tasks: PreloadTask[]) => {
    if (!tasks.length) return;

    updatePhaseLabel(`并发预加载 ${tasks.map(task => task.label).join(' / ')}...`);
    console.log(`[Lifecycle] [Parallel] START ${tasks.map(task => task.label).join(', ')}`);

    await Promise.all(tasks.map(async (task) => {
      console.log(`[Lifecycle] [Parallel] -> ${task.label}`);
      await task.run();
      console.log(`[Lifecycle] [Parallel] <- ${task.label}`);
    }));

    console.log('[Lifecycle] [Parallel] DONE');
  };

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
      hasBootstrapped.value = true;
      isBootstrapping.value = false;
      bootstrapPromise = null;
      setState('READY', '核心数据已完成预加载');
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      fail(`预加载失败: ${message}`);
      throw error;
    }
  };

  const waitForCoreReady = async () => {
    updatePhaseLabel('检查核心服务状态...');
    const currentStatus = await invoke<string>('get_core_status');
    console.log(`[Lifecycle] get_core_status -> ${currentStatus}`);

    if (currentStatus === 'ready') {
      console.log('[Lifecycle] Backend already ready, skip event wait.');
      return;
    }

    if (currentStatus === 'error') {
      const lastError = await invoke<string | null>('get_last_error');
      throw new Error(lastError || '后端在初始化阶段返回错误状态');
    }

    updatePhaseLabel('等待 vcp-core-ready 事件...');

    await new Promise<void>(async (resolve, reject) => {
      let settled = false;

      const settleResolve = () => {
        if (settled) return;
        settled = true;
        cleanupConnectionWaiters();
        resolve();
      };

      const settleReject = (error: Error) => {
        if (settled) return;
        settled = true;
        cleanupConnectionWaiters();
        reject(error);
      };

      coreReadyUnlisten = await listen('vcp-core-ready', () => {
        console.log('[Lifecycle] Received vcp-core-ready event');
        settleResolve();
      });

      connectTimeoutId = setTimeout(async () => {
        if (settled) return;

        try {
          console.warn('[Lifecycle] Wait for core ready timed out, checking status again...');
          const status = await invoke<string>('get_core_status');
          console.log(`[Lifecycle] timeout get_core_status -> ${status}`);

          if (status === 'ready') {
            settleResolve();
            return;
          }

          if (status === 'error') {
            const lastError = await invoke<string | null>('get_last_error');
            settleReject(new Error(lastError || '核心服务启动失败'));
            return;
          }

          settleReject(new Error(`等待核心服务就绪超时（${CONNECT_TIMEOUT_MS}ms）`));
        } catch (error) {
          settleReject(error instanceof Error ? error : new Error(String(error)));
        }
      }, CONNECT_TIMEOUT_MS);
    });
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
    bootstrap
  };
});
