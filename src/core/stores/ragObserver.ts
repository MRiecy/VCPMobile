import { defineStore } from 'pinia';
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export interface VcpInfoMetadata {
  id: string;
  type: string;
  title: string;
  subtitle: string | null;
  summary: string;
  timestamp: string;
  hasDetails: boolean;
}

export type ConnectionStatus = 'closed' | 'connecting' | 'connected' | 'error';

const CONNECTION_STATUSES = new Set<ConnectionStatus>([
  'closed',
  'connecting',
  'connected',
  'error',
]);

const isRecord = (value: unknown): value is Record<string, unknown> =>
  Boolean(value && typeof value === 'object' && !Array.isArray(value));

const readMetadata = (value: unknown): VcpInfoMetadata | null => {
  if (!isRecord(value)) return null;
  if (
    typeof value.id !== 'string' ||
    typeof value.type !== 'string' ||
    typeof value.title !== 'string' ||
    (value.subtitle !== null && typeof value.subtitle !== 'string') ||
    typeof value.summary !== 'string' ||
    typeof value.timestamp !== 'string' ||
    typeof value.hasDetails !== 'boolean'
  ) {
    return null;
  }
  return value as unknown as VcpInfoMetadata;
};

export const useRagObserverStore = defineStore('ragObserver', () => {
  const connectionStatus = ref<ConnectionStatus>('closed');
  const metadataList = ref<VcpInfoMetadata[]>([]);
  const triggerSpectrumAnimation = ref(false);
  
  let unlistenFn: UnlistenFn | null = null;
  let listenerSessionId = 0;

  // 从后端初始化拉取历史 metadata
  const fetchMetadataList = async () => {
    try {
      const list = await invoke<unknown[]>('get_vcp_info_metadata_list');
      metadataList.value = list.map(readMetadata).filter((item): item is VcpInfoMetadata => item !== null);
    } catch (err) {
      console.error('[RagObserverStore] Failed to fetch metadata list:', err);
    }
  };

  // 从后端获取当前连接状态
  const fetchConnectionStatus = async () => {
    try {
      const status = await invoke<ConnectionStatus>('get_vcp_info_connection_status');
      connectionStatus.value = status;
    } catch (err) {
      console.error('[RagObserverStore] Failed to fetch connection status:', err);
    }
  };

  // 根据 id 按需拉取 Payload 详情
  const fetchPayload = async (id: string): Promise<unknown> => {
    try {
      const rawJson = await invoke<string>('get_vcp_info_payload', { id });
      return JSON.parse(rawJson) as unknown;
    } catch (err) {
      console.error(`[RagObserverStore] Failed to fetch payload for ${id}:`, err);
      throw err;
    }
  };

  // 清空所有历史数据
  const clearAll = async () => {
    try {
      await invoke<void>('clear_vcp_info');
      metadataList.value = [];
    } catch (err) {
      console.error('[RagObserverStore] Failed to clear vcp info:', err);
    }
  };

  // 触发一次频谱微动画
  const triggerAnimation = () => {
    triggerSpectrumAnimation.value = true;
    setTimeout(() => {
      triggerSpectrumAnimation.value = false;
    }, 1500); // 持续 1.5 秒
  };

  // 监听 Tauri 后端推送的系统事件
  const initListener = async () => {
    const currentSessionId = ++listenerSessionId;

    if (unlistenFn) {
      unlistenFn();
      unlistenFn = null;
    }

    await fetchConnectionStatus();
    await fetchMetadataList();

    // 检查竞态锁：如果在 await 期间有新的 initListener 调用，放弃当前注册
    if (currentSessionId !== listenerSessionId) {
      return;
    }

    unlistenFn = await listen<unknown>('vcp-info-event', (event) => {
      const payload = event.payload;
      if (!isRecord(payload)) return;

      const type = payload.type;
      
      if (type === 'vcp-info-status') {
        if (
          payload.source === 'VCPInfo' &&
          typeof payload.status === 'string' &&
          CONNECTION_STATUSES.has(payload.status as ConnectionStatus)
        ) {
          connectionStatus.value = payload.status as ConnectionStatus;
        }
      } else if (type === 'vcp-info-message') {
        const metadata = readMetadata(payload.data);
        if (metadata) {
          metadataList.value.unshift(metadata);
          if (metadataList.value.length > 500) {
            metadataList.value.pop();
          }
          triggerAnimation();
        }
      } else if (type === 'vcp-info-clear') {
        metadataList.value = [];
      }
    });
  };

  const destroyListener = () => {
    if (unlistenFn) {
      unlistenFn();
      unlistenFn = null;
    }
  };

  return {
    connectionStatus,
    metadataList,
    triggerSpectrumAnimation,
    fetchMetadataList,
    fetchConnectionStatus,
    fetchPayload,
    clearAll,
    initListener,
    destroyListener,
  };
});
