import { defineStore } from 'pinia';
import { computed, onScopeDispose, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useAssistantStore } from './assistant';
import { useSettingsStore } from './settings';
import { useThemeStore } from './theme';
import { useNotificationStore, type VcpStatus } from './notification';
import { useChatSessionStore } from './chatSessionStore';
import { useChatHistoryStore } from './chatHistoryStore';
import { useTopicStore } from './topicListManager';
import { useAvatarStore } from './avatar';
import { isAndroid, isIOS } from '../utils/platform';
import { isNotificationAuthorized, readNotificationPermission } from '../../features/notification/nativeNotifications';
import { selectLatestLifecycleSnapshot, type LifecycleSnapshotDto } from '../types/lifecycle';
import { hasMissingStartupPermission, type PermissionStatusDto } from '../types/native';
import { bootMark, reportBootTrace, trackBootStage } from '../utils/bootTrace';
import { useNotificationProcessor } from '../composables/useNotificationProcessor';
import { useVoiceDocumentStore } from '../../features/chat/voice/voiceDocument';

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

interface SystemSnapshotDto {
  lifecycle: LifecycleSnapshotDto | null;
  core: 'initializing' | 'optimizing' | 'ready' | 'error';
  message: string;
  log: Extract<VcpStatus['status'], 'closed' | 'connecting' | 'connected' | 'error'>;
  logConfigurationError: string | null;
  sync: string;
  databaseRecovery: DatabaseRecoveryNotice | null;
  backgroundServiceFailures: Array<'guardian' | 'helper' | 'events'>;
}

const LOG_STATUS_MESSAGES: Record<SystemSnapshotDto['log'], string> = {
  closed: '未连接',
  connecting: '正在连接...',
  connected: '已连接',
  error: '连接错误',
};


const CONNECT_TIMEOUT_MS = 15000;

