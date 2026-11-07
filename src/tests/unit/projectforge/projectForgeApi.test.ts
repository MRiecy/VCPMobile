import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  DEFAULT_BRIDGE_TIMEOUT_MS,
  ProjectForgeError,
  bridgeContentParts,
  bridgeContentText,
  describeProjectForgeError,
  extractPtyExitCode,
  extractPtyText,
  getBridgeBase,
  getBridgeToken,
  pfInvoke,
  pfInvokeEnvelope,
  pluginInvoke,
  ptyExec,
  unwrapBridgeResult,
} from '../../../features/projectforge/projectForgeApi';
import {
  DEFAULT_BRIDGE_TOKEN,
  LEGACY_DEFAULT_BASE,
  __resetBridgeConfigForTests,
  saveBridgeConfig,
} from '../../../core/bridge/bridgeConfig';

interface FetchInit {
  method?: string;
  headers?: Record<string, string>;
  body?: string;
  signal?: AbortSignal;
}

const fetchMock = vi.fn();

function httpResponse(body: unknown, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    text: async () => (typeof body === 'string' ? body : JSON.stringify(body)),
  } as unknown as Response;
}

function okEnvelope(details: unknown, content: Array<{ type: string; text: string }> = [{ type: 'text', text: '' }]) {
  return httpResponse({
    ok: true,
    toolName: 'ProjectForge',
    requestId: 'test-req',
    elapsedMs: 3,
    result: { content, details },
  });
}

function parseBody(init: FetchInit): { toolName: string; args: Record<string, unknown> } {
  return JSON.parse(String(init?.body || '{}'));
}

async function expectProjectForgeError(task: Promise<unknown>, kind: string, messagePart: RegExp) {
  try {
    await task;
    expect.unreachable('应当抛错');
  } catch (error) {
    expect(error).toBeInstanceOf(ProjectForgeError);
    const typed = error as ProjectForgeError;
    expect(typed.kind).toBe(kind);
    expect(typed.message).toMatch(messagePart);
    return typed;
  }
}

