/**
 * T3 只读查看器（PfCodeReader）组件测试。
 *
 * 覆盖：行号/等宽渲染、自动换行开关、复制路径、符号大纲点击跳转行、
 * FindSymbol 结果展示与跳行、Trace 结果、编辑/删除/移动/返回事件。
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

import PfCodeReader from '../../../../features/projectforge/components/PfCodeReader.vue';
import {
  __resetProjectForgeSourceForTests,
  useProjectForgeSource,
} from '../../../../features/projectforge/useProjectForgeSource';
import { installSourceBridge } from './sourceTestUtils';

async function flush(times = 8): Promise<void> {
  for (let index = 0; index < times; index += 1) {
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    await Promise.resolve();
  }
}

async function setupReader(options: Parameters<typeof installSourceBridge>[1] = {}) {
  installSourceBridge(apiMock, options);
  const source = useProjectForgeSource();
  await source.initSource();
  await source.openFile('src/main.ts');
  const wrapper = mount(PfCodeReader);
  await flush(2);
  return { wrapper, source };
}

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  __resetProjectForgeSourceForTests();
  // happy-dom 没有实现 scrollIntoView
  Element.prototype.scrollIntoView = vi.fn();
});

describe('PfCodeReader 基础渲染', () => {
  it('展示路径、元信息、行号与等宽正文', async () => {
    const { wrapper } = await setupReader();
    expect(wrapper.find('.pf-reader-path').text()).toBe('src/main.ts');
    expect(wrapper.find('.pf-reader-meta').text()).toContain('2 行');
    expect(wrapper.find('.pf-reader-meta').text()).toContain('TypeScript');
    expect(wrapper.findAll('.pf-code-gutter').map((node) => node.text())).toEqual(['1', '2']);
    expect(wrapper.findAll('.pf-code-text').map((node) => node.text())).toEqual(['export const a = 1;', '']);
    wrapper.unmount();
  });

  it('自动换行开关切换样式类并持久化', async () => {
    const { wrapper } = await setupReader();
    expect(wrapper.find('.pf-code-scroll').classes()).not.toContain('pf-code-wrap');
    await wrapper.find('[aria-label="自动换行"]').trigger('click');
    expect(wrapper.find('.pf-code-scroll').classes()).toContain('pf-code-wrap');
    expect(localStorage.getItem('vcp.projectforge.source.wrap')).toBe('1');
    await wrapper.find('[aria-label="自动换行"]').trigger('click');
    expect(wrapper.find('.pf-code-scroll').classes()).not.toContain('pf-code-wrap');
    wrapper.unmount();
  });

  it('复制路径写入剪贴板并给出提示', async () => {
    const { wrapper } = await setupReader();
    const writeText = vi.mocked(navigator.clipboard.writeText);
    writeText.mockClear();
    await wrapper.find('[aria-label="复制路径"]').trigger('click');
    await flush(2);
    expect(writeText).toHaveBeenCalledWith('src/main.ts');
    expect(wrapper.text()).toContain('已复制路径');
    wrapper.unmount();
  });
});

describe('PfCodeReader 符号工具', () => {
  it('加载符号大纲后点击条目跳转到对应行并高亮', async () => {
    const { wrapper } = await setupReader();
    await wrapper.findAll('.pf-tool-btn').find((button) => button.text() === '大纲')!.trigger('click');
    await flush(4);
    const rows = wrapper.findAll('.pf-outline-row');
    expect(rows).toHaveLength(1);
    expect(rows[0].text()).toContain('a');
    expect(rows[0].text()).toContain('L1');
    await rows[0].trigger('click');
    await flush(2);
    expect(wrapper.find('.pf-code-line[data-line="1"]').classes()).toContain('pf-code-line-hit');
    wrapper.unmount();
  });

  it('FindSymbol 查询展示命中并支持点击跳行', async () => {
    const { wrapper } = await setupReader();
    await wrapper.findAll('.pf-tool-btn').find((button) => button.text() === '符号')!.trigger('click');
    await wrapper.find('.pf-tool-input').setValue('a');
    await wrapper.find('.pf-tool-form').trigger('submit');
    await flush(4);
    const hits = wrapper.findAll('.pf-symbol-row');
    expect(hits).toHaveLength(1);
    expect(hits[0].text()).toContain('a');
    expect(hits[0].text()).toContain('src/main.ts:1');
    expect(wrapper.text()).toContain('扫描 3 个文件');
    await hits[0].trigger('click');
    await flush(2);
    expect(wrapper.find('.pf-code-line[data-line="1"]').classes()).toContain('pf-code-line-hit');
    wrapper.unmount();
  });

  it('Trace 查询展示链路文本', async () => {
    const { wrapper } = await setupReader();
    await wrapper.findAll('.pf-tool-btn').find((button) => button.text() === '链路')!.trigger('click');
    await wrapper.find('.pf-tool-input').setValue('ipc:demo:invoke');
    await wrapper.find('.pf-tool-form').trigger('submit');
    await flush(4);
    expect(wrapper.find('.pf-trace-text').text()).toContain('主进程');
    wrapper.unmount();
  });
});

describe('PfCodeReader 事件', () => {
  it('编辑 / 删除 / 移动 / 返回按钮抛出对应事件', async () => {
    const { wrapper } = await setupReader();
    await wrapper.findAll('.pf-reader-actions button').find((button) => button.text().includes('编辑'))!.trigger('click');
    await wrapper.findAll('.pf-reader-actions button').find((button) => button.text().includes('移动'))!.trigger('click');
    await wrapper.findAll('.pf-reader-actions button').find((button) => button.text().includes('删除'))!.trigger('click');
    await wrapper.find('.pf-reader-head .pf-icon-btn').trigger('click');
    expect(wrapper.emitted('edit')).toHaveLength(1);
    expect(wrapper.emitted('move')).toHaveLength(1);
    expect(wrapper.emitted('delete')).toHaveLength(1);
    expect(wrapper.emitted('back')).toHaveLength(1);
    wrapper.unmount();
  });

  it('FindSymbol 命中其它文件时抛出 open-file 事件', async () => {
    installSourceBridge(apiMock);
    const source = useProjectForgeSource();
    await source.initSource();
    await source.openFile('src/main.ts');
    source.findSymbolHits.value = [{ path: 'src/other.ts', name: 'b', start: 3, raw: 'src/other.ts#b' }];
    const wrapper = mount(PfCodeReader);
    await flush(2);
    await wrapper.findAll('.pf-tool-btn').find((button) => button.text() === '符号')!.trigger('click');
    await wrapper.findAll('.pf-symbol-row')[0].trigger('click');
    expect(wrapper.emitted('open-file')?.[0]).toEqual(['src/other.ts']);
    wrapper.unmount();
  });
});