export const useAppLifecycleStore = defineStore('appLifecycle', () => {
  const state = ref<AppState>('BOOTING');
  const errorMsg = ref<string | null>(null);
  const isBootstrapping = ref(false);
  const hasBootstrapped = ref(false);
  const currentPhaseLabel = ref('准备启动...');
  const lastTransitionAt = ref<number | null>(null);
  const lifecycleSnapshot = ref<LifecycleSnapshotDto | null>(null);
  const isBackground = computed(() => lifecycleSnapshot.value?.phase === 'background');
  const applyLifecycleSnapshot = (next: LifecycleSnapshotDto | null) => {
    lifecycleSnapshot.value = selectLatestLifecycleSnapshot(lifecycleSnapshot.value, next);
  };



  const assistantStore = useAssistantStore();
  const settingsStore = useSettingsStore();
  const themeStore = useThemeStore();
  const notificationStore = useNotificationStore();
  const { processPayload } = useNotificationProcessor();
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
        return '正在维护数据库';
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




  // 全局监听同步状态变化 (移除自动触发神经同步逻辑)
  const unwatchVcpStatus = watch(
    () => notificationStore.vcpStatus.status,
    async (newStatus, oldStatus) => {
      if (newStatus === 'connected' && oldStatus !== 'connected' && state.value === 'READY') {
        // 仅在连接成功时重新拉取数据快照，但不触发耗时的 Manifest 全量同步
        await assistantStore.fetchAgentsAndGroups();
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

      const promises: Promise<unknown>[] = [
        trackBootStage('fe:preload_settings', settingsStore.fetchSettings()),
        trackBootStage('fe:preload_agents', assistantStore.fetchAgentsAndGroups()),
        trackBootStage('fe:preload_avatars', avatarStore.preloadMetadata())
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
          promises.push(trackBootStage('fe:preload_history', historyStore.preloadHistory(ownerId, ownerType, topicId, 5)));
        }
      }

      await Promise.all(promises);
      bootMark('fe:preload_done');
      sessionStore.reconcileCurrentConversation();

      console.log(`[Lifecycle] [Concurrent] DONE Preloading in ${Date.now() - startTime}ms`);
      updatePhaseLabel('核心数据预加载完成');

      hasBootstrapped.value = true;
      isBootstrapping.value = false;
      bootstrapPromise = null;
      setState('READY', '应用就绪');
      bootMark('fe:ready');
      void reportBootTrace();

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
      // Keep the binding initialized before watch's immediate callback can use cleanup.
      // eslint-disable-next-line prefer-const
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

          // 数据库故障核查和旧库物理升级可能超过普通启动的等待期限。
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

  let logStatusRead = 0;
  const hydrateSystemStatus = async () => {
    const read = ++logStatusRead;
    const previousLogStatus = notificationStore.vcpStatus;
    try {
      console.log('[Lifecycle] Fetching system status snapshot...');
      const snapshot = await invoke<SystemSnapshotDto>('get_system_snapshot');
      applyLifecycleSnapshot(snapshot.lifecycle);

      for (const service of snapshot.backgroundServiceFailures) {
        notificationStore.addNotification(processPayload({ type: 'vcp-background-service-error', service }));
      }
      
      // 同步到 Notification Store (唯一真相源)
      notificationStore.updateCoreStatus({
        status: snapshot.core,
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

      if (snapshot.logConfigurationError) {
        notificationStore.addNotification(processPayload({
          type: 'vcp-log-message',
          data: {
            id: 'vcp_connection_configuration_error',
            status: 'error',
            tool_name: 'VCPLog / VCPInfo 配置错误',
            content: `连接已停止：${snapshot.logConfigurationError} 请在设置中修正 WebSocket 地址或 Key。`,
            source: 'VCPLog / VCPInfo',
          },
        }));
      }

      if (read === logStatusRead && notificationStore.vcpStatus === previousLogStatus) {
        notificationStore.updateStatus({
          status: snapshot.log,
          message: snapshot.log === 'closed' && snapshot.logConfigurationError
            ? `连接已停止：${snapshot.logConfigurationError}`
            : LOG_STATUS_MESSAGES[snapshot.log],
          source: 'VCPLog'
        });
      }

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
        bootMark('fe:bootstrap_begin');
        isBootstrapping.value = true;
        errorMsg.value = null;
        hasBootstrapped.value = false;

        // Revoke the previous document's capture and independent playback even
        // if startup stops at the permission guide. Playback release has its
        // own admission barrier; the cached voice connection does not await it.
        void useVoiceDocumentStore().connect().catch(error => {
          console.warn('[Lifecycle] Voice document connection failed:', error);
        });

        if (isAndroid) {
          try {
            const pStatus = await invoke<PermissionStatusDto>('plugin:vcp-mobile|check_all_permissions');
            bootMark('fe:permissions_done');
            if (hasMissingStartupPermission(pStatus)) {
              console.log('[Lifecycle] Missing keep-alive critical permissions, entering permission gate');
              // 仅在确认缺失权限时才将状态设为 PERMISSIONS，避免权限完整时引导页一闪而过
              setState('PERMISSIONS', '权限缺失，展示引导界面');
              // 清除 Promise，以便用户在引导页点击“激活并进入应用”时能重新触发
              bootstrapPromise = null;
              isBootstrapping.value = false;
              return;
            }
          } catch (e) {
            // 权限检查异常时仍允许前台使用，以常驻通知说明后台能力尚未确认。
            console.warn('[Lifecycle] Permission check unavailable, continuing with warning:', e);
            notificationStore.addNotification({
              id: 'startup_permission_check_failed',
              title: '启动权限检查未完成',
              message: '已继续进入应用，可保持前台使用。后台流式回复和连接保活可能受影响，请检查系统中的通知权限、通知访问权限与电池后台设置。',
              type: 'warning',
              duration: 0,
            });
          }
        }

        if (isIOS) {
          let notificationsAllowed = false;
          try {
            notificationsAllowed = isNotificationAuthorized(await readNotificationPermission());
          } catch (cause) {
            console.warn('[Lifecycle] Could not verify notification permission:', cause);
          }
          if (!notificationsAllowed) {
            setState('PERMISSIONS', '需要开启通知才能进入应用');
            bootstrapPromise = null;
            isBootstrapping.value = false;
            return;
          }
        }

        setState('BOOTING', '开始前端主线程启动编排');
        
        // --- 核心优化：先拿快照，再跑流程 ---
        await hydrateSystemStatus();
        bootMark('fe:hydrate_done');

        // --- 并行优化：主题初始化与核心就绪等待无数据依赖，并行执行 ---
        setState('CONNECTING', '等待后端核心服务就绪');
        await Promise.all([
          themeStore.initTheme(),
          waitForCoreReady()
        ]);
        bootMark('fe:core_ready');
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
    lifecycleSnapshot,
    applyLifecycleSnapshot,
    coreStatus: computed(() => notificationStore.vcpCoreStatus),
    bootstrap,
    continueFromPermissionGuide: () => bootstrap(true),
    hydrateSystemStatus
  };
});
