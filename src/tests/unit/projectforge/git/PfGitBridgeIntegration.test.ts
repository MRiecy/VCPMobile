/**
 * `PfGitPanel` ↔ T1 桥接客户端（projectForgeApi）集成测试（mock fetch，不触网）。
 *
 * 验证真实默认运行时链路：
 *   ListWorkspaces（details）→ PTYShellExecutor（ptyExec）→ BEGIN/END 标记解析 → 面板渲染；
 * 以及桥接离线、PTY 插件返回形态差异（details.output / content[0].text）的容错。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises } from '@vue/test-utils';
import PfGitPanel from '../../../../features/projectforge/components/PfGitPanel.vue';
import {
  __resetBridgeConfigForTests,
  saveBridgeConfig,
} from '../../../../core/bridge/bridgeConfig';
import { mountWithPinia } from '../../../utils/mount';
import { FULL_PORCELAIN, probeOutput, ptyOutput, SAMPLE_DIFF, snapshotOutput } from './fixtures';

interface FetchInit {
  method?: string;
  headers?: Record<string, string>;
  body?: string;
}

const fetchMock = vi.fn();

function httpResponse(body: unknown, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    text: async () => (typeof body === 'string' ? body : JSON.stringify(body)),
  } as unknown as Response;
}

function okEnvelope(details: unknown, content: { type: string; text: string }[] = [{ type: 'text', text: '' }]) {
  return httpResponse({
    ok: true,
    toolName: 'mock',
    requestId: 'test',
    elapsedMs: 3,
    result: { content, details },
  });
}

interface InvokeBody {
  toolName: string;
  args: Record<string, unknown>;
}

function parseBody(init: FetchInit | undefined): InvokeBody {
  return JSON.parse(String(init?.body || '{}')) as InvokeBody;
}

function ptyResultShape(text: string, exitCode = 0): unknown {
  return { output: text, exitCode };
}

async function settle(rounds = 8): Promise<void> {
  for (let index = 0; index < rounds; index += 1) await flushPromises();
}

async function mountRealPanel() {
  const wrapper = mountWithPinia(PfGitPanel);
  await settle();
  return wrapper;
}

beforeEach(() => {
  localStorage.clear();
  __resetBridgeConfigForTests();
  saveBridgeConfig({ baseUrl: 'http://mock-bridge:5974', token: 'mock-token' });
  fetchMock.mockReset();
  vi.stubGlobal('fetch', fetchMock);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

function installBridgeMock(options: { ptyContentOnly?: boolean; ptyNoMarkers?: boolean; offline?: boolean } = {}) {
  fetchMock.mockImplementation(async (_url: string, init?: FetchInit) => {
    if (options.offline) throw new Error('Failed to fetch');
    const { toolName, args } = parseBody(init);
    if (toolName === 'ProjectForge') {
      if (args.command === 'ListWorkspaces') {
        return okEnvelope(
          {
            command: 'ListWorkspaces',
            workspaces: [{ id: 'w1', alias: 'demo', path: '/repos/demo', enabled: true }],
          },
          [{ type: 'text', text: '| ID | 别名 | 路径 |\n|---|---|---|\n| w1 | demo | /repos/demo |' }],
        );
      }
      return httpResponse({ ok: false, error: { code: 'UNKNOWN', message: `未知命令 ${String(args.command)}` } }, 400);
    }
    if (toolName === 'PTYShellExecutor') {
      const command = String(args.command ?? '');
      if (options.ptyNoMarkers) {
        return okEnvelope(ptyResultShape('', 127));
      }
      let output = '';
      if (command.includes('rev-parse --is-inside-work-tree')) output = probeOutput([0]);
      else if (command.includes('git status --porcelain=v1 -z')) output = snapshotOutput({ status: FULL_PORCELAIN });
      else if (command.includes('git --no-pager diff')) output = ptyOutput(SAMPLE_DIFF);
      else if (command.includes('git add --')) output = ptyOutput('');
      else if (command.includes('git commit -m')) output = ptyOutput('[main abc1234] mock 提交');
      else if (command.includes('git push')) output = ptyOutput('Everything up-to-date');
      else output = ptyOutput('');
      if (options.ptyContentOnly) {
        // 某些插件实现只把文本放 content[0].text（details 为空）
        return okEnvelope(undefined, [{ type: 'text', text: output }]);
      }
      return okEnvelope(ptyResultShape(output));
    }
    return httpResponse({ ok: false, error: { code: 'UNKNOWN_TOOL', message: toolName } }, 404);
  });
}

describe('PfGitPanel ↔ MobilePluginBridge（mock fetch）', () => {
  it('默认运行时经 ListWorkspaces + PTYShellExecutor 渲染工作区/分支/变更分组', async () => {
    installBridgeMock();
    const wrapper = await mountRealPanel();

    // ListWorkspaces
    expect(wrapper.findAll('select.pf-git-select option')).toHaveLength(1);
    expect(wrapper.find('select.pf-git-select').text()).toContain('demo · /repos/demo');
    // PTY 快照 → 分支与分组
    expect(wrapper.find('.pf-git-branch-name').text()).toBe('main');
    expect(wrapper.find('.pf-git-upstream').text()).toBe('origin/main');
    expect(wrapper.findAll('.pf-git-group')).toHaveLength(4);
    expect(wrapper.findAll('.pf-git-row')).toHaveLength(8);

    // 请求体符合桥接契约：toolName + Bearer Token
    const calls = fetchMock.mock.calls as unknown as [string, FetchInit][];
    expect(calls.some(([url]) => url.endsWith('/api/mobile-bridge/invoke'))).toBe(true);
    const headers = calls[0][1]?.headers ?? {};
    expect(headers.Authorization).toBe('Bearer mock-token');
    const bodies = calls.map(([, init]) => parseBody(init));
    expect(bodies[0]).toEqual({ toolName: 'ProjectForge', args: { command: 'ListWorkspaces' } });
    const ptyBodies = bodies.filter((body) => body.toolName === 'PTYShellExecutor');
    expect(ptyBodies.length).toBeGreaterThanOrEqual(2);
    // 工作区探测：git -C <posix 路径> rev-parse --is-inside-work-tree
    const probeCommand = String(ptyBodies[0]?.args.command ?? '');
    expect(probeCommand).toContain('GIT_TERMINAL_PROMPT=0');
    expect(probeCommand).toContain('git -C "$p" rev-parse --is-inside-work-tree');
    expect(probeCommand).toContain("'/repos/demo'");
    // 状态快照：cd 到 POSIX 仓库路径并带防挂起环境
    const snapshotBody = ptyBodies.find((body) =>
      String(body.args.command).includes('git status --porcelain=v1 -z'),
    );
    expect(snapshotBody).toBeTruthy();
    expect(String(snapshotBody?.args.command)).toContain("cd '/repos/demo'");
    expect(String(snapshotBody?.args.command)).toContain('GIT_TERMINAL_PROMPT=0');
  });

  it('PTY 插件仅返回 content[0].text（details 为空）时仍能解析状态', async () => {
    installBridgeMock({ ptyContentOnly: true });
    const wrapper = await mountRealPanel();
    expect(wrapper.find('.pf-git-branch-name').text()).toBe('main');
    expect(wrapper.findAll('.pf-git-row')).toHaveLength(8);
  });

  it('桥接离线时展示中文错误提示', async () => {
    installBridgeMock({ offline: true });
    const wrapper = await mountRealPanel();
    expect(wrapper.text()).toContain('加载工作区失败');
    expect(wrapper.text()).toContain('无法连接桌面桥接');
  });

  it('桥接可达但 PTY 插件无输出（未启用/命令失败）时给出检测失败提示', async () => {
    installBridgeMock({ ptyNoMarkers: true });
    const wrapper = await mountRealPanel();
    expect(wrapper.text()).toContain('检测 Git 工作区失败');
    expect(wrapper.findAll('select.pf-git-select option')).toHaveLength(0);
  });

  it('桥接在线时可通过默认运行时提交（确认弹层注入 overlayStore 前不自动提交）', async () => {
    installBridgeMock();
    const wrapper = await mountRealPanel();
    await wrapper.findAll('.pf-git-row input[type="checkbox"]')[0].setValue(true);
    await wrapper.find('textarea.pf-git-message').setValue('feat: mock 提交');
    await wrapper.find('.pf-git-btn.primary').trigger('click');
    await settle();

    // 默认确认走 overlayStore.showConfirm，这里没有挂 GlobalOverlayManager，Promise 不会 resolve；
    // 因此只断言「未确认前不会发出写命令」——写操作必须经过用户确认。
    const bodies = (fetchMock.mock.calls as unknown as [string, FetchInit][]).map(([, init]) => parseBody(init));
    const writeBodies = bodies.filter(
      (body) => body.toolName === 'PTYShellExecutor' && /git (add|commit|push)/.test(String(body.args.command)),
    );
    expect(writeBodies).toHaveLength(0);
  });
});
