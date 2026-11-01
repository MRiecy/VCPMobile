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
  let metadataRead = 0;
  let statusRead = 0;
  let liveMessageCount = 0;

  // 从后端初始化拉取历史 metadata
  const fetchMetadataList = async () => {
    const read = ++metadataRead;
    const countAtStart = liveMessageCount;
    try {
      const list = await invoke<unknown[]>('get_vcp_info_metadata_list');
      if (read !== metadataRead) return;
      const history = list.map(readMetadata).filter((item): item is VcpInfoMetadata => item !== null);
      const ids = new Set(history.map(item => item.id));
      // Live events may precede or follow the native snapshot while IPC is in flight.
      const newer = metadataList.value.slice(0, liveMessageCount - countAtStart).filter(item => !ids.has(item.id));
      metadataList.value = [...newer, ...history].slice(0, 500);
    } catch (err) {
      console.error('[RagObserverStore] Failed to fetch metadata list:', err);
    }
  };

  // 从后端获取当前连接状态
  const fetchConnectionStatus = async () => {
    const read = ++statusRead;
    try {
      const status = await invoke<ConnectionStatus>('get_vcp_info_connection_status');
      if (read === statusRead) connectionStatus.value = status;
    } catch (err) {
      console.error('[RagObserverStore] Failed to fetch connection status:', err);
    }
  };

  const refresh = async () => {
    if (unlistenFn) await Promise.all([fetchConnectionStatus(), fetchMetadataList()]);
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
      if (unlistenFn) await fetchMetadataList();
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

  const destroyListener = () => {
    ++listenerSessionId;
    ++metadataRead;
    ++statusRead;
    if (unlistenFn) {
      unlistenFn();
      unlistenFn = null;
    }
  };

  // 先订阅再读快照，让监听注册期间后台收到的消息也进入列表。
  const initListener = async () => {
    destroyListener();
    const currentSessionId = listenerSessionId;
    const stop = await listen<unknown>('vcp-info-event', (event) => {
      if (currentSessionId !== listenerSessionId) return;
      const payload = event.payload;
      if (!isRecord(payload)) return;

      const type = payload.type;
      
      if (type === 'vcp-info-status') {
        if (
          payload.source === 'VCPInfo' &&
          typeof payload.status === 'string' &&
          CONNECTION_STATUSES.has(payload.status as ConnectionStatus)
        ) {
          ++statusRead;
          connectionStatus.value = payload.status as ConnectionStatus;
        }
      } else if (type === 'vcp-info-message') {
        const metadata = readMetadata(payload.data);
        if (metadata && !metadataList.value.some(item => item.id === metadata.id)) {
          ++liveMessageCount;
          metadataList.value.unshift(metadata);
          if (metadataList.value.length > 500) {
            metadataList.value.pop();
          }
          triggerAnimation();
        }
      } else if (type === 'vcp-info-clear') {
        ++metadataRead;
        metadataList.value = [];
      }
    });
    if (currentSessionId !== listenerSessionId) {
      stop();
      return;
    }
    unlistenFn = stop;
    await refresh();
  };

  return {
    connectionStatus,
    metadataList,
    triggerSpectrumAnimation,
    refresh,
    fetchPayload,
    clearAll,
    initListener,
    destroyListener,
  };
});
