/**
 * T2 × T1 集成冒烟：真实 useProjectForgeStore + mock fetch（桥接响应按 T1 契约）。
 *
 * 单测里其他文件 mock store 验证组件行为；这里故意不 mock store，
 * 验证组件与 T1 真实 state / action 的字段契约（projects / detail 聚合 / getBatchNodes /
 * getNodeDiff / rollback 参数与写后回读）一致。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { flushPromises, mount } from '@vue/test-utils';
import PfProjectList from '../../../../features/projectforge/components/PfProjectList.vue';
import PfProjectDetail from '../../../../features/projectforge/components/PfProjectDetail.vue';

const fetchMock = vi.fn();

const PROJECT = {
  id: 'proj1',
  name: '示例工程 A',
  workspace_alias: 'ai打工人',
  root: '/home/user/ai/dsk-money-worker',
  rootInfo: {
    root: '/home/user/ai/dsk-money-worker',
    workspaceAlias: 'ai打工人',
    writable: true,
  },
  status: 'review',
  created_by: 'claude',
  created_at: '2026-10-20T03:00:00.000Z',
  updated_at: '2026-10-29T11:19:14.830Z',
  stats: { nodeCount: 14, fileCount: 7, added: 846, removed: 73, lastAt: '2026-10-29T11:19:14.830Z' },
  report: '# 验收报告\n\n结论：通过',
  report_by: 'claude',
  maids: ['claude'],
};

const TODOS = [
  { id: 1, seq: 1, title: '修复上传通道', status: 'done', note: null },
  { id: 2, seq: 2, title: '核对 state.json', status: 'pending', note: '等待登录态' },
];

const NODES = [
  {
    id: 25,
    batch_id: 5,
    batch_kind: 'edit',
    op: 'edit',
    file_path: 'tools/video.py',
    maid: 'claude',
    effective_reason: '补充说明',
    added: 7,
    removed: 2,
    created_at: '2026-10-29T11:18:58.496Z',
  },
  {
    id: 24,
    batch_id: 5,
    batch_kind: 'edit',
    op: 'edit',
    file_path: 'tools/run.py',
    maid: 'claude',
    effective_reason: '补充说明',
    added: 2,
    removed: 0,
    created_at: '2026-10-29T11:18:58.496Z',
  },
  {
    id: 21,
    batch_id: 4,
    batch_kind: 'edit',
    op: 'edit',
    file_path: 'tools/video.py',
    maid: 'claude',
    effective_reason: '初始实现',
    added: 3,
    removed: 0,
    created_at: '2026-10-28T10:00:00.000Z',
  },
];

const LIST_TEXT = [
  '| ID | 名称 | 工作区 | 状态 | Todo | 报告 | 更新 |',
  '|---|---|---|---|---|---|---|',
  '| `proj1` | 示例工程 A | ai打工人 | review | 1/2 | 有 | 2026-10-29 19:19 |',
].join('\n');

function envelope(details: unknown, text = ''): unknown {
  return { ok: true, toolName: 'ProjectForge', result: { content: [{ type: 'text', text }], details } };
}

function httpResponse(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    text: async () => JSON.stringify(body),
  } as unknown as Response;
}

function installBridgeRouter(): void {
  fetchMock.mockImplementation(async (_url: string, init: RequestInit) => {
    const payload = JSON.parse(String(init?.body || '{}'));
    const args = (payload.args || {}) as Record<string, any>;
    switch (args.command) {
      case 'ListWorkspaces':
        return httpResponse(
          envelope({ workspaces: [{ id: 'ws1', alias: 'ai打工人', path: '/home/user/ai', enabled: true }] }),
        );
      case 'ListProjects':
        return httpResponse(envelope({ projects: [PROJECT] }, LIST_TEXT));
      case 'SearchProjects':
        return httpResponse(envelope({ projects: [PROJECT] }, LIST_TEXT));
      case 'GetProject':
        return httpResponse(
          envelope({
            project: PROJECT,
            todos: TODOS,
            stats: { nodeCount: 3, fileCount: 2, added: 12, removed: 2 },
          }),
        );
      case 'SearchHistory':
        return httpResponse(
          envelope({
            count: NODES.length,
            nodes: args.batch ? NODES.filter((node) => node.batch_id === 5) : NODES,
          }),
        );
      case 'GetNodeDiff':
        return httpResponse(
          envelope({ count: 1 }, '节点 n25 · tools/video.py\n\n```diff\n--- a/tools/video.py\n+++ b/tools/video.py\n@@ -1 +1 @@\n-old\n+new\n```'),
        );
      case 'Rollback':
        return httpResponse(envelope({ status: 'ok', batchId: 30, count: 1 }));
      case 'ListWorkspaces_fallback':
        return httpResponse(envelope({}));
      default:
        throw new Error(`集成路由未覆盖命令：${String(args.command)}`);
    }
  });
}

function mountWith(pinia: ReturnType<typeof createPinia>, component: any, options: Record<string, any> = {}) {
  return mount(component, {
    ...options,
    global: { plugins: [pinia], stubs: { Teleport: true } },
  });
}

async function settle(times = 8): Promise<void> {
  for (let index = 0; index < times; index += 1) await flushPromises();
}

describe('T2 × T1 集成：真实 store + mock 桥接', () => {
  let pinia: ReturnType<typeof createPinia>;

  beforeEach(() => {
    localStorage.clear();
    fetchMock.mockReset();
    installBridgeRouter();
    vi.stubGlobal('fetch', fetchMock);
    pinia = createPinia();
    setActivePinia(pinia);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('工程列表用真实 store 拉取并渲染（含 Markdown Todo 进度解析）', async () => {
    const wrapper = mountWith(pinia, PfProjectList, { props: { activeProjectId: 'proj1' } });
    await settle();

    expect(fetchMock).toHaveBeenCalled();
    expect(wrapper.text()).toContain('示例工程 A');
    expect(wrapper.text()).toContain('proj1');
    expect(wrapper.text()).toContain('待验收');
    expect(wrapper.text()).toContain('Todo 1/2');

    await wrapper.find('.pf-project-main').trigger('click');
    expect(wrapper.emitted('select')?.[0]).toEqual(['proj1']);
  });

  it('工程详情用真实 store：GetProject + SearchHistory 聚合出 5 tab 数据', async () => {
    const wrapper = mountWith(pinia, PfProjectDetail, { props: { projectId: 'proj1' } });
    await settle();

    // 工程头
    expect(wrapper.text()).toContain('示例工程 A');
    expect(wrapper.text()).toContain('/home/user/ai/dsk-money-worker');
    // 概览：统计卡与 todo
    expect(wrapper.text()).toContain('变动节点');
    expect(wrapper.text()).toContain('修复上传通道');
    expect(wrapper.text()).toContain('@claude');

    // 开发脉络：真实 groupTimeline 聚合（b5 / node_count / files 为 string[]）
    const timelineTab = wrapper.findAll('.pf-tab').find((tab: any) => tab.text() === '开发脉络')!;
    await timelineTab.trigger('click');
    await settle();
    expect(wrapper.text()).toContain('b5');
    expect(wrapper.text()).toContain('2 个节点');
    expect(wrapper.text()).toContain('tools/video.py');

    // 展开批次 → getBatchNodes（SearchHistory batch 过滤）→ 点节点打开 diff 弹层
    await wrapper.find('.pf-batch-head').trigger('click');
    await settle();
    expect(wrapper.text()).toContain('n25');

    await wrapper.findAll('.pf-node').find((node: any) => node.text().includes('n25'))!.trigger('click');
    await settle();
    const diffRequests = fetchMock.mock.calls
      .map((call) => JSON.parse(String((call[1] as RequestInit)?.body || '{}')))
      .filter((payload) => payload.args?.command === 'GetNodeDiff');
    expect(diffRequests.length).toBeGreaterThan(0);
    expect(diffRequests[diffRequests.length - 1].args.node).toBe('n25');
    expect(wrapper.find('.pf-diff-scroll').exists()).toBe(true);
    expect(wrapper.find('.pf-diff-line.is-add').text()).toContain('+new');
  });

  it('回退：真实 store 组装 Rollback 参数（toNode/path/署名）并写后回读', async () => {
    const wrapper = mountWith(pinia, PfProjectDetail, { props: { projectId: 'proj1' } });
    await settle();

    // 设置回退署名
    const signatureInput = wrapper.find('.pf-signature-input');
    await signatureInput.setValue('claude');
    await signatureInput.trigger('change');
    await settle();

    // 打开开发脉络 → b5 → n25 diff
    await wrapper.findAll('.pf-tab').find((tab: any) => tab.text() === '开发脉络')!.trigger('click');
    await settle();
    await wrapper.find('.pf-batch-head').trigger('click');
    await settle();
    await wrapper.findAll('.pf-node').find((node: any) => node.text().includes('n25'))!.trigger('click');
    await settle();

    // 撤销此变动 → 二次确认 → Rollback
    const beforeButton = wrapper.findAll('.pf-revert-actions .pf-btn').find((button: any) =>
      button.text().includes('撤销此变动'),
    )!;
    await beforeButton.trigger('click');
    await settle();
    await wrapper
      .findAll('.pf-confirm-actions .pf-btn')
      .find((button: any) => button.text().includes('确认回退'))!
      .trigger('click');
    await settle();

    const rollbackRequest = fetchMock.mock.calls
      .map((call) => JSON.parse(String((call[1] as RequestInit)?.body || '{}')))
      .find((payload) => payload.args?.command === 'Rollback');
    expect(rollbackRequest).toBeTruthy();
    expect(rollbackRequest.args).toMatchObject({
      projectId: 'proj1',
      toNode: 'n21',
      path: 'tools/video.py',
      maid: 'claude',
    });

    // 写后回读：rollback 成功后 store 重新拉取 GetProject（详情仍正常渲染）
    expect(wrapper.text()).toContain('示例工程 A');

    // 成功 toast 文案
    const notificationStore = (await import('../../../../core/stores/notification')).useNotificationStore(pinia);
    expect(notificationStore.activeToasts.length).toBeGreaterThan(0);
  });
});
