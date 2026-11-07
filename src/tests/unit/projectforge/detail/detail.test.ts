import { beforeEach, describe, expect, it, vi } from 'vitest';
import PfProjectDetail from '../../../../features/projectforge/components/PfProjectDetail.vue';
import { createMockStore, flush, makeDetail, makeProject, mountWithPinia } from './testUtils';

const state = vi.hoisted(() => ({ store: {} as any }));

vi.mock('../../../../features/projectforge/projectForgeStore', () => ({
  useProjectForgeStore: () => state.store,
}));

function mountDetail(overrides: Record<string, any> = {}, projectId = 'proj1') {
  state.store = createMockStore({
    currentProjectId: projectId,
    detail: makeDetail(),
    ...overrides,
  });
  return mountWithPinia(PfProjectDetail, { props: { projectId } });
}

describe('PfProjectDetail — 工程详情 shell', () => {
  beforeEach(() => {
    state.store = createMockStore();
  });

  it('渲染工程头（名称 / 状态 / ID / 工作区 / 根目录 / 创建者）与 5 个 tab', async () => {
    const { wrapper } = mountDetail();
    await flush();

    expect(wrapper.text()).toContain('示例工程 A');
    expect(wrapper.text()).toContain('待验收');
    expect(wrapper.text()).toContain('proj1');
    expect(wrapper.text()).toContain('ai打工人');
    expect(wrapper.text()).toContain('/home/user/ai/dsk-money-worker');
    expect(wrapper.text()).toContain('@claude');

    const tabs = wrapper.findAll('.pf-tab');
    expect(tabs).toHaveLength(5);
    expect(tabs.map((tab: any) => tab.text())).toEqual(['概览', '开发脉络', '改动文件', '搜索历史', '验收报告']);
    // 默认展示概览
    expect(wrapper.text()).toContain('变动节点');
  });

  it('没有 detail 时挂载即调用 openProject(projectId)，加载成功后渲染', async () => {
    state.store = createMockStore({
      currentProjectId: null,
      detail: null,
      openProject: vi.fn().mockImplementation(async () => {
        state.store.detail = makeDetail();
        state.store.currentProjectId = 'proj1';
        return state.store.detail;
      }),
    });
    const { wrapper } = mountWithPinia(PfProjectDetail, { props: { projectId: 'proj1' } });
    await flush();

    expect(state.store.openProject).toHaveBeenCalledTimes(1);
    expect(state.store.openProject.mock.calls[0][0]).toBe('proj1');
    expect(wrapper.text()).toContain('示例工程 A');
  });

  it('只读工程显示「无法回退」警告', async () => {
    const { wrapper } = mountDetail({
      detail: makeDetail({
        project: makeProject({
          rootInfo: { root: '/home/ai/ro', workspaceAlias: 'ai打工人', writable: false, blockedReason: '工程只读' },
        }),
      }),
    });
    await flush();
    expect(wrapper.text()).toContain('工程只读');
    expect(wrapper.text()).toContain('无法回退');
  });

  it('已删除工程显示删除警告', async () => {
    const { wrapper } = mountDetail({
      detail: makeDetail({
        project: makeProject({ deleted_at: '2026-10-29T00:00:00.000Z', deleted_by: 'claude' }),
      }),
    });
    await flush();
    expect(wrapper.text()).toContain('已被 @claude 删除');
    expect(wrapper.text()).toContain('仅数据库记录');
  });

  it('切换 tab 渲染对应子组件（开发脉络 / 改动文件 / 搜索历史 / 验收报告）', async () => {
    const { wrapper } = mountDetail();
    await flush();

    const tab = (label: string) => wrapper.findAll('.pf-tab').find((item: any) => item.text() === label)!;

    await tab('开发脉络').trigger('click');
    await flush();
    expect(wrapper.text()).toContain('b5');
    expect(wrapper.text()).toContain('补充说明');

    await tab('改动文件').trigger('click');
    await flush();
    expect(wrapper.text()).toContain('tools/video.py');
    expect(wrapper.text()).toContain('改动 3 次');

    await tab('搜索历史').trigger('click');
    await flush();
    expect(wrapper.text()).toContain('输入条件后搜索本工程的变动历史。');

    await tab('验收报告').trigger('click');
    await flush();
    expect(wrapper.text()).toContain('尚未提交验收报告。');
  });

  it('概览「查看其改动」跳转搜索历史并带 byMaid 过滤', async () => {
    const searchHistory = vi.fn().mockResolvedValue([]);
    const { wrapper } = mountDetail({ searchHistory });
    await flush();

    await wrapper.find('.pf-contributor .pf-link-btn').trigger('click');
    await flush();

    // 已切到搜索历史 tab 并执行过滤
    expect(wrapper.find('form.pf-filter').exists()).toBe(true);
    expect(searchHistory).toHaveBeenCalledWith(expect.objectContaining({ byMaid: 'claude' }), {
      projectId: 'proj1',
    });
  });

  it('返回键 emit back；刷新按钮强制重读 openProject', async () => {
    const { wrapper } = mountDetail();
    await flush();

    await wrapper.find('.pf-detail-header .pf-icon-btn').trigger('click');
    expect(wrapper.emitted('back')).toBeTruthy();

    const refreshButton = wrapper.findAll('.pf-detail-header .pf-icon-btn')[1];
    await refreshButton.trigger('click');
    await flush();
    expect(state.store.openProject).toHaveBeenCalledWith('proj1', { force: true });
  });

  it('openProject 失败展示错误态并可重试', async () => {
    const openProject = vi.fn().mockRejectedValue(new Error('无法连接桌面桥接'));
    state.store = createMockStore({ currentProjectId: null, detail: null, openProject });
    const { wrapper } = mountWithPinia(PfProjectDetail, { props: { projectId: 'proj1' } });
    await flush();

    expect(wrapper.text()).toContain('无法连接桌面桥接');
    expect(wrapper.text()).toContain('重试');

    await wrapper.findAll('.pf-btn').find((button: any) => button.text() === '重试')!.trigger('click');
    await flush();
    expect(openProject).toHaveBeenCalledTimes(2);
  });
});