beforeEach(() => {
  localStorage.clear();
  __resetBridgeConfigForTests();
  fetchMock.mockReset();
  vi.stubGlobal('fetch', fetchMock);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('桥接地址与请求构造', () => {
  it('getBridgeBase / getBridgeToken 跟随统一桥接配置', () => {
    expect(getBridgeBase()).toBe(LEGACY_DEFAULT_BASE);
    expect(getBridgeToken()).toBe(DEFAULT_BRIDGE_TOKEN);

    saveBridgeConfig({ baseUrl: 'http://127.0.0.1:8080/api/mobile-bridge/', token: ' tk ' });
    expect(getBridgeBase()).toBe('http://127.0.0.1:8080/api/mobile-bridge');
    expect(getBridgeToken()).toBe('tk');
  });

  it('pfInvoke 发送 POST /invoke，body 为 {toolName,args}，并带 Bearer 鉴权', async () => {
    fetchMock.mockResolvedValue(okEnvelope({ command: 'ListProjects', projects: [] }));
    await pfInvoke('ListProjects', { workspace: 'all', limit: 50 });

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as [string, FetchInit];
    expect(url).toBe(`${LEGACY_DEFAULT_BASE}/invoke`);
    expect(init.method).toBe('POST');
    expect(init.headers?.Authorization).toBe(`Bearer ${DEFAULT_BRIDGE_TOKEN}`);
    expect(init.headers?.['Content-Type']).toBe('application/json');
    expect(parseBody(init)).toEqual({
      toolName: 'ProjectForge',
      args: { command: 'ListProjects', workspace: 'all', limit: 50 },
    });
  });

  it('空 command 本地拦下（validation，不发请求）', async () => {
    await expectProjectForgeError(pfInvoke('   '), 'validation', /缺少 command/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('默认超时常量存在且为正数', () => {
    expect(DEFAULT_BRIDGE_TIMEOUT_MS).toBeGreaterThan(0);
  });
});

describe('pfInvoke 返回解包', () => {
  it('details 优先原样返回', async () => {
    fetchMock.mockResolvedValue(okEnvelope({ command: 'GetProject', project: { id: 'p1' }, todos: [{ seq: 1 }] }));
    const result = await pfInvoke<{ project: { id: string }; todos: unknown[] }>('GetProject', { projectId: 'p1' });
    expect(result.project.id).toBe('p1');
    expect(result.todos).toHaveLength(1);
  });

  it('details 为空对象时解析 content[0].text 的 JSON', async () => {
    fetchMock.mockResolvedValue(
      okEnvelope({}, [{ type: 'text', text: '{"okData":true,"count":2}' }]),
    );
    const result = await pfInvoke<{ okData: boolean; count: number }>('SomeCommand');
    expect(result).toEqual({ okData: true, count: 2 });
  });

  it('details 为 null 时容忍 ```json 围栏解析', async () => {
    fetchMock.mockResolvedValue(
      okEnvelope(null, [{ type: 'text', text: '```json\n{"nested":{"value":7}}\n```' }]),
    );
    const result = await pfInvoke<{ nested: { value: number } }>('SomeCommand');
    expect(result.nested.value).toBe(7);
  });

  it('details 与 JSON 都无法解析时返回纯文本', async () => {
    fetchMock.mockResolvedValue(okEnvelope(undefined, [{ type: 'text', text: '# 纯文本摘要\n- 一行' }]));
    const result = await pfInvoke<string>('SomeCommand');
    expect(result).toContain('纯文本摘要');
  });

  it('details 与 content 都为空时抛 protocol', async () => {
    fetchMock.mockResolvedValue(okEnvelope(null, []));
    await expectProjectForgeError(pfInvoke('GetProject', { projectId: 'p1' }), 'protocol', /空结果/);
  });
});

describe('pfInvokeEnvelope 与 content 工具', () => {
  it('保留多段 content 与 details（ReadCode 正文在 content[1]）', async () => {
    fetchMock.mockResolvedValue(
      okEnvelope(
        { command: 'ReadCode', read: [{ path: 'a.py', kind: 'text', size: 12 }], skipped: [], failed: [] },
        [
          { type: 'text', text: '## ReadCode · 1 个文件' },
          { type: 'text', text: '#### a.py\n```python\n1 | print(1)\n```' },
        ],
      ),
    );
    const envelope = await pfInvokeEnvelope('ReadCode', { projectId: 'p1', path: 'a.py' });
    const parts = bridgeContentParts(envelope);
    expect(parts).toHaveLength(2);
    expect(parts[1].text).toContain('print(1)');
    expect(bridgeContentText(envelope)).toContain('## ReadCode');
    expect((envelope.result?.details as { read: unknown[] }).read).toHaveLength(1);
    expect(envelope.ok).toBe(true);
    expect(envelope.requestId).toBe('test-req');
    expect(envelope.elapsedMs).toBe(3);
  });

  it('unwrapBridgeResult 可直接解 details 或 JSON 文本', () => {
    expect(unwrapBridgeResult('X', { ok: true, result: { details: { a: 1 } } })).toEqual({ a: 1 });
    expect(
      unwrapBridgeResult('X', { ok: true, result: { content: [{ type: 'text', text: '{"b":2}' }] } }),
    ).toEqual({ b: 2 });
    expect(() => unwrapBridgeResult('X', { ok: true, result: {} })).toThrow(ProjectForgeError);
  });
});

describe('pluginInvoke / ptyExec', () => {
  it('pluginInvoke 原样透传 toolName 与 args（不注入 command 包装）', async () => {
    fetchMock.mockResolvedValue(okEnvelope({ files: ['a.ts'] }));
    const result = await pluginInvoke<{ files: string[] }>('FileOperator', {
      command: 'ListDirectory',
      directoryPath: 'src',
    });
    expect(result.files).toEqual(['a.ts']);
    const body = parseBody(fetchMock.mock.calls[0][1] as FetchInit);
    expect(body.toolName).toBe('FileOperator');
    expect(body.args).toEqual({ command: 'ListDirectory', directoryPath: 'src' });
  });

  it('pluginInvoke 无 details 时返回 content 纯文本', async () => {
    fetchMock.mockResolvedValue(okEnvelope(undefined, [{ type: 'text', text: 'total 12\nfile.txt' }]));
    const result = await pluginInvoke<string>('FileOperator', { command: 'ListDirectory' });
    expect(result).toBe('total 12\nfile.txt');
  });

  it('ptyExec 发送 {command} 并解析 details.text / exitCode', async () => {
    fetchMock.mockResolvedValue(okEnvelope({ command: 'git status', text: 'On branch main', exitCode: 0 }));
    const result = await ptyExec('git status');
    expect(result.ok).toBe(true);
    expect(result.text).toBe('On branch main');
    expect(result.exitCode).toBe(0);
    const body = parseBody(fetchMock.mock.calls[0][1] as FetchInit);
    expect(body.toolName).toBe('PTYShellExecutor');
    expect(body.args).toEqual({ command: 'git status' });
  });

  it('ptyExec 非 0 退出码 → ok=false（兼容 exit_code / 字符串数字）', async () => {
    fetchMock.mockResolvedValue(okEnvelope({ output: 'fatal: not a git repository', exit_code: '128' }));
    const result = await ptyExec('git status');
    expect(result.ok).toBe(false);
    expect(result.exitCode).toBe(128);
    expect(result.text).toContain('not a git repository');
  });

  it('ptyExec 可透传额外 args（cwd 等）且 command 不被覆盖', async () => {
    fetchMock.mockResolvedValue(okEnvelope({ text: '', exitCode: 0 }));
    await ptyExec('git log -1', { args: { cwd: '/d/work', shell: 'bash' } });
    const body = parseBody(fetchMock.mock.calls[0][1] as FetchInit);
    expect(body.args).toEqual({ cwd: '/d/work', shell: 'bash', command: 'git log -1' });
  });

  it('ptyExec 只返回纯文本时 text 兜底且 ok 为 true', async () => {
    fetchMock.mockResolvedValue(okEnvelope(undefined, [{ type: 'text', text: 'hello from pty' }]));
    const result = await ptyExec('echo hello');
    expect(result.text).toBe('hello from pty');
    expect(result.ok).toBe(true);
    expect(result.exitCode).toBeNull();
  });

  it('ptyExec 空命令本地拦下', async () => {
    await expectProjectForgeError(ptyExec('  '), 'validation', /命令不能为空/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('extractPtyText / extractPtyExitCode 支持嵌套结构', () => {
    expect(extractPtyText({ result: { stdout: 'nested out' } })).toBe('nested out');
    expect(extractPtyText([{ type: 'text', text: 'a' }, { type: 'text', text: 'b' }])).toBe('a\nb');
    expect(extractPtyExitCode({ result: { code: 0 } })).toBe(0);
    expect(extractPtyExitCode({ text: 'no code' })).toBeNull();
  });
});

describe('错误分级', () => {
  it('网络不可达 → network，并提示检查网络/设置', async () => {
    fetchMock.mockRejectedValue(new TypeError('Failed to fetch'));
    const error = await expectProjectForgeError(pfInvoke('ListProjects'), 'network', /无法连接桌面桥接/);
    expect(error.status).toBeUndefined();
  });

  it('超时 → network（带毫秒数）', async () => {
    fetchMock.mockImplementation(
      (_url: string, init: FetchInit) =>
        new Promise((_resolve, reject) => {
          init?.signal?.addEventListener('abort', () => {
            const error = new Error('The operation was aborted');
            error.name = 'AbortError';
            reject(error);
          });
        }),
    );
    const error = await expectProjectForgeError(
      pfInvoke('ListProjects', {}, { timeoutMs: 10 }),
      'network',
      /请求超时（10ms）/,
    );
    expect(error.message).toContain('重试');
  });

  it('外部 AbortSignal 取消 → network', async () => {
    fetchMock.mockImplementation(
      (_url: string, init: FetchInit) =>
        new Promise((_resolve, reject) => {
          init?.signal?.addEventListener('abort', () => {
            const error = new Error('aborted');
            error.name = 'AbortError';
            reject(error);
          });
        }),
    );
    const controller = new AbortController();
    const task = pfInvoke('ListProjects', {}, { signal: controller.signal });
    controller.abort();
    await expectProjectForgeError(task, 'network', /请求已取消/);
  });

  it('401 / 403 → auth（提示检查 Token）', async () => {
    fetchMock.mockResolvedValue(
      httpResponse({ ok: false, error: { code: 'BRIDGE_AUTH_FAILED', message: 'Unauthorized' } }, 401),
    );
    const error = await expectProjectForgeError(pfInvoke('ListProjects'), 'auth', /Token/);
    expect(error.status).toBe(401);
    expect(error.code).toBe('BRIDGE_AUTH_FAILED');

    fetchMock.mockResolvedValue(httpResponse('<html>forbidden</html>', 403));
    await expectProjectForgeError(pfInvoke('ListProjects'), 'auth', /鉴权失败（HTTP 403）/);
  });

  it('502 + ok:false → bridge，透出桌面端 message 与 code', async () => {
    fetchMock.mockResolvedValue(
      httpResponse(
        {
          ok: false,
          error: { code: 'BRIDGE_INVOKE_FAILED', message: '[ProjectForge] 缺少 projectId。可用 ListProjects 查询已有工程。' },
        },
        502,
      ),
    );
    const error = await expectProjectForgeError(pfInvoke('GetProject'), 'bridge', /缺少 projectId/);
    expect(error.status).toBe(502);
    expect(error.code).toBe('BRIDGE_INVOKE_FAILED');
  });

  it('HTTP 200 但 ok:false 也算 bridge', async () => {
    fetchMock.mockResolvedValue(httpResponse({ ok: false, error: { message: '业务失败' } }, 200));
    await expectProjectForgeError(pfInvoke('ListProjects'), 'bridge', /业务失败/);
  });

  it('非 JSON 响应 → protocol（提示检查 Base 地址）', async () => {
    fetchMock.mockResolvedValue(httpResponse('<html>bad gateway</html>', 502));
    await expectProjectForgeError(pfInvoke('ListProjects'), 'protocol', /非 JSON 响应（HTTP 502）/);
  });

  it('缺 result 字段 → protocol', async () => {
    fetchMock.mockResolvedValue(httpResponse({ ok: true }, 200));
    await expectProjectForgeError(pfInvoke('ListProjects'), 'protocol', /缺少 result/);
  });

  it('缺 ok:true → protocol', async () => {
    fetchMock.mockResolvedValue(httpResponse({ result: { details: { a: 1 } } }, 200));
    await expectProjectForgeError(pfInvoke('ListProjects'), 'protocol', /不是预期的 \{ ok:true \} 结构/);
  });

  it('空响应体 → protocol', async () => {
    fetchMock.mockResolvedValue(httpResponse('', 200));
    await expectProjectForgeError(pfInvoke('ListProjects'), 'protocol', /桥接响应为空/);
  });

  it('describeProjectForgeError 给出类型前缀；未知错误可读', () => {
    expect(describeProjectForgeError(new ProjectForgeError('auth', 'Token 失效'))).toBe('[鉴权] Token 失效');
    expect(describeProjectForgeError(new ProjectForgeError('network', '网络断了'))).toBe('[网络] 网络断了');
    expect(describeProjectForgeError(new Error('boom'))).toBe('boom');
  });
});
