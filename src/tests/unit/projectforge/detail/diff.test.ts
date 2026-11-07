import { beforeEach, describe, expect, it, vi } from 'vitest';
import PfDiffSheet from '../../../../features/projectforge/components/PfDiffSheet.vue';
import {
  createMockStore,
  flush,
  makeBatch,
  makeDetail,
  makeHistoryRow,
  mountWithPinia,
} from './testUtils';

const state = vi.hoisted(() => ({ store: {} as any }));

vi.mock('../../../../features/projectforge/projectForgeStore', () => ({
  useProjectForgeStore: () => state.store,
}));

const DIFF_TEXT = [
  '节点 n25 · tools/video.py',
  '',
  '```diff',
  '--- a/tools/video.py',
  '+++ b/tools/video.py',
  '@@ -1,2 +1,2 @@',
  '-old line',
  '+new line',
  ' context line',
  '```',
].join('\n');

function nodeTarget(id = 25) {
  return { kind: 'node' as const, node: makeHistoryRow({ id, batch_id: 5 }) };
}

function batchTarget() {
  return { kind: 'batch' as const, batch: makeBatch({ id: 5 }) };
}

function mountSheet(overrides: Record<string, any> = {}, props: Record<string, any> = {}) {
  state.store = createMockStore({
    currentProjectId: 'proj1',
    detail: makeDetail(),
    signature: 'claude',
    getNodeDiff: vi.fn().mockResolvedValue({ title: '节点 n25', text: DIFF_TEXT }),
    searchHistory: vi.fn().mockResolvedValue([
      makeHistoryRow({ id: 21, batch_id: 4 }),
      makeHistoryRow({ id: 25, batch_id: 5 }),
    ]),
    ...overrides,
  });
  return mountWithPinia(PfDiffSheet, {
    props: { open: true, projectId: 'proj1', target: nodeTarget(), readonly: false, ...props },
  });
}

