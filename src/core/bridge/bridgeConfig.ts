/**
 * 桌面桥接（MobilePluginBridge）统一配置。
 *
 * 背景：远程桌面 / 文坊 / V工程 原先各自在自己页面保存 Base/Token，且默认值彼此不一致。
 * 用户在「设置 → 同步」里把手机连到自己的桌面后，这三个页面仍可能访问旧的硬编码地址，
 * 导致「功能没和远程电脑同步」的错觉。
 *
 * 现在统一为一个配置源（响应式），解析优先级：
 *   1. 用户在本页显式保存的桥接配置（vcp.bridge.settings.v1）
 *   2. 三个旧 key 中已自定义过的配置（兼容老版本；仍是旧默认值的视为未配置）
 *   3. 从「设置 → 同步」的 HTTP 服务 URL + Token 推导（同一台桌面机）
 *   4. 历史默认值（旧公网桥）
 */
import { ref } from 'vue';

export type BridgeConfigSource = 'saved' | 'legacy' | 'sync' | 'default';

export interface BridgeConfig {
  baseUrl: string;
  token: string;
  /** 配置来源，用于设置面板提示 */
  source: BridgeConfigSource;
}

/**
 * 历史默认值：不预置任何私有服务地址/凭据。
 * 新用户需要先配置「设置 → 同步」或在本功能设置里填写桌面桥接 Base/Token。
 */
export const LEGACY_DEFAULT_BASE = 'http://127.0.0.1:5974/api/mobile-bridge';
export const DEFAULT_BRIDGE_TOKEN = '';
/** MobilePluginBridge 在桌面端的固定路径前缀。 */
export const BRIDGE_PATH = '/api/mobile-bridge';
/** 新的统一配置 key。 */
export const BRIDGE_STORAGE_KEY = 'vcp.bridge.settings.v1';
/** 老版本三个功能各自的配置 key（读旧值兼容，写入时同步镜像）。 */
export const LEGACY_STORAGE_KEYS = [
  'vcp.remoteDesktop.settings.v1',
  'vcp.scriptoriumRemote.settings.v1',
  'vcp.projectForge.settings.v1',
] as const;

export const BRIDGE_CONFIG_SOURCE_LABELS: Record<BridgeConfigSource, string> = {
  saved: '自定义（本机已保存）',
  legacy: '沿用旧版本保存的配置',
  sync: '跟随「设置 → 同步」的桌面地址',
  default: '构建默认桥接地址（未检测到同步设置）',
};

type SyncSettingsLike =
  | {
      /** 旧版扁平字段（过渡期兼容） */
      syncHttpUrl?: string;
      syncToken?: string;
      /** 新版 Settings 为域嵌套结构：settings.sync.httpUrl / settings.sync.token */
      sync?: { httpUrl?: string; token?: string };
    }
  | null
  | undefined;

function pickSyncHttpUrl(settings: SyncSettingsLike): string {
  if (!settings) return '';
  return String(settings.sync?.httpUrl ?? settings.syncHttpUrl ?? '').trim();
}

function pickSyncToken(settings: SyncSettingsLike): string {
  if (!settings) return '';
  return String(settings.sync?.token ?? settings.syncToken ?? '').trim();
}

/** 记录最近一次从 App 设置里拿到的同步配置，供「恢复默认」等场景使用。 */
let lastSyncSettings: { syncHttpUrl?: string; syncToken?: string } = {};

function readStored(key: string): { baseUrl: string; token: string } | null {
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return null;
    const parsed = JSON.parse(raw);
    if (parsed && typeof parsed.baseUrl === 'string' && typeof parsed.token === 'string') {
      return { baseUrl: parsed.baseUrl, token: parsed.token };
    }
  } catch {
    // localStorage 不可用或内容损坏时忽略
  }
  return null;
}

/**
 * 把用户输入的地址规整成 MobilePluginBridge 的 Base URL：
 * - 去首尾空白、去末尾斜杠
 * - 没带 /api/mobile-bridge 时自动补上（允许只填 http://桌面:5974）
 */
export function normalizeBridgeBase(raw: unknown): string {
  const trimmed = String(raw ?? '').trim();
  if (!trimmed) return '';
  const withoutHash = trimmed.split('#')[0];
  const stripped = withoutHash.replace(/\/+$/, '');
  if (!stripped) return '';
  if (/\/api\/mobile-bridge$/i.test(stripped)) return stripped;
  return stripped + BRIDGE_PATH;
}

/**
 * 构建期注入的默认桥接地址（可选，供自部署发行版使用）：
 * `VITE_VCP_DEFAULT_BRIDGE_BASE=http://<your-desktop>:5974/api/mobile-bridge`。
 * 未注入时不预置任何地址，开发者需在设置中配置。
 */
const ENV_DEFAULT_BRIDGE_BASE = String(import.meta.env?.VITE_VCP_DEFAULT_BRIDGE_BASE ?? '').trim();

/** 在「没有显式保存、也没有可用同步设置」时使用的最终默认 Base（构建期可注入；默认空）。 */
export const DEFAULT_BRIDGE_BASE = ENV_DEFAULT_BRIDGE_BASE
  ? normalizeBridgeBase(ENV_DEFAULT_BRIDGE_BASE)
  : LEGACY_DEFAULT_BASE;

