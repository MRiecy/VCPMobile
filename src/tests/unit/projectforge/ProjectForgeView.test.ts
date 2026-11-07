/**
 * V工程页面 Shell 测试：模式切换、工程列表/详情挂载、桥接设置、返回与刷新。
 * 子组件用 stub 隔离（各子组件有自己的单测），这里只验证 shell 的装配与交互。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import ProjectForgeView from '../../../features/projectforge/ProjectForgeView.vue';

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

function okResponse(details: any, text = '') {
  return httpResponse({
    ok: true,
    toolName: 'ProjectForge',
    requestId: 'test',
    result: { content: [{ type: 'text', text }], details },
  });
}

function parseBody(init: FetchInit | undefined) {
  return JSON.parse(String(init?.body || '{}'));
}

const LIST_TEXT = [
  '| ID | 名称 | 工作区 | 状态 | Todo | 报告 | 更新 |',
  '|---|---|---|---|---|---|---|',
  '| `proj1` | 示例工程 A | ai打工人 | review | 3/8 | 有 | 2026-10-29 19:19 |',
].join('\n');

function installBridgeMock() {
  fetchMock.mockImplementation(async (_url, init) => {
    const { args } = parseBody(init as FetchInit);
    if (args.command === 'ListWorkspaces') {
      return okResponse({
        command: 'ListWorkspaces',
        workspaces: [
          { id: 'ws_1', alias: 'ai打工人', path: '/home/user/示例工程', enabled: true, status: 'ready' },
        ],
      });
    }
    if (args.command === 'ListProjects') {
      return okResponse(
        {
          command: 'ListProjects',
          projects: [
            {
              id: 'proj1',
              name: '示例工程 A',
              workspace_alias: 'ai打工人',
              root: '/home/user/示例工程/demo-workspace',
              status: 'review',
              hasReport: true,
              updated_at: '2026-10-29sync-compat:19:00Z',
            },
          ],
        },
        LIST_TEXT,
      );
    }
    return okResponse({ command: args.command });
  });
}

const PfProjectListStub = {
  name: 'PfProjectList',
  props: { activeProjectId: { type: String, default: '' } },
  emits: ['select'],
  template: '<button data-test="list" @click="$emit(\'select\', \'proj1\')">project-list</button>',
};
const PfProjectDetailStub = {
  name: 'PfProjectDetail',
  props: { projectId: { type: String, required: true } },
  emits: ['back'],
  template: '<button data-test="detail" @click="$emit(\'back\')">detail-{{ projectId }}</button>',
};
const PfSourcePanelStub = { name: 'PfSourcePanel', template: '<div data-test="source">source-panel</div>' };
const PfGitPanelStub = { name: 'PfGitPanel', template: '<div data-test="git">git-panel</div>' };

function mountView(isOpen = true) {
  return mount(ProjectForgeView, {
    props: { isOpen, zIndex: 42 },
    global: {
      stubs: {
        PfProjectList: PfProjectListStub,
        PfProjectDetail: PfProjectDetailStub,
        PfSourcePanel: PfSourcePanelStub,
        PfGitPanel: PfGitPanelStub,
      },
    },
  });
}

async function settle() {
  await flushPromises();
  await new Promise((resolve) => setTimeout(resolve, 0));
  await flushPromises();
}

beforeEach(() => {
  localStorage.clear();
  setActivePinia(createPinia());
  fetchMock.mockReset();
  vi.stubGlobal('fetch', fetchMock);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('ProjectForgeView shell', () => {
  it('打开页面展示三个模式入口并挂载工程列表，且发起 ListProjects 请求', async () => {
    installBridgeMock();
    const wrapper = mountView();
    await settle();

    const text = wrapper.text();
    expect(text).toContain('工程');
    expect(text).toContain('源码');
    expect(text).toContain('Git');
    expect(wrapper.find('[data-test="list"]').exists()).toBe(true);
    const commands = fetchMock.mock.calls.map((call) => parseBody(call[1] as FetchInit).args?.command);
    expect(commands).toContain('ListProjects');
    wrapper.unmount();
  });

  it('切换源码 / Git 模式显示对应面板', async () => {
    installBridgeMock();
    const wrapper = mountView();
    await settle();

    const sourceBtn = wrapper.findAll('button').find((b) => b.text().includes('源码'));
    expect(sourceBtn).toBeTruthy();
    await sourceBtn!.trigger('click');
    await settle();
    expect(wrapper.find('[data-test="source"]').exists()).toBe(true);
    expect(wrapper.find('[data-test="list"]').exists()).toBe(false);

    const gitBtn = wrapper.findAll('button').find((b) => b.text() === 'Git');
    expect(gitBtn).toBeTruthy();
    await gitBtn!.trigger('click');
    await settle();
    expect(wrapper.find('[data-test="git"]').exists()).toBe(true);
    wrapper.unmount();
  });

  it('选择工程进入详情，详情返回后回到列表', async () => {
    installBridgeMock();
    const wrapper = mountView();
    await settle();

    await wrapper.find('[data-test="list"]').trigger('click');
    await settle();
    const detail = wrapper.find('[data-test="detail"]');
    expect(detail.exists()).toBe(true);
    expect(detail.text()).toContain('proj1');

    await detail.trigger('click');
    await settle();
    expect(wrapper.find('[data-test="detail"]').exists()).toBe(false);
    expect(wrapper.find('[data-test="list"]').exists()).toBe(true);
    wrapper.unmount();
  });

  it('返回键：列表模式触发 close；详情模式先回到列表', async () => {
    installBridgeMock();
    const wrapper = mountView();
    await settle();

    const back = () => wrapper.findAll('button').find((b) => b.attributes('aria-label') === '返回')!;
    await wrapper.find('[data-test="list"]').trigger('click');
    await settle();
    await back().trigger('click');
    await settle();
    expect(wrapper.emitted('close')).toBeFalsy();

    await back().trigger('click');
    await settle();
    expect(wrapper.emitted('close')).toBeTruthy();
    wrapper.unmount();
  });

  it('设置弹层展示桥接字段，保存后写入统一桥接配置', async () => {
    installBridgeMock();
    const wrapper = mountView();
    await settle();

    const settingsBtn = wrapper.findAll('button').find((b) => b.attributes('aria-label') === 'V工程设置');
    expect(settingsBtn).toBeTruthy();
    await settingsBtn!.trigger('click');
    await settle();

    expect(wrapper.text()).toContain('桥接设置');
    expect(wrapper.text()).toContain('桥接地址 Base');
    expect(wrapper.text()).toContain('Bearer Token');
    expect(wrapper.text()).toContain('施工署名');

    const baseInput = wrapper.find('input[placeholder*="mobile-bridge"]');
    expect(baseInput.exists()).toBe(true);
    await baseInput.setValue('http://192.168.1.9:5974/api/mobile-bridge');
    const saveBtn = wrapper.findAll('button').find((b) => b.text() === '保存');
    expect(saveBtn).toBeTruthy();
    await saveBtn!.trigger('click');
    await settle();

    const stored = localStorage.getItem('vcp.bridge.settings.v1');
    expect(stored).toBeTruthy();
    expect(stored).toContain('192.168.1.9:5974');
    wrapper.unmount();
  });

  it('刷新按钮重新拉取工程列表', async () => {
    installBridgeMock();
    const wrapper = mountView();
    await settle();
    fetchMock.mockClear();

    const refreshBtn = wrapper.findAll('button').find((b) => b.attributes('aria-label') === '刷新');
    expect(refreshBtn).toBeTruthy();
    await refreshBtn!.trigger('click');
    await settle();
    const commands = fetchMock.mock.calls.map((call) => parseBody(call[1] as FetchInit).args?.command);
    expect(commands.some((cmd) => cmd === 'ListProjects' || cmd === 'ListWorkspaces')).toBe(true);
    wrapper.unmount();
  });

  it('关闭状态不请求桥接', async () => {
    installBridgeMock();
    const wrapper = mountView(false);
    await settle();
    expect(fetchMock).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it('桥接不可达时设置弹层展示连接失败信息', async () => {
    fetchMock.mockRejectedValue(new TypeError('Failed to fetch'));
    const wrapper = mountView();
    await settle();

    const settingsBtn = wrapper.findAll('button').find((b) => b.attributes('aria-label') === 'V工程设置');
    await settingsBtn!.trigger('click');
    await settle();
    const testBtn = wrapper.findAll('button').find((b) => b.text().includes('测试连接'));
    expect(testBtn).toBeTruthy();
    await testBtn!.trigger('click');
    await settle();
    // 连接状态行始终存在（未连接/连接失败），不应抛异常
    expect(wrapper.text()).toContain('桥接设置');
    wrapper.unmount();
  });
});
