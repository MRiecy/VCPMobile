import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import {
  DEFAULT_REMOTE_DISPLAY,
  clampDisplayMaxWidth,
  clampDisplayQuality,
  loadRemoteDisplaySettings,
  useRemoteDesktopStore,
} from '../../../features/remotedesktop/remoteDesktopStore';
import { __resetBridgeConfigForTests } from '../../../core/bridge/bridgeConfig';

const fetchMock = vi.fn();

function jsonResponse(body: unknown, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    text: async () => JSON.stringify(body),
  } as unknown as Response;
}

function ok(body: Record<string, unknown>) {
  return jsonResponse({ ok: true, ...body });
}

function lastBody(): any {
  const call = fetchMock.mock.calls[fetchMock.mock.calls.length - 1];
  return JSON.parse(String((call?.[1] as RequestInit | undefined)?.body || '{}'));
}

beforeEach(() => {
  localStorage.clear();
  __resetBridgeConfigForTests();
  setActivePinia(createPinia());
  fetchMock.mockReset();
  vi.stubGlobal('fetch', fetchMock);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('远程桌面显示参数', () => {
  it('默认画质 45 / 最大宽度 960', () => {
    expect(DEFAULT_REMOTE_DISPLAY).toEqual({ quality: 45, maxWidth: 960 });
    expect(loadRemoteDisplaySettings()).toEqual({ quality: 45, maxWidth: 960 });
    const store = useRemoteDesktopStore();
    expect(store.displaySettings).toEqual({ quality: 45, maxWidth: 960 });
  });

  it('updateDisplaySettings 会夹紧范围并持久化', () => {
    const store = useRemoteDesktopStore();
    store.updateDisplaySettings({ quality: 200, maxWidth: -5 });
    expect(store.displaySettings).toEqual({ quality: 90, maxWidth: 0 });
    expect(JSON.parse(localStorage.getItem('vcp.remoteDesktop.display.v1')!)).toEqual({
      quality: 90,
      maxWidth: 0,
    });

    store.updateDisplaySettings({ quality: 30, maxWidth: 720 });
    expect(JSON.parse(localStorage.getItem('vcp.remoteDesktop.display.v1')!)).toEqual({
      quality: 30,
      maxWidth: 720,
    });
  });

  it('clamp 辅助函数处理非法值', () => {
    expect(clampDisplayQuality('abc')).toBe(45);
    expect(clampDisplayQuality(0)).toBe(45);
    expect(clampDisplayQuality(12)).toBe(20);
    expect(clampDisplayQuality(100)).toBe(90);
    expect(clampDisplayMaxWidth('x')).toBe(0);
    expect(clampDisplayMaxWidth(-1)).toBe(0);
    expect(clampDisplayMaxWidth(99999)).toBe(4096);
  });

  it('/screen 请求带上 quality 与 maxWidth，并解析缩放后的尺寸与字节数', async () => {
    fetchMock.mockImplementation(async (url: string) => {
      if (String(url).endsWith('/pages')) {
        return ok({ pages: [{ id: 'p1', title: 'VCPChat', url: 'file:///x' }] });
      }
      return ok({ pageId: 'p1', width: 960, height: 591, dataUrl: 'data:image/jpeg;base64,' + 'A'.repeat(4000) });
    });
    const store = useRemoteDesktopStore();
    await store.refreshPages();
    store.updateDisplaySettings({ quality: 30, maxWidth: 720 });

    await store.capture();
    expect(lastBody()).toMatchObject({ pageId: 'p1', quality: 30, maxWidth: 720 });
    expect(store.frameWidth).toBe(960);
    expect(store.frameHeight).toBe(591);
    expect(store.frameBytes).toBe(3000);
  });
});