/** 从「设置 → 同步」的 HTTP 地址推导桥接 Base（同一台桌面机的 5974 端口）。 */
export function deriveBridgeBaseFromSync(syncHttpUrl: unknown): string | null {
  const trimmed = String(syncHttpUrl ?? '').trim();
  if (!trimmed) return null;
  if (!/^https?:\/\//i.test(trimmed)) return null;
  return normalizeBridgeBase(trimmed);
}

/** 旧 key 里的值如果仍等于历史默认值，说明用户从未真正自定义过，视为未配置。 */
function isPristineLegacy(config: { baseUrl: string; token: string }): boolean {
  const base = normalizeBridgeBase(config.baseUrl);
  const token = String(config.token || '').trim();
  return base === LEGACY_DEFAULT_BASE && token === DEFAULT_BRIDGE_TOKEN;
}

function resolveStoredConfig(): BridgeConfig | null {
  const own = readStored(BRIDGE_STORAGE_KEY);
  if (own) {
    return {
      baseUrl: normalizeBridgeBase(own.baseUrl),
      token: String(own.token || '').trim() || DEFAULT_BRIDGE_TOKEN,
      source: 'saved',
    };
  }
  for (const key of LEGACY_STORAGE_KEYS) {
    const legacy = readStored(key);
    if (legacy && !isPristineLegacy(legacy)) {
      return {
        baseUrl: normalizeBridgeBase(legacy.baseUrl),
        token: String(legacy.token || '').trim() || DEFAULT_BRIDGE_TOKEN,
        source: 'legacy',
      };
    }
  }
  return null;
}

/** 解析当前应使用的桥接配置（同步函数，供 store 初始化 / 页面展示用）。 */
export function resolveBridgeConfig(sync?: SyncSettingsLike): BridgeConfig {
  const stored = resolveStoredConfig();
  if (stored) return stored;

  const effectiveSync = sync === undefined ? lastSyncSettings : sync || {};
  const derived = deriveBridgeBaseFromSync(effectiveSync?.syncHttpUrl);
  if (derived) {
    return {
      baseUrl: derived,
      token: String(effectiveSync?.syncToken || '').trim() || DEFAULT_BRIDGE_TOKEN,
      source: 'sync',
    };
  }
  return { baseUrl: DEFAULT_BRIDGE_BASE, token: DEFAULT_BRIDGE_TOKEN, source: 'default' };
}

/** 「恢复默认」用：忽略本机显式保存的配置，回到同步设置推导 / 构建默认值。 */
export function resolveDefaultBridgeConfig(): BridgeConfig {
  const derived = deriveBridgeBaseFromSync(lastSyncSettings.syncHttpUrl);
  if (derived) {
    return {
      baseUrl: derived,
      token: String(lastSyncSettings.syncToken || '').trim() || DEFAULT_BRIDGE_TOKEN,
      source: 'sync',
    };
  }
  return { baseUrl: DEFAULT_BRIDGE_BASE, token: DEFAULT_BRIDGE_TOKEN, source: 'default' };
}

// —— 响应式共享状态：三个功能页面共用同一份配置，任一页面修改，其它页面立即生效 ——
const initialBridgeConfig = resolveBridgeConfig();
export const bridgeBaseUrl = ref(initialBridgeConfig.baseUrl);
export const bridgeToken = ref(initialBridgeConfig.token);
export const bridgeConfigSource = ref<BridgeConfigSource>(initialBridgeConfig.source);

/** 从 localStorage（显式保存/旧 key）+ 最近一次 App 同步设置重新解析并刷新共享状态。 */
export function refreshBridgeConfig(): BridgeConfig {
  const config = resolveBridgeConfig(lastSyncSettings);
  bridgeBaseUrl.value = config.baseUrl;
  bridgeToken.value = config.token;
  bridgeConfigSource.value = config.source;
  return config;
}

/**
 * 在 App 设置加载/更新后调用：同步配置变化时，未显式保存桥接配置的功能会自动跟随。
 */
export function applyBridgeConfigFromAppSettings(settings?: SyncSettingsLike): BridgeConfig {
  lastSyncSettings = {
    syncHttpUrl: pickSyncHttpUrl(settings),
    syncToken: pickSyncToken(settings),
  };
  const config = resolveBridgeConfig(lastSyncSettings);
  bridgeBaseUrl.value = config.baseUrl;
  bridgeToken.value = config.token;
  bridgeConfigSource.value = config.source;
  return config;
}

/** 用户在设置面板显式保存：记为自定义配置，并镜像到旧 key（兼容回退旧版本 App）。 */
export function saveBridgeConfig(next: { baseUrl: string; token: string }): BridgeConfig {
  const rawBase = String(next.baseUrl ?? '').trim();
  const config: BridgeConfig = {
    baseUrl: rawBase ? normalizeBridgeBase(rawBase) : DEFAULT_BRIDGE_BASE,
    token: String(next.token ?? '').trim() || DEFAULT_BRIDGE_TOKEN,
    source: 'saved',
  };
  try {
    const payload = JSON.stringify({ baseUrl: config.baseUrl, token: config.token });
    localStorage.setItem(BRIDGE_STORAGE_KEY, payload);
    for (const key of LEGACY_STORAGE_KEYS) localStorage.setItem(key, payload);
  } catch {
    // ignore
  }
  bridgeBaseUrl.value = config.baseUrl;
  bridgeToken.value = config.token;
  bridgeConfigSource.value = 'saved';
  return config;
}

/** 仅供单元测试重置模块级响应式状态。 */
export function __resetBridgeConfigForTests(): void {
  lastSyncSettings = {};
  bridgeBaseUrl.value = DEFAULT_BRIDGE_BASE;
  bridgeToken.value = DEFAULT_BRIDGE_TOKEN;
  bridgeConfigSource.value = 'default';
}
