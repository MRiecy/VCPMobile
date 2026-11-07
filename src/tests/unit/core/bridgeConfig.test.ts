import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  BRIDGE_STORAGE_KEY,
  DEFAULT_BRIDGE_TOKEN,
  LEGACY_DEFAULT_BASE,
  LEGACY_STORAGE_KEYS,
  __resetBridgeConfigForTests,
  applyBridgeConfigFromAppSettings,
  bridgeBaseUrl,
  bridgeConfigSource,
  bridgeToken,
  deriveBridgeBaseFromSync,
  normalizeBridgeBase,
  resolveBridgeConfig,
  resolveDefaultBridgeConfig,
  saveBridgeConfig,
} from '../../../core/bridge/bridgeConfig';

beforeEach(() => {
  localStorage.clear();
  __resetBridgeConfigForTests();
});

describe('normalizeBridgeBase', () => {
  it('空值返回空串（不再回退任何私有默认地址）', () => {
    expect(normalizeBridgeBase('')).toBe('');
    expect(normalizeBridgeBase('   ')).toBe('');
    expect(normalizeBridgeBase(null)).toBe('');
  });

  it('只填 host:port 时自动补 /api/mobile-bridge', () => {
    expect(normalizeBridgeBase('http://127.0.0.1:5974')).toBe('http://127.0.0.1:5974/api/mobile-bridge');
    expect(normalizeBridgeBase('http://desk.example.com:5974/')).toBe(
      'http://desk.example.com:5974/api/mobile-bridge',
    );
  });

  it('完整 Base 地址去空白与末尾斜杠，保持不变', () => {
    expect(normalizeBridgeBase(' http://desk:5974/api/mobile-bridge/ ')).toBe(
      'http://desk:5974/api/mobile-bridge',
    );
  });
});

describe('deriveBridgeBaseFromSync', () => {
  it('从同步 HTTP 地址推导', () => {
    expect(deriveBridgeBaseFromSync('http://desk.example.com:5974')).toBe(
      'http://desk.example.com:5974/api/mobile-bridge',
    );
  });

  it('空值或非法协议返回 null', () => {
    expect(deriveBridgeBaseFromSync('')).toBeNull();
    expect(deriveBridgeBaseFromSync('desk:5974')).toBeNull();
    expect(deriveBridgeBaseFromSync(undefined)).toBeNull();
  });
});

describe('resolveBridgeConfig', () => {
  it('无任何配置时回退历史默认公网桥', () => {
    const config = resolveBridgeConfig();
    expect(config).toEqual({
      baseUrl: LEGACY_DEFAULT_BASE,
      token: DEFAULT_BRIDGE_TOKEN,
      source: 'default',
    });
  });

  it('没有显式/旧配置时，从同步设置推导（无影云场景）', () => {
    const config = resolveBridgeConfig({
      syncHttpUrl: 'http://desk.example.com:5974',
      syncToken: 'token-sync',
    });
    expect(config.baseUrl).toBe('http://desk.example.com:5974/api/mobile-bridge');
    expect(config.token).toBe('token-sync');
    expect(config.source).toBe('sync');
  });

  it('显式保存的新 key 优先级最高', () => {
    localStorage.setItem(
      BRIDGE_STORAGE_KEY,
      JSON.stringify({ baseUrl: 'http://saved:5974/api/mobile-bridge', token: 'saved-token' }),
    );
    const config = resolveBridgeConfig({ syncHttpUrl: 'http://sync:5974', syncToken: 'sync-token' });
    expect(config.source).toBe('saved');
    expect(config.baseUrl).toBe('http://saved:5974/api/mobile-bridge');
    expect(config.token).toBe('saved-token');
  });

  it('旧 key 自定义过时沿用旧配置（优先于同步推导）', () => {
    localStorage.setItem(
      'vcp.remoteDesktop.settings.v1',
      JSON.stringify({ baseUrl: 'http://legacy:5974', token: 'legacy-token' }),
    );
    const config = resolveBridgeConfig({ syncHttpUrl: 'http://sync:5974', syncToken: 'sync-token' });
    expect(config.source).toBe('legacy');
    expect(config.baseUrl).toBe('http://legacy:5974/api/mobile-bridge');
    expect(config.token).toBe('legacy-token');
  });

  it('旧 key 仍等于历史默认值时视为未配置', () => {
    localStorage.setItem(
      'vcp.remoteDesktop.settings.v1',
      JSON.stringify({ baseUrl: LEGACY_DEFAULT_BASE, token: DEFAULT_BRIDGE_TOKEN }),
    );
    const config = resolveBridgeConfig({ syncHttpUrl: 'http://sync:5974', syncToken: 'sync-token' });
    expect(config.source).toBe('sync');
    expect(config.baseUrl).toBe('http://sync:5974/api/mobile-bridge');
  });
});

