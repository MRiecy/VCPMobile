import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import {
  clampLineLimit,
  levelOf,
  splitByKeyword,
  splitLogChunk,
  stripAnsi,
} from '@/features/logcenter/logText';
import { useLogCenterStore } from '@/features/logcenter/logCenterStore';
import { mockInvoke, invokeMock } from '../../mocks/tauri';

describe('logText 纯函数', () => {
  it('splitLogChunk 切分完整行并分离末尾半行', () => {
    const chunk = splitLogChunk('a\nb\nc');
    expect(chunk.lines).toEqual(['a', 'b', 'c']);
    expect(chunk.trailing).toBe('c');
  });

  it('splitLogChunk 以换行结尾时无半行', () => {
    const chunk = splitLogChunk('a\nb\n');
    expect(chunk.lines).toEqual(['a', 'b']);
    expect(chunk.trailing).toBe('');
  });

  it('splitLogChunk 半行拼接：carry 拼接到下一段开头（跨轮询不断行）', () => {
    const first = splitLogChunk('[INFO] hello wo');
    expect(first.trailing).toBe('[INFO] hello wo');
    const second = splitLogChunk('rld\n[WARN] next\n', first.trailing);
    expect(second.lines).toEqual(['[INFO] hello world', '[WARN] next']);
    expect(second.trailing).toBe('');
  });

  it('splitLogChunk 归一化 CRLF', () => {
    const chunk = splitLogChunk('a\r\nb\r\n');
    expect(chunk.lines).toEqual(['a', 'b']);
  });

  it('stripAnsi 剥除颜色控制码', () => {
    expect(stripAnsi('\x1b[31mred\x1b[0m plain')).toBe('red plain');
  });

  it('levelOf 覆盖全部级别标签（含 FATAL/WARNING）', () => {
    expect(levelOf('[t] [ERROR] boom')).toBe('error');
    expect(levelOf('[t] [FATAL] boom')).toBe('error');
    expect(levelOf('[t] [WARN] careful')).toBe('warn');
    expect(levelOf('[t] [WARNING] careful')).toBe('warn');
    expect(levelOf('[t] [INFO] ok')).toBe('info');
    expect(levelOf('[t] [LOG] ok')).toBe('info');
    expect(levelOf('[t] [DEBUG] trace')).toBe('debug');
    expect(levelOf('no tag line')).toBe('normal');
  });

  it('clampLineLimit 钳制区间与非法输入回落', () => {
    expect(clampLineLimit(10)).toBe(50);
    expect(clampLineLimit(99999)).toBe(5000);
    expect(clampLineLimit(500)).toBe(500);
    expect(clampLineLimit(Number.NaN)).toBe(500);
  });

  it('splitByKeyword 大小写不敏感且覆盖多次命中', () => {
    const parts = splitByKeyword('Error and error', 'ERROR');
    expect(parts.filter((p) => p.hit)).toHaveLength(2);
    expect(parts.map((p) => p.text).join('')).toBe('Error and error');
  });

  it('splitByKeyword 空关键词原样返回', () => {
    expect(splitByKeyword('abc', '  ')).toEqual([{ text: 'abc', hit: false }]);
  });
});

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

  it('首次会话执行全量拉取并应用快照', async () => {
    mockInvoke('logcenter_fetch', () => fullPayload('[INFO] a\n[INFO] b\n', 42));
    const store = useLogCenterStore();
    await store.startSession();
    store.stopSession();

    expect(invokeMock).toHaveBeenCalledWith('logcenter_fetch', {
      incremental: false,
      offset: 0,
    });
    expect(store.displayedLines).toEqual(['[INFO] a', '[INFO] b']);
    expect(store.logPath).toBe('/srv/DebugLog/ServerLog.txt');
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

  it('needFullReload 触发全量重拉', async () => {
    mockInvoke('logcenter_fetch', (args) => {
      if ((args as { incremental: boolean }).incremental) {
        return { content: '', offset: 0, path: '/p', fileSize: 0, needFullReload: true };
      }
      return fullPayload('[INFO] fresh\n', 10);
    });

    const store = useLogCenterStore();
    await store.startSession();
    await store.pollOnce();
    store.stopSession();
    expect(store.displayedLines).toEqual(['[INFO] fresh']);
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

  it('筛选仅作用于缓冲且大小写不敏感，匹配计数正确', async () => {
    mockInvoke('logcenter_fetch', () =>
      fullPayload('[INFO] alpha\n[ERROR] beta\n[INFO] ALPHA2\n', 100),
    );
    const store = useLogCenterStore();
    await store.startSession();
    store.stopSession();

    store.filterText = 'alpha';
    expect(store.matchedCount).toBe(2);
    expect(store.displayedLines).toEqual(['[INFO] alpha', '[INFO] ALPHA2']);
  });

  it('倒序切换反转显示顺序并持久化', async () => {
    mockInvoke('logcenter_fetch', () => fullPayload('[INFO] a\n[INFO] b\n', 10));
    const store = useLogCenterStore();
    await store.startSession();
    store.stopSession();

    store.toggleReverse();
    expect(store.displayedLines).toEqual(['[INFO] b', '[INFO] a']);
    expect(window.localStorage.getItem('vcp_log_reverse')).toBe('1');
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
