/**
 * T3 源码模式 shell（PfSourcePanel）组件测试。
 *
 * 走真实子组件（PfFileTree / PfCodeReader / PfCodeEditor），bridge 全部 mock：
 * 覆盖初始化树、打开文件、编辑→脏→未保存离开确认、搜索命中打开、删除/移动强确认流程、
 * 新建文件流程、FileOperator 降级提示。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, type VueWrapper } from '@vue/test-utils';

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

import PfSourcePanel from '../../../../features/projectforge/components/PfSourcePanel.vue';
import { __resetProjectForgeSourceForTests } from '../../../../features/projectforge/useProjectForgeSource';
import { installSourceBridge } from './sourceTestUtils';

function buttonByText(wrapper: VueWrapper, text: string) {
  return wrapper.findAll('button').find((button) => button.text().includes(text));
}

async function flush(times = 12): Promise<void> {
  for (let index = 0; index < times; index += 1) {
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    await Promise.resolve();
  }
}

async function mountPanel(options: Parameters<typeof installSourceBridge>[1] = {}) {
  const state = installSourceBridge(apiMock, options);
  const wrapper = mount(PfSourcePanel, { props: { isOpen: true } });
  await flush();
  return { wrapper, state };
}

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  __resetProjectForgeSourceForTests();
});

describe('PfSourcePanel 文件树与打开文件', () => {
  it('初始化后展示工作区/工程选择、根目录与文件树', async () => {
    const { wrapper } = await mountPanel();
    const text = wrapper.text();
    expect(text).toContain('工程区');
    expect(text).toContain('测试工程');
    expect(text).toContain('/home/user/proj/demo');
    expect(text).toContain('FileOperator');
    const fileRows = wrapper.findAll('.pf-tree-row-file');
    expect(fileRows.map((row) => row.attributes('data-path'))).toEqual(['README.md']);
    expect(wrapper.findAll('.pf-tree-row-dir').map((row) => row.attributes('data-dir'))).toEqual(['src']);
    wrapper.unmount();
  });

  it('展开目录懒加载子文件，点文件进入只读查看（行号 + 内容）', async () => {
    const { wrapper } = await mountPanel();
    const dirRow = wrapper.find('.pf-tree-row-dir[data-dir="src"]');
    await dirRow.trigger('click');
    await flush();
    const child = wrapper.find('.pf-tree-row-file[data-path="src/main.ts"]');
    expect(child.exists()).toBe(true);

    await child.trigger('click');
    await flush();
    expect(wrapper.find('.pf-reader').exists()).toBe(true);
    expect(wrapper.find('.pf-reader-path').text()).toBe('src/main.ts');
    expect(wrapper.find('.pf-code-line .pf-code-gutter').text()).toBe('1');
    expect(wrapper.find('.pf-code-line .pf-code-text').text()).toBe('export const a = 1;');
    wrapper.unmount();
  });

  it('文件名搜索命中后点击结果打开文件（Enter 立即搜索）', async () => {
    const { wrapper } = await mountPanel();
    const input = wrapper.find('.pf-search-input');
    await input.setValue('main');
    await input.trigger('keydown.enter');
    await flush();
    const hit = wrapper.find('.pf-tree-row-file[data-path="src/main.ts"]');
    expect(hit.exists()).toBe(true);
    await hit.trigger('click');
    await flush();
    expect(wrapper.find('.pf-reader-path').text()).toBe('src/main.ts');
    wrapper.unmount();
  });

  it('FileOperator 异常时使用兼容模式并显示中性提示（不显示原始错误）', async () => {
    const { wrapper } = await mountPanel({ fileOperatorFails: true });
    const text = wrapper.text();
    expect(text).toContain('兼容模式（PTY）');
    expect(text).toContain('兼容模式');
    expect(text).not.toContain('Access denied');
    expect(text).not.toContain('降级');
    expect(wrapper.find('.pf-tree-row-dir[data-dir="src"]').exists()).toBe(true);
    wrapper.unmount();
  });

  it('真实 FileOperator 返回形态下树行按 items 正确渲染（目录/文件/大小）', async () => {
    const { wrapper } = await mountPanel({ fileOperatorRealShape: true });
    const text = wrapper.text();
    expect(text).toContain('文件树：FileOperator');
    const dirRows = wrapper.findAll('.pf-tree-row-dir');
    expect(dirRows.map((row) => row.attributes('data-dir'))).toEqual(['src']);
    const fileRows = wrapper.findAll('.pf-tree-row-file');
    expect(fileRows.map((row) => row.attributes('data-path'))).toEqual(['README.md']);
    expect(wrapper.text()).not.toContain('| 文件名称');
    wrapper.unmount();
  });

  it('工程根目录不在白名单时静默使用兼容模式（不显示错误提示条）', async () => {
    const { wrapper } = await mountPanel({ fileOperatorNotAllowed: true });
    const text = wrapper.text();
    expect(text).toContain('兼容模式（PTY）');
    expect(wrapper.find('.pf-banner-warn').exists()).toBe(false);
    expect(wrapper.find('.pf-banner-error').exists()).toBe(false);
    expect(text).not.toContain('Access denied');
    expect(wrapper.find('.pf-tree-row-dir[data-dir="src"]').exists()).toBe(true);
    wrapper.unmount();
  });
});

describe('PfSourcePanel 编辑与未保存离开确认', () => {
  it('编辑后返回会拦截并弹出未保存确认；取消保留编辑内容，确认放弃回到只读', async () => {
    const { wrapper } = await mountPanel();
    await wrapper.find('.pf-tree-row-file[data-path="README.md"]').trigger('click');
    await flush();
    await buttonByText(wrapper, '编辑')!.trigger('click');
    await flush();
    expect(wrapper.find('.pf-editor').exists()).toBe(true);

    const textarea = wrapper.find('textarea.pf-editor-textarea');
    expect((textarea.element as HTMLTextAreaElement).value).toBe('# 测试\n');
    await textarea.setValue('# 修改后的标题\n');
    expect(wrapper.find('.pf-editor-state').text()).toContain('未保存修改');

    await wrapper.find('.pf-editor-head .pf-icon-btn').trigger('click');
    await flush();
    expect(wrapper.text()).toContain('放弃未保存的修改');
    // 取消：仍在编辑器，草稿保留
    await buttonByText(wrapper, '取消')!.trigger('click');
    await flush();
    expect(wrapper.find('.pf-editor').exists()).toBe(true);
    expect((wrapper.find('textarea.pf-editor-textarea').element as HTMLTextAreaElement).value).toBe('# 修改后的标题\n');

    // 再次返回 → 放弃修改
    await wrapper.find('.pf-editor-head .pf-icon-btn').trigger('click');
    await flush();
    await buttonByText(wrapper, '放弃修改')!.trigger('click');
    await flush();
    expect(wrapper.find('.pf-editor').exists()).toBe(false);
    expect(wrapper.find('.pf-reader').exists()).toBe(true);
    // 只读视图仍是磁盘原文
    expect(wrapper.text()).toContain('# 测试');
    wrapper.unmount();
  });
});

describe('PfSourcePanel 删除 / 移动 / 新建', () => {
  it('删除文件需填写原因并走强确认，成功后回读校验并返回树', async () => {
    const { wrapper, state } = await mountPanel();
    await wrapper.find('.pf-tree-row-file[data-path="README.md"]').trigger('click');
    await flush();
    await buttonByText(wrapper, '删除')!.trigger('click');
    await flush();

    expect(wrapper.text()).toContain('删除文件');
    const confirmButton = wrapper.find('.pf-dialog-danger');
    expect(confirmButton.attributes('disabled')).toBeDefined();
    await wrapper.find('#pf-delete-reason').setValue('清理旧说明');
    await wrapper.find('.pf-dialog-danger').trigger('click');
    await flush();

    expect(state.removeCalls).toHaveLength(1);
    expect(state.removeCalls[0]).toMatchObject({ path: 'README.md', reason: '清理旧说明' });
    expect(wrapper.find('.pf-reader').exists()).toBe(false);
    expect(wrapper.find('.pf-source-head').exists()).toBe(true);
    expect(wrapper.find('.pf-tree-row-file[data-path="README.md"]').exists()).toBe(false);
    expect(wrapper.find('.pf-tree-row-dir[data-dir="src"]').exists()).toBe(true);
    wrapper.unmount();
  });

  it('移动/重命名需填目标路径与原因，成功后打开新路径', async () => {
    const { wrapper, state } = await mountPanel();
    await wrapper.find('.pf-tree-row-file[data-path="README.md"]').trigger('click');
    await flush();
    await buttonByText(wrapper, '移动')!.trigger('click');
    await flush();
    expect(wrapper.text()).toContain('移动 / 重命名文件');

    await wrapper.find('#pf-move-target').setValue('docs/README.md');
    await wrapper.find('#pf-move-reason').setValue('整理文档目录');
    await wrapper.find('.pf-dialog-primary').trigger('click');
    await flush();

    expect(state.moveCalls).toHaveLength(1);
    expect(state.moveCalls[0]).toMatchObject({ from: 'README.md', to: 'docs/README.md', reason: '整理文档目录' });
    expect(wrapper.find('.pf-reader-path').text()).toBe('docs/README.md');
    wrapper.unmount();
  });

  it('新建文件：路径 + 内容 + 原因 + 强确认后 CreateFile 并进入只读', async () => {
    const { wrapper, state } = await mountPanel();
    await wrapper.find('.pf-new-btn').trigger('click');
    await flush();
    expect(wrapper.find('#pf-create-path-input').exists()).toBe(true);

    await wrapper.find('#pf-create-path-input').setValue('src/added.ts');
    await wrapper.find('textarea.pf-editor-textarea').setValue('export const b = 2;\n');
    await wrapper.find('.pf-save-btn').trigger('click');
    await flush();
    expect(wrapper.text()).toContain('确认新建文件');
    await wrapper.find('#pf-write-reason').setValue('新增文件');
    await wrapper.find('.pf-confirm-actions .pf-action-primary').trigger('click');
    await flush();

    expect(state.createCalls).toHaveLength(1);
    expect(state.createCalls[0]).toMatchObject({ path: 'src/added.ts', content: 'export const b = 2;\n', reason: '新增文件' });
    expect(wrapper.find('.pf-reader-path').text()).toBe('src/added.ts');
    expect(wrapper.text()).toContain('export const b = 2;');
    wrapper.unmount();
  });
});
