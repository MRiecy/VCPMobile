import { beforeEach, describe, expect, it, vi } from 'vitest';
import PfProjectList from '../../../../features/projectforge/components/PfProjectList.vue';
import { createMockStore, flush, makeProject, mountWithPinia } from './testUtils';

const state = vi.hoisted(() => ({ store: {} as any }));

vi.mock('../../../../features/projectforge/projectForgeStore', () => ({
  useProjectForgeStore: () => state.store,
}));

function mountList(overrides: Record<string, any> = {}) {
  state.store = createMockStore(overrides);
  return mountWithPinia(PfProjectList);
}

describe('PfProjectList — 工程列表', () => {
  beforeEach(() => {
    state.store = createMockStore();
  });

  it('渲染工程名称 / ID / 状态徽标 / 工作区 / Todo 进度 / 变动次数，点击 emit select', async () => {
    const { wrapper } = mountList({ projects: [makeProject()] });
    await flush();

    expect(wrapper.text()).toContain('示例工程 A');
    expect(wrapper.text()).toContain('proj1');
    expect(wrapper.text()).toContain('待验收');
    expect(wrapper.text()).toContain('ai打工人');
    expect(wrapper.text()).toContain('Todo 3/8');
    expect(wrapper.text()).toContain('14 次变动');

    const progressFill = wrapper.find('.pf-progress-fill');
    expect(progressFill.attributes('style')).toContain('width: 38%');

    await wrapper.find('.pf-project-main').trigger('click');
    expect(wrapper.emitted('select')?.[0]).toEqual(['proj1']);
  });

  it('搜索框本地即时过滤（名称 / ID / 施工署名）并在防抖后调用 searchProjects', async () => {
    const { wrapper } = mountList({
      projects: [
        makeProject({ id: 'proj1', name: '示例工程 A', maids: ['claude'] }),
        makeProject({ id: 'other1', name: '另一个工程', maids: ['gemini'] }),
      ],
    });
    await flush();

    const input = wrapper.find('.pf-search-input');
    await input.setValue('claude');
    expect(wrapper.text()).toContain('示例工程 A');
    expect(wrapper.text()).not.toContain('另一个工程');

    await input.setValue('other1');
    expect(wrapper.text()).toContain('另一个工程');
    expect(wrapper.text()).not.toContain('示例工程 A');

    // 防抖 300ms 后触发服务端搜索（按名称/ID）
    await new Promise((resolve) => setTimeout(resolve, 360));
    await flush();
    expect(state.store.searchProjects).toHaveBeenLastCalledWith('other1');
  });

  it('「含已删除」开关按 includeDeleted 重新拉取，并显示已删除工程的恢复 / 永久清除', async () => {
    const { wrapper } = mountList({
      projects: [
        makeProject({ id: 'accepted1', name: '已验收工程', status: 'accepted' }),
        makeProject({ id: 'deleted1', name: '已删除工程', deleted_at: '2026-10-29T00:00:00.000Z', deleted_by: 'claude' }),
        makeProject({ id: 'active1', name: '施工中工程', status: 'active' }),
      ],
    });
    await flush();

    // 已验收 → 删除按钮；已删除 → 恢复 / 永久清除；施工中 → 无操作按钮
    const items = wrapper.findAll('.pf-project');
    expect(items[0].find('.pf-action-btn').text()).toContain('删除');
    expect(items[1].text()).toContain('恢复');
    expect(items[1].text()).toContain('永久清除');
    expect(items[2].findAll('.pf-action-btn')).toHaveLength(0);
    expect(wrapper.text()).toContain('已删除');

    await wrapper.find('.pf-include-deleted input').setValue(true);
    await flush();
    expect(state.store.loadProjects).toHaveBeenCalledWith({ includeDeleted: true });
  });

  it('新建工程：校验必填，提交时带上名称 / 工作区 / 子目录 / todo，成功后 emit select', async () => {
    state.store = createMockStore({
      projects: [],
      workspaces: [{ id: 'ws1', alias: 'ai打工人', path: '/home/ai', enabled: true }],
      createProject: vi.fn().mockResolvedValue({ id: 'new-1', name: '全新工程' }),
      loadProjects: vi.fn().mockResolvedValue(undefined),
    });
    const { wrapper } = mountWithPinia(PfProjectList);
    await flush();

    await wrapper.find('.pf-new-btn').trigger('click');
    await wrapper.find('form.pf-create').trigger('submit');
    await flush();
    expect(wrapper.text()).toContain('请填写工程名称');
    expect(state.store.createProject).not.toHaveBeenCalled();

    const inputs = wrapper.findAll('.pf-field input');
    await inputs[0].setValue('全新工程');
    await inputs[1].setValue('sub-dir');
    await wrapper.find('.pf-field textarea').setValue('任务A, 任务B');
    await wrapper.find('form.pf-create').trigger('submit');
    await flush();

    expect(state.store.createProject).toHaveBeenCalledWith({
      name: '全新工程',
      workspace: undefined,
      dir: 'sub-dir',
      todos: '任务A, 任务B',
    });
    expect(wrapper.emitted('select')?.[0]).toEqual(['new-1']);
  });

  it('删除已验收工程需要二次确认，确认后调用 deleteProjects', async () => {
    const { wrapper } = mountList({
      projects: [makeProject({ id: 'p1', status: 'accepted' })],
    });
    await flush();

    await wrapper.find('.pf-action-btn').trigger('click');
    expect(wrapper.text()).toContain('仅删除数据库记录');
    expect(wrapper.text()).toContain('删除工程');

    const confirmButtons = wrapper.findAll('.pf-confirm-actions .pf-btn');
    await confirmButtons[1].trigger('click');
    await flush();
    expect(state.store.deleteProjects).toHaveBeenCalledWith(['p1']);
  });

  it('永久清除已删除工程需要危险确认，恢复则直接调用 restoreProjects', async () => {
    const { wrapper } = mountList({
      projects: [makeProject({ id: 'gone', deleted_at: '2026-10-29T00:00:00.000Z' })],
    });
    await flush();

    const actionButtons = wrapper.findAll('.pf-action-btn');
    // 恢复
    await actionButtons[0].trigger('click');
    await flush();
    expect(state.store.restoreProjects).toHaveBeenCalledWith(['gone']);

    // 永久清除 → 危险确认
    await wrapper.findAll('.pf-action-btn')[1].trigger('click');
    expect(wrapper.text()).toContain('不可撤销');
    const confirmButtons = wrapper.findAll('.pf-confirm-actions .pf-btn');
    await confirmButtons[1].trigger('click');
    await flush();
    expect(state.store.purgeProjects).toHaveBeenCalledWith(['gone']);
  });

  it('空态 / 错误态 / 加载态', async () => {
    // 空态
    let mounted = mountList({ projects: [] });
    await flush();
    expect(mounted.wrapper.text()).toContain('没有工程');

    // 错误态
    mounted = mountList({ projects: [], listError: '无法连接桌面桥接（网络不可达）' });
    await flush();
    expect(mounted.wrapper.text()).toContain('无法连接桌面桥接（网络不可达）');
    expect(mounted.wrapper.text()).toContain('重试');

    // 加载态
    mounted = mountList({
      projects: [],
      loading: { workspaces: false, projects: true, detail: false, history: false, action: false },
    });
    expect(mounted.wrapper.text()).toContain('正在加载工程…');
  });
});
