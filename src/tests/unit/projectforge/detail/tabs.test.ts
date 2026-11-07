import { beforeEach, describe, expect, it, vi } from 'vitest';
import PfOverviewTab from '../../../../features/projectforge/components/PfOverviewTab.vue';
import PfTimelineTab from '../../../../features/projectforge/components/PfTimelineTab.vue';
import PfFilesTab from '../../../../features/projectforge/components/PfFilesTab.vue';
import {
  createMockStore,
  flush,
  makeDetail,
  makeFileChange,
  makeHistoryRow,
  makeTodo,
  mountWithPinia,
} from './testUtils';

const state = vi.hoisted(() => ({ store: {} as any }));

vi.mock('../../../../features/projectforge/projectForgeStore', () => ({
  useProjectForgeStore: () => state.store,
}));

function mountTab(component: any, overrides: Record<string, any> = {}) {
  state.store = createMockStore(overrides);
  return mountWithPinia(component);
}

describe('PfOverviewTab — 概览', () => {
  beforeEach(() => {
    state.store = createMockStore();
  });

  it('渲染进度统计卡、Todo 列表与施工角色表', async () => {
    const { wrapper } = mountTab(PfOverviewTab, { detail: makeDetail() });
    await flush();

    // 统计卡：Todo 完成 / 变动节点 / 涉及文件 / 累计增删 / 最近变动
    expect(wrapper.text()).toContain('3/8');
    expect(wrapper.text()).toContain('变动节点');
    expect(wrapper.text()).toContain('14');
    expect(wrapper.text()).toContain('涉及文件');
    expect(wrapper.text()).toContain('+846');
    expect(wrapper.text()).toContain('-73');
    expect(wrapper.text()).toContain('最近变动');

    // Todo 列表
    expect(wrapper.text()).toContain('#1');
    expect(wrapper.text()).toContain('修复上传通道');
    expect(wrapper.text()).toContain('待处理');

    // 施工角色表
    expect(wrapper.text()).toContain('施工角色');
    expect(wrapper.text()).toContain('@claude');
    expect(wrapper.text()).toContain('查看其改动');
  });

  it('Todo 状态操作调用 updateTodos（完成 / 进行中 / 阻塞 / 删除）且成功后通知', async () => {
    const { wrapper, notify } = mountTab(PfOverviewTab, {
      detail: makeDetail({ todos: [makeTodo({ seq: 2, title: '核对 state.json', status: 'pending' })] }),
    });
    await flush();

    const chip = (label: string) =>
      wrapper.findAll('.pf-chip-btn').find((button: any) => button.text().includes(label))!;

    await chip('完成').trigger('click');
    expect(state.store.updateTodos).toHaveBeenLastCalledWith({ done: '2' });

    await chip('进行中').trigger('click');
    expect(state.store.updateTodos).toHaveBeenLastCalledWith({ doing: '2' });

    await chip('阻塞').trigger('click');
    expect(state.store.updateTodos).toHaveBeenLastCalledWith({ blocked: '2' });

    await chip('删除').trigger('click');
    expect(state.store.updateTodos).toHaveBeenLastCalledWith({ remove: '2' });

    expect(notify).toHaveBeenCalled();
  });

  it('勾选快捷按钮按当前状态切换完成 / 待处理', async () => {
    const { wrapper } = mountTab(PfOverviewTab, {
      detail: makeDetail({ todos: [makeTodo({ seq: 1, status: 'pending' })] }),
    });
    await flush();
    await wrapper.find('.pf-todo-check').trigger('click');
    expect(state.store.updateTodos).toHaveBeenLastCalledWith({ done: '1' });

    // 已完成的 Todo 再点一次应回到待处理
    wrapper.unmount();
    const second = mountTab(PfOverviewTab, {
      detail: makeDetail({ todos: [makeTodo({ seq: 1, status: 'done' })] }),
    });
    await flush();
    await second.wrapper.find('.pf-todo-check').trigger('click');
    expect(state.store.updateTodos).toHaveBeenLastCalledWith({ pending: '1' });
  });

  it('新增待办：空内容校验；填写后调用 updateTodos({ add })', async () => {
    const { wrapper } = mountTab(PfOverviewTab, { detail: makeDetail() });
    await flush();

    await wrapper.find('.pf-todo-add .pf-btn').trigger('click');
    expect(wrapper.text()).toContain('请输入待办内容');
    expect(state.store.updateTodos).not.toHaveBeenCalled();

    await wrapper.find('.pf-todo-add-input').setValue('部署到测试机');
    await wrapper.find('.pf-todo-add .pf-btn').trigger('click');
    await flush();
    expect(state.store.updateTodos).toHaveBeenLastCalledWith({ add: '部署到测试机' });
  });

  it('编辑待办：改标题与备注后调用 updateTodos({ seq, title, note })', async () => {
    const { wrapper } = mountTab(PfOverviewTab, { detail: makeDetail() });
    await flush();

    const editButton = wrapper.findAll('.pf-link-btn').find((button: any) => button.text().includes('编辑'))!;
    await editButton.trigger('click');

    await wrapper.find('.pf-todo-edit-input').setValue('新的待办标题');
    await wrapper.find('.pf-todo-edit-note').setValue('补充备注');
    await wrapper.findAll('.pf-todo-actions .pf-btn').find((button: any) => button.text() === '保存')!.trigger('click');
    await flush();

    expect(state.store.updateTodos).toHaveBeenLastCalledWith({
      seq: '1',
      title: '新的待办标题',
      note: '补充备注',
    });
  });

  it('施工角色「查看其改动」emit open-history({ byMaid })', async () => {
    const { wrapper } = mountTab(PfOverviewTab, { detail: makeDetail() });
    await flush();

    await wrapper.find('.pf-contributor .pf-link-btn').trigger('click');
    expect(wrapper.emitted('open-history')?.[0]).toEqual([{ byMaid: 'claude' }]);
  });

  it('Todo / 施工角色为空时展示空态', async () => {
    const { wrapper } = mountTab(PfOverviewTab, {
      detail: makeDetail({ todos: [], contributors: [] }),
    });
    await flush();
    expect(wrapper.text()).toContain('暂无 todo');
    expect(wrapper.text()).toContain('暂无施工角色');
  });
});

