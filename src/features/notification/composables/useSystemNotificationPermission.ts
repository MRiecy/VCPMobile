import { computed, onMounted, onUnmounted, ref, shallowRef, watch } from 'vue';
import { useAppLifecycleStore } from '../../../core/stores/appLifecycle';
import { isIOS } from '../../../core/utils/platform';
import {
  isNotificationAuthorized,
  openNotificationSettings,
  readNotificationPermission,
  requestNotificationPermission,
  type NotificationPermission,
} from '../nativeNotifications';

/** A view's latest native observation; never persisted or used as a platform capability. */
export function useSystemNotificationPermission() {
  const lifecycle = useAppLifecycleStore();
  const permission = shallowRef<NotificationPermission | null>(null);
  const busy = ref(false);
  const acting = ref(false);
  const openingSettings = ref(false);
  const error = ref('');
  let pending: Promise<void> | null = null;
  let disposed = false;
  let requesting = false;
  let refreshAfterPending = false;

  const allowed = computed(() => permission.value !== null && isNotificationAuthorized(permission.value));
  const actionBusy = computed(() => openingSettings.value || (busy.value && (acting.value || permission.value === null)));
  const description = computed(() => {
    if (error.value) return error.value;
    switch (permission.value?.authorization) {
      case 'notDetermined': return '可选择开启系统通知；不启用也能正常聊天。';
      case 'denied': return '系统通知已关闭，可前往系统设置开启。';
      case 'authorized': return '已允许通知，显示方式由系统设置决定。';
      case 'provisional': return '已允许安静通知，提醒方式可在系统设置调整。';
      case 'ephemeral': return '已取得临时通知授权，显示方式由系统设置决定。';
      case 'unknown': return '暂时无法确认通知授权，请重试读取。';
      default: return '正在读取系统通知设置…';
    }
  });
  const actionLabel = computed(() => {
    if (actionBusy.value) return '处理中…';
    if (error.value || !permission.value || permission.value.authorization === 'unknown') return '重新读取';
    return permission.value.authorization === 'notDetermined' ? '开启通知' : '系统设置';
  });

  function observe(request: () => Promise<NotificationPermission>, isRequest = false): Promise<void> {
    if (disposed) return Promise.resolve();
    if (pending) return pending;
    busy.value = true;
    requesting = isRequest;
    error.value = '';
    pending = request().then(result => {
      if (!disposed) permission.value = result;
    }).catch(cause => {
      console.warn('[NotificationPermission] Native observation failed:', cause);
      if (!disposed) error.value = isRequest
        ? '通知授权未完成，可重试或稍后在系统设置调整。聊天不受影响。'
        : '暂时无法读取系统通知设置，请重试。聊天不受影响。';
    }).finally(() => {
      pending = null;
      if (!disposed) busy.value = false;
      if (!disposed && refreshAfterPending) {
        refreshAfterPending = false;
        void refresh();
      }
    });
    return pending;
  }

  const refresh = () => {
    // Returning from Settings can overlap an older getSettings snapshot. Keep
    // one trailing read; an authorization request already reads after completion.
    if (pending && !requesting) refreshAfterPending = true;
    return observe(readNotificationPermission);
  };
  const request = () => observe(requestNotificationPermission, true);
  const openSettings = async () => {
    if (disposed || openingSettings.value || busy.value) return;
    openingSettings.value = true;
    error.value = '';
    try {
      await openNotificationSettings();
    } catch (cause) {
      console.warn('[NotificationPermission] Settings could not open:', cause);
      if (!disposed) error.value = '未能打开系统设置，请稍后重试。';
    } finally {
      if (!disposed) openingSettings.value = false;
    }
  };

  const act = async () => {
    if (disposed || busy.value || openingSettings.value) return;
    acting.value = true;
    try {
      if (error.value || !permission.value || permission.value.authorization === 'unknown') {
        await refresh();
      } else if (permission.value.authorization === 'notDetermined') {
        await request();
      } else {
        await openSettings();
      }
    } finally {
      acting.value = false;
    }
  };

  const onVisible = () => {
    if (!document.hidden) void refresh();
  };
  watch(() => lifecycle.lifecycleSnapshot?.phase, (phase, previous) => {
    if (isIOS && phase === 'active' && previous !== 'active') void refresh();
  });
  onMounted(() => {
    if (!isIOS) return;
    document.addEventListener('visibilitychange', onVisible);
    void refresh();
  });
  onUnmounted(() => {
    disposed = true;
    document.removeEventListener('visibilitychange', onVisible);
  });

  return { permission, allowed, busy, actionBusy, openingSettings, error, description, actionLabel, refresh, act };
}