describe('saveBridgeConfig / applyBridgeConfigFromAppSettings', () => {
  it('保存后更新共享状态、写入新 key 并镜像旧 key', () => {
    const saved = saveBridgeConfig({ baseUrl: 'http://127.0.0.1:8080/', token: '  ' });
    expect(saved.baseUrl).toBe('http://127.0.0.1:8080/api/mobile-bridge');
    expect(saved.token).toBe(DEFAULT_BRIDGE_TOKEN);
    expect(saved.source).toBe('saved');
    expect(bridgeBaseUrl.value).toBe('http://127.0.0.1:8080/api/mobile-bridge');
    expect(bridgeToken.value).toBe(DEFAULT_BRIDGE_TOKEN);
    expect(bridgeConfigSource.value).toBe('saved');

    expect(JSON.parse(localStorage.getItem(BRIDGE_STORAGE_KEY)!)).toEqual({
      baseUrl: 'http://127.0.0.1:8080/api/mobile-bridge',
      token: DEFAULT_BRIDGE_TOKEN,
    });
    for (const key of LEGACY_STORAGE_KEYS) {
      expect(JSON.parse(localStorage.getItem(key)!)).toEqual({
        baseUrl: 'http://127.0.0.1:8080/api/mobile-bridge',
        token: DEFAULT_BRIDGE_TOKEN,
      });
    }
  });

  it('同步设置变化时共享状态跟随，但已显式保存的配置不被覆盖', () => {
    applyBridgeConfigFromAppSettings({ syncHttpUrl: 'http://sync:5974', syncToken: 'sync-token' });
    expect(bridgeBaseUrl.value).toBe('http://sync:5974/api/mobile-bridge');
    expect(bridgeConfigSource.value).toBe('sync');

    saveBridgeConfig({ baseUrl: 'http://custom:5974', token: 'custom' });
    applyBridgeConfigFromAppSettings({ syncHttpUrl: 'http://sync2:5974', syncToken: 'sync2' });
    expect(bridgeBaseUrl.value).toBe('http://custom:5974/api/mobile-bridge');
    expect(bridgeConfigSource.value).toBe('saved');
  });

  it('恢复默认忽略显式保存，回到同步推导', () => {
    saveBridgeConfig({ baseUrl: 'http://custom:5974', token: 'custom' });
    applyBridgeConfigFromAppSettings({ syncHttpUrl: 'http://sync:5974', syncToken: 'sync-token' });
    const fallback = resolveDefaultBridgeConfig();
    expect(fallback.baseUrl).toBe('http://sync:5974/api/mobile-bridge');
    expect(fallback.token).toBe('sync-token');
    expect(fallback.source).toBe('sync');
  });
});

describe('构建期默认地址注入', () => {
  it('VITE_VCP_DEFAULT_BRIDGE_BASE 注入后作为“无配置时”的默认 Base', async () => {
    vi.stubEnv('VITE_VCP_DEFAULT_BRIDGE_BASE', 'http://desk.example.com:5974');
    vi.resetModules();
    try {
      const mod = await import('../../../core/bridge/bridgeConfig');
      expect(mod.DEFAULT_BRIDGE_BASE).toBe(
        'http://desk.example.com:5974/api/mobile-bridge',
      );
      const config = mod.resolveBridgeConfig();
      expect(config.baseUrl).toBe('http://desk.example.com:5974/api/mobile-bridge');
      expect(config.token).toBe(DEFAULT_BRIDGE_TOKEN);
      expect(config.source).toBe('default');
    } finally {
      vi.unstubAllEnvs();
      vi.resetModules();
    }
  });

  it('未注入时默认地址仍为历史公网桥（普通构建行为不变）', async () => {
    vi.stubEnv('VITE_VCP_DEFAULT_BRIDGE_BASE', '');
    vi.resetModules();
    try {
      const mod = await import('../../../core/bridge/bridgeConfig');
      expect(mod.DEFAULT_BRIDGE_BASE).toBe(LEGACY_DEFAULT_BASE);
    } finally {
      vi.unstubAllEnvs();
      vi.resetModules();
    }
  });
});
