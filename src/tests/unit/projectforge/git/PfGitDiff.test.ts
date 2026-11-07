/**
 * `PfGitDiff.vue` 渲染单测：unified diff 着色/行号、加载/错误/空态、自动换行开关。
 */
import { beforeEach, describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import PfGitDiff from '../../../../features/projectforge/components/PfGitDiff.vue';
import { SAMPLE_DIFF } from './fixtures';

const WRAP_KEY = 'vcp.projectForge.git.diffWrap.v1';

beforeEach(() => {
  window.localStorage.clear();
});

describe('PfGitDiff 渲染', () => {
  it('按行类型渲染新增/删除/上下文/hunk/file 并显示新旧行号', () => {
    const wrapper = mount(PfGitDiff, {
      props: { text: SAMPLE_DIFF, title: 'src/app.ts', groupLabel: '更改' },
    });

    expect(wrapper.find('.pf-git-diff-title').text()).toBe('src/app.ts');
    expect(wrapper.find('.pf-git-diff-group').text()).toBe('更改');
    expect(wrapper.findAll('.pf-git-diff-line.is-file')).toHaveLength(1);
    expect(wrapper.findAll('.pf-git-diff-line.is-hunk')).toHaveLength(1);
    expect(wrapper.findAll('.pf-git-diff-line.is-add')).toHaveLength(2);
    expect(wrapper.findAll('.pf-git-diff-line.is-del')).toHaveLength(1);
    expect(wrapper.findAll('.pf-git-diff-line.is-context')).toHaveLength(2);

    // 新增行只显示新行号：hunk 起点 +1，第一行上下文后新增行从 2、3 开始
    const addNumbers = wrapper
      .findAll('.pf-git-diff-line.is-add')
      .map((line) => line.findAll('.pf-git-diff-ln').map((cell) => cell.text()));
    expect(addNumbers).toEqual([
      ['', '2'],
      ['', '3'],
    ]);
    // 删除行只显示旧行号 2
    const delNumbers = wrapper
      .find('.pf-git-diff-line.is-del')
      .findAll('.pf-git-diff-ln')
      .map((cell) => cell.text());
    expect(delNumbers).toEqual(['2', '']);

    expect(wrapper.html()).toContain('const b = 3;');
  });

  it('空差异显示提示文案', () => {
    const wrapper = mount(PfGitDiff, { props: { text: '' } });
    expect(wrapper.find('.pf-git-diff-state').text()).toContain('没有可显示的差异');
  });

  it('自定义空态提示优先展示', () => {
    const wrapper = mount(PfGitDiff, {
      props: { text: '', emptyHint: '未跟踪的新文件没有文本差异。' },
    });
    expect(wrapper.find('.pf-git-diff-state').text()).toContain('未跟踪的新文件没有文本差异');
  });

  it('加载态与错误态', async () => {
    const wrapper = mount(PfGitDiff, { props: { loading: true } });
    expect(wrapper.find('.pf-git-diff-state').text()).toContain('正在读取差异');
    await wrapper.setProps({ loading: false, error: '读取差异失败：timeout' });
    expect(wrapper.find('.pf-git-diff-state.error').text()).toContain('读取差异失败');
  });

  it('自动换行开关切换 is-wrap 并持久化偏好', async () => {
    const wrapper = mount(PfGitDiff, { props: { text: SAMPLE_DIFF } });
    const body = wrapper.find('.pf-git-diff-body');
    expect(body.classes()).not.toContain('is-wrap');

    await wrapper.find('.pf-git-diff-wrap-btn').trigger('click');
    expect(wrapper.find('.pf-git-diff-body').classes()).toContain('is-wrap');
    expect(window.localStorage.getItem(WRAP_KEY)).toBe('1');
    expect(wrapper.find('.pf-git-diff-wrap-btn').attributes('aria-pressed')).toBe('true');

    await wrapper.find('.pf-git-diff-wrap-btn').trigger('click');
    expect(wrapper.find('.pf-git-diff-body').classes()).not.toContain('is-wrap');
    expect(window.localStorage.getItem(WRAP_KEY)).toBe('0');
  });

  it('记住上次的换行偏好', () => {
    window.localStorage.setItem(WRAP_KEY, '1');
    const wrapper = mount(PfGitDiff, { props: { text: SAMPLE_DIFF } });
    expect(wrapper.find('.pf-git-diff-body').classes()).toContain('is-wrap');
  });

  it('超大 diff 截断渲染并提示', () => {
    const lines = ['diff --git a/big.txt b/big.txt', '--- a/big.txt', '+++ b/big.txt', '@@ -1 +1 @@'];
    for (let index = 0; index < 4100; index += 1) lines.push(`+line ${index}`);
    const wrapper = mount(PfGitDiff, { props: { text: lines.join('\n') } });
    expect(wrapper.findAll('.pf-git-diff-line').length).toBeLessThanOrEqual(4000);
    expect(wrapper.find('.pf-git-diff-truncated').exists()).toBe(true);
    expect(wrapper.find('.pf-git-diff-truncated').text()).toContain('仅显示前');
  });
});