describe('PfTimelineTab — 开发脉络', () => {
  beforeEach(() => {
    state.store = createMockStore();
  });

  it('渲染批次 timeline（b5 / 署名 / 时间 / 节点数 / +/- / reason / 文件）', async () => {
    const { wrapper } = mountTab(PfTimelineTab, { detail: makeDetail() });
    await flush();

    expect(wrapper.text()).toContain('b5');
    expect(wrapper.text()).toContain('编辑');
    expect(wrapper.text()).toContain('@claude');
    expect(wrapper.text()).toContain('2 个节点');
    expect(wrapper.text()).toContain('+10');
    expect(wrapper.text()).toContain('-3');
    expect(wrapper.text()).toContain('补充说明');
    expect(wrapper.text()).toContain('tools/video.py');
    expect(wrapper.text()).toContain('tools/run.py');
  });

  it('点击批次加载节点（getBatchNodes），点节点 emit open-diff(node)', async () => {
    const getBatchNodes = vi
      .fn()
      .mockResolvedValue([
        makeHistoryRow({ id: 21, batch_id: 5 }),
        makeHistoryRow({ id: 22, batch_id: 5, file_path: 'tools/run.py', op: 'edit' }),
      ]);
    const { wrapper } = mountTab(PfTimelineTab, { detail: makeDetail(), getBatchNodes });
    await flush();

    await wrapper.find('.pf-batch-head').trigger('click');
    await flush();

    expect(getBatchNodes).toHaveBeenCalledWith('proj1', 5);
    expect(wrapper.text()).toContain('n21');
    expect(wrapper.text()).toContain('n22');
    expect(wrapper.text()).toContain('tools/run.py');

    const nodes = wrapper.findAll('.pf-node');
    expect(nodes.length).toBe(2);
    await nodes[0].trigger('click');
    expect(wrapper.emitted('open-diff')?.[0]).toMatchObject([{ kind: 'node', node: { id: 21 } }]);
  });

  it('无 getBatchNodes 时退化为 searchHistory({ batch }) 过滤', async () => {
    const searchHistory = vi
      .fn()
      .mockResolvedValue([makeHistoryRow({ id: 31, batch_id: 5 }), makeHistoryRow({ id: 32, batch_id: 9 })]);
    const { wrapper } = mountTab(PfTimelineTab, { detail: makeDetail(), searchHistory });
    delete (state.store as any).getBatchNodes;
    await flush();

    await wrapper.find('.pf-batch-head').trigger('click');
    await flush();

    expect(searchHistory).toHaveBeenCalledWith({ batch: 'b5', limit: 500 }, { projectId: 'proj1' });
    expect(wrapper.text()).toContain('n31');
    expect(wrapper.text()).not.toContain('n32');
  });

  it('「查看批次差异」emit open-diff({ kind: batch })', async () => {
    const { wrapper } = mountTab(PfTimelineTab, { detail: makeDetail() });
    await flush();

    await wrapper.find('.pf-batch-tools .pf-link-btn').trigger('click');
    expect(wrapper.emitted('open-diff')?.[0]).toMatchObject([{ kind: 'batch', batch: { id: 5 } }]);
  });

  it('没有改动时展示空态', async () => {
    const { wrapper } = mountTab(PfTimelineTab, { detail: makeDetail({ timeline: [] }) });
    await flush();
    expect(wrapper.text()).toContain('暂无改动');
  });
});

describe('PfFilesTab — 改动文件', () => {
  beforeEach(() => {
    state.store = createMockStore();
  });

  it('渲染文件路径 / 改动次数 / ± / 最近节点', async () => {
    const { wrapper } = mountTab(PfFilesTab, { detail: makeDetail() });
    await flush();

    expect(wrapper.text()).toContain('tools/video.py');
    expect(wrapper.text()).toContain('改动 3 次');
    expect(wrapper.text()).toContain('+12');
    expect(wrapper.text()).toContain('-4');
    expect(wrapper.text()).toContain('n21');
  });

  it('点文件 emit open-history({ file })；点「最近节点差异」emit open-diff(node)', async () => {
    const { wrapper } = mountTab(PfFilesTab, {
      detail: makeDetail({ files: [makeFileChange()] }),
    });
    await flush();

    await wrapper.find('.pf-file-main').trigger('click');
    expect(wrapper.emitted('open-history')?.[0]).toEqual([{ file: 'tools/video.py' }]);

    await wrapper.find('.pf-file .pf-link-btn').trigger('click');
    expect(wrapper.emitted('open-diff')?.[0]).toMatchObject([
      { kind: 'node', node: { id: 21, file_path: 'tools/video.py' } },
    ]);
  });

  it('没有改动文件时展示空态', async () => {
    const { wrapper } = mountTab(PfFilesTab, { detail: makeDetail({ files: [] }) });
    await flush();
    expect(wrapper.text()).toContain('暂无改动文件');
  });
});
