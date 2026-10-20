import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useLogCenterStore } from '@/features/logcenter/logCenterStore';
import { mockInvoke, invokeMock } from '../../mocks/tauri';


describe('logCenterStore 增量状态机', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    window.localStorage.clear();
  });

  const fullPayload = (content: string, offset = 100) => ({
    content,
    offset,
    path: '/srv/DebugLog/ServerLog.txt',
    fileSize: offset,
    needFullReload: false,
  });


  it('增量拉取携带 offset，半行在下一次增量原位补全', async () => {
    let call = 0;
    mockInvoke('logcenter_fetch', (args) => {
      call += 1;
      if (call === 1) return fullPayload('[INFO] first\n[INFO] par', 50);
      expect(args).toEqual({ incremental: true, offset: 50 });
      return fullPayload('tial line\n[WARN] done\n', 80);
    });

    const store = useLogCenterStore();
    await store.startSession();
    expect(store.displayedLines).toEqual(['[INFO] first', '[INFO] par']);

    await store.pollOnce();
    store.stopSession();
    expect(store.displayedLines).toEqual(['[INFO] first', '[INFO] partial line', '[WARN] done']);
  });

  it('行数限制裁剪最旧行', async () => {
    const manyLines = Array.from({ length: 120 }, (_, i) => `[INFO] line-${i}`).join('\n') + '\n';
    mockInvoke('logcenter_fetch', () => fullPayload(manyLines, 1000));
    const store = useLogCenterStore();
    store.setLineLimit(100);
    await store.startSession();
    store.stopSession();

    expect(store.totalBuffered).toBe(100);
    expect(store.displayedLines[0]).toBe('[INFO] line-20');
  });



  it('拉取失败进入退避并记录错误，不清空已有缓冲', async () => {
    let shouldFail = false;
    mockInvoke('logcenter_fetch', () => {
      if (shouldFail) throw new Error('network down');
      return fullPayload('[INFO] keep\n', 10);
    });
    const store = useLogCenterStore();
    await store.startSession();
    expect(store.displayedLines).toHaveLength(1);

    shouldFail = true;
    await store.refresh();
    store.stopSession();
    expect(store.error).toContain('network down');
    expect(store.consecutiveFailures).toBeGreaterThan(0);
    expect(store.displayedLines).toEqual(['[INFO] keep']);
  });

  it('clearLocal 只清缓冲；clearServer 调专用命令并重拉', async () => {
    mockInvoke('logcenter_fetch', () => fullPayload('[INFO] x\n', 10));
    const clearSpy = vi.fn(() => Promise.resolve('ok'));
    mockInvoke('logcenter_clear_server', clearSpy);

    const store = useLogCenterStore();
    await store.startSession();
    store.clearLocal();
    expect(store.displayedLines).toEqual([]);

    await store.clearServer();
    store.stopSession();
    expect(clearSpy).toHaveBeenCalledOnce();
    expect(store.displayedLines).toEqual(['[INFO] x']);
  });

  it('暂停后不再排程轮询，恢复后重启', async () => {
    vi.useFakeTimers();
    try {
      mockInvoke('logcenter_fetch', () => fullPayload('[INFO] a\n', 10));
      const store = useLogCenterStore();
      await store.startSession();

      store.togglePause();
      const callsAfterPause = invokeMock.mock.calls.length;
      await vi.advanceTimersByTimeAsync(20000);
      expect(invokeMock.mock.calls.length).toBe(callsAfterPause);
      expect(store.isPolling).toBe(false);

      store.togglePause();
      await vi.advanceTimersByTimeAsync(3500);
      expect(invokeMock.mock.calls.length).toBeGreaterThan(callsAfterPause);
      store.stopSession();
    } finally {
      vi.useRealTimers();
    }
  });
});
