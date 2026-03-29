import { defineStore } from 'pinia';
import { ref } from 'vue';

export interface VcpNotification {
  id: string;
  type: 'info' | 'success' | 'warning' | 'error' | 'tool' | 'agent';
  title: string;
  message: string;
  timestamp: number;
  duration?: number; // 毫秒, 0 为永不消失
  isPreformatted?: boolean;
  actions?: { label: string; value: boolean; color: string }[];
  silent?: boolean;
  read?: boolean;
  rawPayload?: any; // 用于保存原始数据，方便处理 action
}

export interface VcpStatus {
  status: 'open' | 'closed' | 'error' | 'connecting' | 'connected' | 'disconnected';
  message: string;
  source: string;
}

export const useNotificationStore = defineStore('notification', () => {
  const historyList = ref<VcpNotification[]>([]);
  const activeToasts = ref<VcpNotification[]>([]);
  const unreadCount = ref(0);
  const isDrawerOpen = ref(false);

  const vcpStatus = ref<VcpStatus>({
    status: 'connecting',
    message: '等待初始化...',
    source: 'VCPLog'
  });

  const updateStatus = (payload: VcpStatus) => {
    vcpStatus.value = payload;
  };

  const addNotification = (payload: Partial<VcpNotification>) => {
    if (payload.silent) return;

    const id = Math.random().toString(36).substring(2, 9);
    const timestamp = Date.now();
    const notification: VcpNotification = {
      id,
      timestamp,
      read: false,
      title: payload.title || 'VCP Notification',
      message: payload.message || '',
      type: payload.type || 'info',
      ...payload
    } as VcpNotification;

    // 1. 入历史列表（置顶）
    historyList.value.unshift(notification);
    if (historyList.value.length > 100) historyList.value.pop();
    
    unreadCount.value++;

    // 2. 推入活动气泡 (抽屉打开时抑制 Toast)
    if (!isDrawerOpen.value) {
      activeToasts.value.push(notification);
      
      // 3. 自动移除逻辑 (如果 duration 为 0 则不自动消失)
      if (notification.duration !== 0) {
        setTimeout(() => {
          activeToasts.value = activeToasts.value.filter(t => t.id !== id);
        }, notification.duration || 3000);
      }
    }
  };

  const clearHistory = () => {
    historyList.value = [];
    unreadCount.value = 0;
  };

  const markAllRead = () => {
    historyList.value.forEach(n => n.read = true);
    unreadCount.value = 0;
  };

  /**
   * 执行通知动作（如：审批）
   * 将业务逻辑从 UI 组件下沉到 Store，确保状态一致性
   */
  const executeAction = async (notificationId: string, action: { label: string; value: any }) => {
    const item = historyList.value.find(n => n.id === notificationId);
    if (!item) return;

    if (item.type === 'warning' && item.rawPayload?.type === 'tool_approval_request') {
      const response = {
        type: 'tool_approval_response',
        data: {
          requestId: item.rawPayload.data.requestId,
          approved: action.value
        }
      };
      
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        // 通过 vcp_log_service 接口回传
        await invoke('send_vcp_log_message', { payload: response });
        
        // 处理后 UI 反馈：清空按钮并从 Toast 移除
        item.actions = [];
        item.message = `[已处理] 操作: ${action.label}`;
        activeToasts.value = activeToasts.value.filter(t => t.id !== item.id);
      } catch (e) {
        console.error('[NotificationStore] Action failed:', e);
      }
    }
  };

  // 幽灵 Toast 清理机制 (每 30s 检查一次)
  setInterval(() => {
    const now = Date.now();
    activeToasts.value = activeToasts.value.filter(toast => {
      // duration === 0 为审批类通知，不应被清理
      if (toast.duration === 0) return true;
      const duration = toast.duration || 3000;
      return now - toast.timestamp < duration + 5000; // 冗余 5s 后强制清理
    });
  }, 30000);

  return {
    historyList,
    activeToasts,
    unreadCount,
    isDrawerOpen,
    vcpStatus,
    updateStatus,
    addNotification,
    clearHistory,
    markAllRead,
    executeAction
  };
});
