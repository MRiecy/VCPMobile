/**
 * T3 编辑器（PfCodeEditor）组件测试。
 *
 * 覆盖：textarea 行号/脏标记、保存强确认（reason 必填 → EditCode）、保存失败回读错误展示、
 * Ctrl+S 打开确认、新建文件（CreateFile）流程、删除/移动事件、defineExpose 的 isDirty/resetDraft。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';

const apiMock = vi.hoisted(() => ({
  pfInvoke: vi.fn(),
  pfInvokeEnvelope: vi.fn(),
  pluginInvoke: vi.fn(),
  ptyExec: vi.fn(),
}));

vi.mock('../../../../features/projectforge/projectForgeApi', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../../../features/projectforge/projectForgeApi')>();
  return { ...actual, ...apiMock };
});

import PfCodeEditor from '../../../../features/projectforge/components/PfCodeEditor.vue';
import {
  __resetProjectForgeSourceForTests,
  useProjectForgeSource,
} from '../../../../features/projectforge/useProjectForgeSource';
import { installSourceBridge } from './sourceTestUtils';

interface EditorExposed {
  isDirty: () => boolean;
  resetDraft: () => void;
}

async function flush(times = 8): Promise<void> {
  for (let index = 0; index < times; index += 1) {
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    await Promise.resolve();
  }
}

async function setupEditor(
  mode: 'edit' | 'create' = 'edit',
  options: Parameters<typeof installSourceBridge>[1] = {},
) {
  const state = installSourceBridge(apiMock, options);
  const source = useProjectForgeSource();
  await source.initSource();
  if (mode === 'edit') await source.openFile('src/main.ts');
  const wrapper = mount(PfCodeEditor, { props: { mode } });
  await flush(2);
  return { wrapper, source, state };
}

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  __resetProjectForgeSourceForTests();
});

describe('PfCodeEditor 编辑与保存', () => {
  it('初始化显示磁盘内容与行号，修改后出现脏标记', async () => {
    const { wrapper } = await setupEditor();
    const textarea = wrapper.find('textarea.pf-editor-textarea');
    expect((textarea.element as HTMLTextAreaElement).value).toBe('export const a = 1;\n');
    expect(wrapper.findAll('.pf-gutter-line').map((node) => node.text())).toEqual(['1', '2']);
    expect(wrapper.find('.pf-editor-state').text()).toContain('已保存');

    await textarea.setValue('export const a = 2;\n');
    expect(wrapper.find('.pf-editor-state').text()).toContain('未保存修改');
    expect((wrapper.vm as unknown as EditorExposed).isDirty()).toBe(true);
    wrapper.unmount();
  });

  it('保存需强确认 + 必填 reason，确认后走 EditCode 并回读校验', async () => {
    const { wrapper, state } = await setupEditor();
    await wrapper.find('textarea.pf-editor-textarea').setValue('export const a = 2;\n');
    await wrapper.find('.pf-save-btn').trigger('click');
    await flush(2);

    expect(wrapper.text()).toContain('确认保存文件');
    expect(wrapper.text()).toContain('EditCode');
    const confirmButton = wrapper.find('.pf-confirm-actions .pf-action-primary');
    expect(confirmButton.attributes('disabled')).toBeDefined();
    expect(confirmButton.text()).toContain('确认保存');

    await wrapper.find('#pf-write-reason').setValue('修正常量值');
    await confirmButton.trigger('click');
    await flush(4);

    expect(state.editCalls[0]).toMatchObject({
      projectId: 'p1',
      path: 'src/main.ts',
      reason: '修正常量值',
      target: 'export const a = 1;\n',
      replace: 'export const a = 2;\n',
    });
    expect(wrapper.find('.pf-confirm-card').exists()).toBe(false);
    expect(wrapper.find('.pf-editor-state').text()).toContain('已保存');
    expect(wrapper.emitted('saved')?.[0]).toEqual(['src/main.ts']);
    wrapper.unmount();
  });

  it('保存失败（回读不一致）在确认弹层内展示错误，且不关闭弹层', async () => {
    const { wrapper } = await setupEditor('edit', { skipApplyEdits: true });
    await wrapper.find('textarea.pf-editor-textarea').setValue('export const a = 2;\n');
    await wrapper.find('.pf-save-btn').trigger('click');
    await flush(2);
    await wrapper.find('#pf-write-reason').setValue('修正常量值');
    await wrapper.find('.pf-confirm-actions .pf-action-primary').trigger('click');
    await flush(4);

    expect(wrapper.find('.pf-confirm-card').exists()).toBe(true);
    expect(wrapper.find('.pf-confirm-error').text()).toContain('回读校验失败');
    wrapper.unmount();
  });

  it('Ctrl+S 打开保存确认弹层', async () => {
    const { wrapper } = await setupEditor();
    await wrapper.find('textarea.pf-editor-textarea').setValue('changed\n');
    await wrapper.find('textarea.pf-editor-textarea').trigger('keydown', { key: 's', ctrlKey: true });
    await flush(2);
    expect(wrapper.find('.pf-confirm-card').exists()).toBe(true);
    expect(wrapper.text()).toContain('确认保存文件');
    wrapper.unmount();
  });

  it('resetDraft 丢弃修改并恢复磁盘内容', async () => {
    const { wrapper } = await setupEditor();
    const textarea = wrapper.find('textarea.pf-editor-textarea');
    await textarea.setValue('dirty content');
    const vm = wrapper.vm as unknown as EditorExposed;
    expect(vm.isDirty()).toBe(true);
    vm.resetDraft();
    await flush(2);
    expect((textarea.element as HTMLTextAreaElement).value).toBe('export const a = 1;\n');
    expect(vm.isDirty()).toBe(false);
    wrapper.unmount();
  });
});

describe('PfCodeEditor 新建文件', () => {
  it('填写路径/内容/原因并确认后 CreateFile，抛出 created 事件', async () => {
    const { wrapper, state } = await setupEditor('create');
    expect(wrapper.text()).toContain('新建文件');
    await wrapper.find('#pf-create-path-input').setValue('src/created.ts');
    await wrapper.find('textarea.pf-editor-textarea').setValue('export const c = 3;\n');
    await wrapper.find('.pf-save-btn').trigger('click');
    await flush(2);

    expect(wrapper.text()).toContain('确认新建文件');
    await wrapper.find('#pf-write-reason').setValue('新增工具文件');
    await wrapper.find('.pf-confirm-actions .pf-action-primary').trigger('click');
    await flush(4);

    expect(state.createCalls[0]).toMatchObject({
      projectId: 'p1',
      path: 'src/created.ts',
      content: 'export const c = 3;\n',
      reason: '新增工具文件',
    });
    expect(wrapper.emitted('created')?.[0]).toEqual(['src/created.ts']);
    wrapper.unmount();
  });

  it('未填路径时点新建给出校验提示', async () => {
    const { wrapper } = await setupEditor('create');
    await wrapper.find('.pf-save-btn').trigger('click');
    await flush(2);
    expect(wrapper.find('.pf-editor-banner').text()).toContain('请填写新文件路径');
    expect(wrapper.find('.pf-confirm-card').exists()).toBe(false);
    wrapper.unmount();
  });
});

describe('PfCodeEditor 事件', () => {
  it('编辑模式下删除 / 移动按钮抛出事件', async () => {
    const { wrapper } = await setupEditor();
    await wrapper.findAll('.pf-editor-actions button').find((button) => button.text().includes('移动'))!.trigger('click');
    await wrapper.findAll('.pf-editor-actions button').find((button) => button.text().includes('删除'))!.trigger('click');
    expect(wrapper.emitted('move')).toHaveLength(1);
    expect(wrapper.emitted('delete')).toHaveLength(1);
    wrapper.unmount();
  });
});