describe('PfDiffSheet — 节点 / 批次 diff 与回退', () => {
  beforeEach(() => {
    state.store = createMockStore();
  });

  it('节点 diff：调用 GetNodeDiff(node)，+/- 着色、围栏剥离、hunk 高亮', async () => {
    const { wrapper } = mountSheet();
    await flush();

    expect(state.store.getNodeDiff).toHaveBeenCalledWith('proj1', { node: 25, maxLines: 800 });

    const addLine = wrapper.find('.pf-diff-line.is-add');
    expect(addLine.text()).toContain('+new line');
    const delLine = wrapper.find('.pf-diff-line.is-del');
    expect(delLine.text()).toContain('-old line');
    expect(wrapper.find('.pf-diff-line.is-hunk').text()).toContain('@@');
    // Markdown 标题行按普通文本展示（不参与 +/- 着色）
    expect(wrapper.find('.pf-diff-scroll').text()).toContain('节点 n25');

    // ``` 围栏不应出现在渲染结果里
    expect(wrapper.find('.pf-diff-scroll').text()).not.toContain('```');
  });

  it('批次 diff：调用 GetNodeDiff(batch)，回退栏只提供「撤销此批次」', async () => {
    const { wrapper } = mountSheet({}, { target: batchTarget() });
    await flush();

    expect(state.store.getNodeDiff).toHaveBeenCalledWith('proj1', { batch: 5, maxLines: 800 });
    const buttons = wrapper.findAll('.pf-revert-actions .pf-btn');
    expect(buttons).toHaveLength(1);
    expect(buttons[0].text()).toContain('撤销此批次');
    expect(wrapper.text()).not.toContain('恢复到此版本');
  });

  it('只读工程禁用回退并提示原因', async () => {
    const { wrapper } = mountSheet({}, { readonly: true });
    await flush();

    expect(wrapper.text()).toContain('工程只读或已删除，无法回退');
    const buttons = wrapper.findAll('.pf-revert-actions .pf-btn');
    expect(buttons.length).toBeGreaterThan(0);
    buttons.forEach((button: any) => {
      expect(button.attributes('disabled')).toBeDefined();
    });
  });

  it('未设置署名时禁用回退并提示', async () => {
    const { wrapper } = mountSheet({ signature: '' });
    await flush();

    expect(wrapper.text()).toContain('回退需要署名');
    wrapper.findAll('.pf-revert-actions .pf-btn').forEach((button: any) => {
      expect(button.attributes('disabled')).toBeDefined();
    });
  });

  it('「撤销此变动」二次确认后 rollback(toNode=上一节点, path)，成功后 toast + emit', async () => {
    const { wrapper, notify } = mountSheet({ rollback: vi.fn().mockResolvedValue({ status: 'ok', batchId: 9 }) });
    await flush();

    // searchHistory 反查出同文件上一节点 n21
    expect(state.store.searchHistory).toHaveBeenCalledWith(
      { file: 'tools/video.py', limit: 500 },
      { projectId: 'proj1' },
    );
    expect(wrapper.text()).toContain('该文件上一节点');
    expect(wrapper.text()).toContain('n21');

    const beforeButton = wrapper.findAll('.pf-revert-actions .pf-btn').find((button: any) =>
      button.text().includes('撤销此变动'),
    )!;
    await beforeButton.trigger('click');
    await flush();

    expect(wrapper.text()).toContain('撤销此变动');
    expect(wrapper.text()).toContain('n21');
    expect(wrapper.text()).toContain('@claude');

    const confirmButton = wrapper.findAll('.pf-confirm-actions .pf-btn').find((button: any) =>
      button.text().includes('确认回退'),
    )!;
    await confirmButton.trigger('click');
    await flush();

    expect(state.store.rollback).toHaveBeenCalledWith(
      expect.objectContaining({
        projectId: 'proj1',
        toNode: 21,
        path: 'tools/video.py',
        force: false,
      }),
    );
    expect(wrapper.emitted('rolled-back')).toBeTruthy();
    expect(wrapper.emitted('close')).toBeTruthy();
    expect(notify.mock.calls.some((call: any[]) => String(call[0]?.message || '').includes('已回退'))).toBe(true);
  });

  it('「恢复到此版本」rollback(toNode=该节点, path)', async () => {
    const { wrapper } = mountSheet();
    await flush();

    const afterButton = wrapper.findAll('.pf-revert-actions .pf-btn').find((button: any) =>
      button.text().includes('恢复到此版本'),
    )!;
    await afterButton.trigger('click');
    await flush();

    await wrapper
      .findAll('.pf-confirm-actions .pf-btn')
      .find((button: any) => button.text().includes('确认回退'))!
      .trigger('click');
    await flush();

    expect(state.store.rollback).toHaveBeenCalledWith(
      expect.objectContaining({
        projectId: 'proj1',
        toNode: 25,
        path: 'tools/video.py',
        force: false,
      }),
    );
  });

  it('批次回退 rollback(batch)，并透传回退原因', async () => {
    const { wrapper } = mountSheet({}, { target: batchTarget() });
    await flush();

    await wrapper.find('.pf-revert-reason').setValue('批次实现有问题，整体回退');
    const batchButton = wrapper.findAll('.pf-revert-actions .pf-btn')[0];
    await batchButton.trigger('click');
    await flush();

    expect(wrapper.text()).toContain('撤销整个批次 b5');
    await wrapper
      .findAll('.pf-confirm-actions .pf-btn')
      .find((button: any) => button.text().includes('确认回退'))!
      .trigger('click');
    await flush();

    expect(state.store.rollback).toHaveBeenCalledWith(
      expect.objectContaining({
        projectId: 'proj1',
        batch: 5,
        reason: '批次实现有问题，整体回退',
        force: false,
      }),
    );
  });

  it('冲突时进入强确认，强制回退带 force=true', async () => {
    const rollback = vi
      .fn()
      .mockRejectedValueOnce(new Error('磁盘内容与记录不一致，请加 force=true 强制回退'))
      .mockResolvedValueOnce({ status: 'ok', batchId: 11 });
    const { wrapper } = mountSheet({ rollback });
    await flush();

    const beforeButton = wrapper.findAll('.pf-revert-actions .pf-btn').find((button: any) =>
      button.text().includes('撤销此变动'),
    )!;
    await beforeButton.trigger('click');
    await flush();
    await wrapper
      .findAll('.pf-confirm-actions .pf-btn')
      .find((button: any) => button.text().includes('确认回退'))!
      .trigger('click');
    await flush();

    expect(wrapper.text()).toContain('需要强制回退');
    expect(wrapper.text()).toContain('强制覆盖');

    await wrapper
      .findAll('.pf-confirm-actions .pf-btn')
      .find((button: any) => button.text().includes('强制回退'))!
      .trigger('click');
    await flush();

    expect(rollback).toHaveBeenCalledTimes(2);
    expect(rollback.mock.calls[1][0]).toMatchObject({ toNode: 21, force: true });
    expect(wrapper.emitted('rolled-back')).toBeTruthy();
  });

  it('rollback 返回 noop 时提示无需回退且不 emit rolled-back', async () => {
    const { wrapper, notify } = mountSheet({ rollback: vi.fn().mockResolvedValue({ status: 'noop' }) });
    await flush();

    const beforeButton = wrapper.findAll('.pf-revert-actions .pf-btn').find((button: any) =>
      button.text().includes('撤销此变动'),
    )!;
    await beforeButton.trigger('click');
    await flush();
    await wrapper
      .findAll('.pf-confirm-actions .pf-btn')
      .find((button: any) => button.text().includes('确认回退'))!
      .trigger('click');
    await flush();

    expect(notify.mock.calls.some((call: any[]) => String(call[0]?.message || '').includes('无需回退'))).toBe(true);
    expect(wrapper.emitted('rolled-back')).toBeFalsy();
  });

  it('文件没有更早节点时「撤销此变动」保持禁用', async () => {
    const { wrapper } = mountSheet({
      searchHistory: vi.fn().mockResolvedValue([makeHistoryRow({ id: 25, batch_id: 5 })]),
    });
    await flush();

    const beforeButton = wrapper.findAll('.pf-revert-actions .pf-btn').find((button: any) =>
      button.text().includes('撤销此变动'),
    )!;
    expect(beforeButton.attributes('disabled')).toBeDefined();
    expect(wrapper.text()).toContain('该文件没有更早节点');
  });

  it('工程读写在 diff 加载失败时展示错误态并可重试', async () => {
    const getNodeDiff = vi
      .fn()
      .mockRejectedValueOnce(new Error('读取差异失败：桥接超时'))
      .mockResolvedValueOnce({ title: '节点 n25', text: DIFF_TEXT });
    const { wrapper } = mountSheet({ getNodeDiff });
    await flush();

    expect(wrapper.text()).toContain('桥接超时');
    await wrapper.findAll('.pf-diff-state .pf-btn').find((button: any) => button.text() === '重试')!.trigger('click');
    await flush();
    expect(getNodeDiff).toHaveBeenCalledTimes(2);
    expect(wrapper.findAll('.pf-diff-line').length).toBeGreaterThan(0);
  });
});
