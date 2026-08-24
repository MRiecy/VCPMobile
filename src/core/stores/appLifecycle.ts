import { defineStore } from 'pinia';
import { computed, onScopeDispose, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useAssistantStore } from './assistant';
import { useSettingsStore } from './settings';
import { useThemeStore } from './theme';
import { useNotificationStore } from './notification';
import { useChatSessionStore } from './chatSessionStore';
import { useChatHistoryStore } from './chatHistoryStore';
import { useTopicStore } from './topicListManager';
import { updateDistributedState } from '../../features/distributed/composables/useDistributed';
import { useAvatarStore } from './avatar';

export type AppState = 'PERMISSIONS' | 'BOOTING' | 'CONNECTING' | 'PRELOADING' | 'READY' | 'ERROR' | 'OPTIMIZING';

export interface CoreStatus {
  status: 'initializing' | 'ready' | 'error' | 'none';
  message: string;
}

interface DatabaseRecoveryNotice {
  message: string;
  archivePath: string;
  recoveredAt: number;
}



const CONNECT_TIMEOUT_MS = 15000;

export const useAppLifecycleStore = defineStore('appLifecycle', () => {
  const state = ref<AppState>('BOOTING');
  const errorMsg = ref<string | null>(null);
  const isBootstrapping = ref(false);
  const hasBootstrapped = ref(false);
  const currentPhaseLabel = ref('准备启动...');
  const lastTransitionAt = ref<number | null>(null);
  const isBackground = ref(false);



  const assistantStore = useAssistantStore();
  const settingsStore = useSettingsStore();
  const themeStore = useThemeStore();
  const notificationStore = useNotificationStore();
  const avatarStore = useAvatarStore();

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
      case 'OPTIMIZING':
        return '正在优化数据库...';
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

      const sessionStore = useChatSessionStore();
      const topicStore = useTopicStore();

      const promises: Promise<any>[] = [
        settingsStore.fetchSettings(),
        assistantStore.fetchAgentsAndGroups(),
        avatarStore.preloadAll()
      ];

      // 启动预加载：若 Pinia 恢复了活跃会话，提前拉取首屏聊天历史
      // 让 DB + IPC 开销与 Vue 组件挂载并行，ChatView mount 后直接命中缓存（零延迟）
      if (sessionStore.currentSelectedItem?.id && sessionStore.currentTopicId) {
        const historyStore = useChatHistoryStore();
        const ownerId = sessionStore.currentSelectedItem.id;
        const ownerType = sessionStore.currentSelectedItem.type;
        const topicId = sessionStore.currentTopicId;
        if (ownerType === 'agent' || ownerType === 'group') {
          console.log(`[Lifecycle] Preloading chat history for ${ownerType} ${ownerId}, topic: ${topicId}`);
          promises.push(historyStore.preloadHistory(ownerId, ownerType, topicId, 5));
        }
      }

      await Promise.all(promises);
      sessionStore.reconcileCurrentConversation();

      console.log(`[Lifecycle] [Concurrent] DONE Preloading in ${Date.now() - startTime}ms`);
      updatePhaseLabel('核心数据预加载完成');

      hasBootstrapped.value = true;
      isBootstrapping.value = false;
      bootstrapPromise = null;
      setState('READY', '应用就绪');

      // 话题列表延迟到 READY 后异步加载，不阻塞首屏渲染
      // 首屏只需 currentTopicId（Pinia persist 已恢复），话题列表仅侧边栏需要
      if (sessionStore.currentSelectedItem?.id) {
        const ownerId = sessionStore.currentSelectedItem.id;
        const ownerType = sessionStore.currentSelectedItem.type;
        if (ownerType === 'agent' || ownerType === 'group') {
          console.log(`[Lifecycle] Restored session detected, deferring topic list load for ${ownerType} ${ownerId}...`);
          topicStore.loadTopicList(ownerId, ownerType).catch(err => {
            console.error(`[Lifecycle] Deferred topic list load failed:`, err);
          });
        }
      }
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
      let timeoutId: ReturnType<typeof setTimeout> | undefined;
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

          // 数据库物理优化可能超过 15 秒，期间暂停核心就绪超时。
          if (newStatus === 'optimizing') {
            state.value = 'OPTIMIZING';
            currentPhaseLabel.value = notificationStore.vcpCoreStatus.message;
            if (timeoutId) {
              clearTimeout(timeoutId);
              timeoutId = undefined;
            }
          }

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
      const snapshot = await invoke<{
        core: string;
        message: string;
        log: string;
        sync: string;
        distributed: string;
        databaseRecovery?: DatabaseRecoveryNotice | null;
      }>('get_system_snapshot');
      
      // 同步到 Notification Store (唯一真相源)
      notificationStore.updateCoreStatus({
        status: snapshot.core as any,
        message: snapshot.message || (snapshot.core === 'ready' ? '核心引擎已就绪' : '核心引擎初始化中...'),
        source: 'Core'
      });

      if (snapshot.databaseRecovery) {
        notificationStore.addNotification({
          id: `database_recovery_${snapshot.databaseRecovery.recoveredAt}`,
          title: '数据库已进入安全恢复模式',
          message: snapshot.databaseRecovery.message,
          type: 'warning',
          duration: 0,
          rawPayload: {
            type: 'vcp-database-recovery',
            archivePath: snapshot.databaseRecovery.archivePath,
          },
        });
      }

      notificationStore.updateStatus({
        status: snapshot.log as any,
        message: snapshot.log === 'connected' ? '已连接' : '正在连接...',
        source: 'VCPLog'
      });

      // 同步分布式连接状态到专有的 Distributed Composable
      updateDistributedState(snapshot.distributed as any);

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

        // --- 权限门禁：通知、电池优化豁免与通知监听是本产品（常驻中继节点）的保活核心能力，
        // 缺失时先进入引导页；存储等非核心权限保持按需（feature-triggered）申请 ---
        try {
          const pStatus = await invoke<{ notification: boolean; battery: boolean }>('plugin:vcp-mobile|check_all_permissions');
          const listenerRes = await invoke<{ enabled: boolean }>('plugin:vcp-mobile|check_notification_listener_permission');
          if (!pStatus.notification || !pStatus.battery || !listenerRes.enabled) {
            console.log('[Lifecycle] Missing keep-alive critical permissions, entering permission gate');
            // 仅在确认缺失权限时才将状态设为 PERMISSIONS，避免权限完整时引导页一闪而过
            setState('PERMISSIONS', '权限缺失，展示引导界面');
            // 清除 Promise，以便用户在引导页点击“激活并进入应用”时能重新触发
            bootstrapPromise = null;
            isBootstrapping.value = false;
            return;
          }
        } catch (e) {
          // 非 Android host（桌面开发/测试）无系统权限概念，直接放行
          console.warn('[Lifecycle] Permission check unavailable, skipping gate:', e);
        }

        setState('BOOTING', '开始前端主线程启动编排');
        
        // --- 核心优化：先拿快照，再跑流程 ---
        await hydrateSystemStatus();

        // --- 并行优化：主题初始化与核心就绪等待无数据依赖，并行执行 ---
        setState('CONNECTING', '等待后端核心服务就绪');
        await Promise.all([
          themeStore.initTheme(),
          waitForCoreReady()
        ]);
        console.log('[Lifecycle] Theme init + core ready complete');
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
    isBackground,
    coreStatus: computed(() => notificationStore.vcpCoreStatus),
    bootstrap,
    hydrateSystemStatus
  };
});
