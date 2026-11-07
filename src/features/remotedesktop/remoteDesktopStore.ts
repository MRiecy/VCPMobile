import { computed, ref } from 'vue';
import { defineStore } from 'pinia';
import {
  bridgeBaseUrl,
  bridgeConfigSource,
  bridgeToken,
  refreshBridgeConfig,
  saveBridgeConfig,
} from '../../core/bridge/bridgeConfig';

export interface RemotePage {
  id: string;
  title: string;
  url: string;
}

/** 远程画面显示参数：JPEG 画质 + 缩放最大宽度（0 = 原图）。 */
export interface RemoteDisplaySettings {
  quality: number;
  maxWidth: number;
}

export const DEFAULT_REMOTE_DISPLAY: RemoteDisplaySettings = { quality: 45, maxWidth: 960 };

const DISPLAY_STORAGE_KEY = 'vcp.remoteDesktop.display.v1';

export function clampDisplayQuality(value: unknown): number {
  const n = Number(value);
  if (!Number.isFinite(n) || n <= 0) return DEFAULT_REMOTE_DISPLAY.quality;
  return Math.min(Math.max(Math.round(n), 20), 90);
}

export function clampDisplayMaxWidth(value: unknown): number {
  const n = Number(value);
  if (!Number.isFinite(n) || n <= 0) return 0;
  return Math.min(Math.round(n), 4096);
}

export function loadRemoteDisplaySettings(): RemoteDisplaySettings {
  try {
    const raw = localStorage.getItem(DISPLAY_STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw);
      return {
        quality: clampDisplayQuality(parsed?.quality),
        maxWidth: clampDisplayMaxWidth(parsed?.maxWidth),
      };
    }
  } catch {
    // localStorage 不可用或内容损坏时用默认值
  }
  return { ...DEFAULT_REMOTE_DISPLAY };
}

function persistRemoteDisplaySettings(settings: RemoteDisplaySettings): void {
  try {
    localStorage.setItem(DISPLAY_STORAGE_KEY, JSON.stringify(settings));
  } catch {
    // ignore
  }
}

/**
 * 远程桌面：手机端只负责显示帧与转发输入，界面与执行全部发生在桌面端 VCPChat。
 * 通过 MobilePluginBridge 的 CDP 代理（/pages、/screen、/input）工作。
 */
export const useRemoteDesktopStore = defineStore('remoteDesktop', () => {
  refreshBridgeConfig();
  // 桥接地址/Token 统一由 core/bridge/bridgeConfig 管理：
  // 默认跟随「设置 → 同步」的桌面地址，用户也可在本页设置面板里显式覆盖。
  const baseUrl = computed(() => bridgeBaseUrl.value);
  const token = computed(() => bridgeToken.value);
  const configSource = computed(() => bridgeConfigSource.value);
  const pages = ref<RemotePage[]>([]);
  const activePageId = ref('');
  const frame = ref('');
  const frameWidth = ref(0);
  const frameHeight = ref(0);
  const error = ref('');
  const lastFrameAt = ref(0);
  const latencyMs = ref(0);
  const displaySettings = ref<RemoteDisplaySettings>(loadRemoteDisplaySettings());

  /** 当前帧 JPEG 字节数（由 dataUrl 长度估算），用于设置面板展示带宽代价。 */
  const frameBytes = computed(() => {
    const url = frame.value;
    if (!url) return 0;
    const comma = url.indexOf(',');
    const base64 = comma >= 0 ? url.slice(comma + 1) : url;
    return Math.round((base64.length * 3) / 4);
  });

  const activePage = computed(() => pages.value.find((page) => page.id === activePageId.value) || null);

  async function call<T>(path: string, body?: unknown): Promise<T> {
    const response = await fetch(`${baseUrl.value}${path}`, {
      method: body === undefined ? 'GET' : 'POST',
      headers: {
        Authorization: `Bearer ${token.value}`,
        ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await response.text();
    let payload: any = null;
    try {
      payload = text ? JSON.parse(text) : null;
    } catch {
      throw new Error(`Bridge returned non-JSON (${response.status})`);
    }
    if (!response.ok || payload?.ok === false) {
      const message = payload?.error?.message || `Bridge error ${response.status}`;
      throw new Error(message);
    }
    return payload as T;
  }

  async function refreshPages(): Promise<void> {
    const data = await call<{ pages: RemotePage[] }>('/pages');
    pages.value = data.pages || [];
    if (!activePageId.value || !pages.value.some((page) => page.id === activePageId.value)) {
      activePageId.value = pages.value[0]?.id || '';
    }
  }

  async function capture(): Promise<void> {
    if (!activePageId.value) return;
    const startedAt = Date.now();
    const data = await call<{
      dataUrl: string;
      width: number | null;
      height: number | null;
    }>('/screen', {
      pageId: activePageId.value,
      quality: displaySettings.value.quality,
      maxWidth: displaySettings.value.maxWidth,
    });
    frame.value = data.dataUrl;
    frameWidth.value = data.width || frameWidth.value;
    frameHeight.value = data.height || frameHeight.value;
    lastFrameAt.value = Date.now();
    latencyMs.value = Date.now() - startedAt;
  }

  async function tap(nx: number, ny: number): Promise<void> {
    await call('/input', { pageId: activePageId.value, type: 'tap', nx, ny });
  }

  async function scroll(nx: number, ny: number, deltaY: number): Promise<void> {
    await call('/input', { pageId: activePageId.value, type: 'scroll', nx, ny, deltaY });
  }

  async function typeText(text: string): Promise<void> {
    if (!text) return;
    await call('/input', { pageId: activePageId.value, type: 'text', text });
  }

  async function sendKey(key: string): Promise<void> {
    await call('/input', { pageId: activePageId.value, type: 'key', key });
  }

  function setError(message: string): void {
    error.value = message;
  }

  function clearError(): void {
    error.value = '';
  }

  /** 更新画质/分辨率（立即生效，下一次抓帧使用）。 */
  function updateDisplaySettings(next: Partial<RemoteDisplaySettings>): RemoteDisplaySettings {
    const merged: RemoteDisplaySettings = {
      quality: clampDisplayQuality(next.quality ?? displaySettings.value.quality),
      maxWidth: clampDisplayMaxWidth(next.maxWidth ?? displaySettings.value.maxWidth),
    };
    displaySettings.value = merged;
    persistRemoteDisplaySettings(merged);
    return merged;
  }

  function updateSettings(next: { baseUrl: string; token: string }): void {
    saveBridgeConfig(next);
    // 端点变了，旧的画面/页面列表不再可信
    pages.value = [];
    activePageId.value = '';
    frame.value = '';
    frameWidth.value = 0;
    frameHeight.value = 0;
    lastFrameAt.value = 0;
    latencyMs.value = 0;
    error.value = '';
  }

  return {
    baseUrl,
    token,
    configSource,
    displaySettings,
    frameBytes,
    updateDisplaySettings,
    pages,
    activePageId,
    activePage,
    frame,
    frameWidth,
    frameHeight,
    error,
    lastFrameAt,
    latencyMs,
    refreshPages,
    capture,
    tap,
    scroll,
    typeText,
    sendKey,
    setError,
    clearError,
    updateSettings,
  };
});
