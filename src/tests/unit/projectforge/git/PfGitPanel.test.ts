/**
 * `PfGitPanel.vue` 交互单测：工作区选择/加载态、分支行、变更分组、勾选 → 提交（二次确认）、
 * 推送、diff 子页、手动路径兜底。运行时依赖全部注入假 exec / confirm，不触网。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises } from '@vue/test-utils';
import PfGitPanel from '../../../../features/projectforge/components/PfGitPanel.vue';
import type {
  PfGitConfirmOptions,
  PfGitExec,
  PfGitRuntimeDeps,
} from '../../../../features/projectforge/useProjectForgeGit';
import { mountWithPinia } from '../../../utils/mount';
import { FULL_PORCELAIN, probeOutput, ptyOutput, SAMPLE_DIFF, snapshotOutput } from './fixtures';

interface FakeOptions {
  probe?: string;
  snapshot?: string;
  confirmResult?: boolean;
  listWorkspacesError?: Error;
  workspaces?: { id?: string; alias?: string; path: string }[];
}

interface RecordedCall {
  command: string;
}

function createRuntime(options: FakeOptions = {}) {
  const calls: RecordedCall[] = [];
  const exec: PfGitExec = async (command) => {
    calls.push({ command });
    if (command.includes('rev-parse --is-inside-work-tree')) {
      return { text: options.probe ?? probeOutput([0]) };
    }
    if (command.includes('git status --porcelain=v1 -z')) {
      return { text: options.snapshot ?? snapshotOutput({ status: FULL_PORCELAIN }) };
    }
    if (command.includes('git --no-pager diff')) {
      return { text: ptyOutput(SAMPLE_DIFF), exitCode: 0 };
    }
    if (command.includes('git add --')) return { text: ptyOutput('') };
    if (command.includes('git commit -m')) return { text: ptyOutput('[main abc1234] 面板提交') };
    if (command.includes('git push')) return { text: ptyOutput('To origin/main\n   abc..def  main -> main') };
    return { text: ptyOutput('') };
  };
  const confirm = vi.fn(async (_options: PfGitConfirmOptions) => options.confirmResult ?? true);
  const listWorkspaces = vi.fn(async () => {
    if (options.listWorkspacesError) throw options.listWorkspacesError;
    return options.workspaces ?? [{ id: 'w1', alias: 'demo', path: '/repos/demo' }];
  });
  const runtime: PfGitRuntimeDeps = { exec, listWorkspaces, confirm };
  const find = (pattern: RegExp): RecordedCall[] => calls.filter((call) => pattern.test(call.command));
  return { runtime, calls, confirm, listWorkspaces, find };
}

async function settle(rounds = 8): Promise<void> {
  for (let index = 0; index < rounds; index += 1) await flushPromises();
}

async function mountPanel(options: FakeOptions = {}) {
  const fake = createRuntime(options);
  const wrapper = mountWithPinia(PfGitPanel, { props: { runtime: fake.runtime } });
  await settle();
  return { wrapper, ...fake };
}

beforeEach(() => {
  window.localStorage.clear();
});

describe('PfGitPanel 工作区与状态展示', () => {
  it('加载后展示工作区下拉、分支/上游与全部变更分组', async () => {
    const { wrapper, listWorkspaces } = await mountPanel();

    expect(listWorkspaces).toHaveBeenCalledTimes(1);
    const select = wrapper.find('select.pf-git-select');
    expect(select.exists()).toBe(true);
    expect(wrapper.findAll('select.pf-git-select option')).toHaveLength(1);
    expect(select.text()).toContain('demo · /repos/demo');

    expect(wrapper.find('.pf-git-branch-name').text()).toBe('main');
    expect(wrapper.find('.pf-git-upstream').text()).toBe('origin/main');
    expect(wrapper.find('.pf-git-ahead').text()).toContain('2');
    expect(wrapper.find('.pf-git-behind').text()).toContain('1');

    const titles = wrapper.findAll('.pf-git-group-title').map((item) => item.text());
    expect(titles).toEqual(['合并冲突', '暂存的更改', '更改', '未跟踪文件']);
    expect(wrapper.findAll('.pf-git-row')).toHaveLength(8);
  });

  it('干净仓库显示空态，不展示分组', async () => {
    const { wrapper } = await mountPanel({ snapshot: snapshotOutput({ status: '' }) });
    expect(wrapper.find('.pf-git-empty').text()).toContain('干净的工作区');
    expect(wrapper.findAll('.pf-git-group')).toHaveLength(0);
  });

  it('非 Git 仓库显示错误提示', async () => {
    const { wrapper } = await mountPanel({
      snapshot: snapshotOutput({ topLevel: 'fatal: not a git repository (or any parent): .git' }),
    });
    expect(wrapper.text()).toContain('not a git repository');
  });

  it('工作区加载失败显示中文错误', async () => {
    const { wrapper } = await mountPanel({
      listWorkspacesError: new Error('桥接请求失败：HTTP 502'),
    });
    expect(wrapper.text()).toContain('加载工作区失败');
    expect(wrapper.text()).toContain('502');
  });

  it('手动输入路径：合法仓库加入下拉并切换', async () => {
    const { wrapper, find } = await mountPanel({
      workspaces: [{ id: 'w1', alias: 'demo', path: '/repos/demo' }],
    });

    await wrapper.find('button[aria-label="手动输入 Git 仓库路径"]').trigger('click');
    const input = wrapper.find('input.pf-git-manual-input');
    expect(input.exists()).toBe(true);
    await input.setValue('D:\\Workspace\\Project');
    await wrapper.find('form.pf-git-manual').trigger('submit');
    await settle();

    expect(wrapper.findAll('select.pf-git-select option')).toHaveLength(2);
    expect(wrapper.find('select.pf-git-select').text()).toContain('Workspace');
    const probe = find(/rev-parse --is-inside-work-tree/);
    expect(probe.some((call) => call.command.includes("'/d/Workspace/Project'"))).toBe(true);
    const select = wrapper.find('select.pf-git-select').element as HTMLSelectElement;
    expect(select.value).toBe('/d/Workspace/Project');
  });
});

describe('PfGitPanel 勾选与提交', () => {
  it('勾选文件并填写提交信息后提交按钮可用；无勾选/无信息时禁用', async () => {
    const { wrapper } = await mountPanel();
    const commitButton = wrapper.find('.pf-git-btn.primary');
    expect(commitButton.attributes('disabled')).toBeDefined();

    const firstRowCheckbox = wrapper.find('.pf-git-row input[type="checkbox"]');
    await firstRowCheckbox.setValue(true);
    await wrapper.find('textarea.pf-git-message').setValue('feat: 面板提交');
    expect(commitButton.attributes('disabled')).toBeUndefined();

    // 取消勾选 → 再次禁用
    await firstRowCheckbox.setValue(false);
    expect(wrapper.find('.pf-git-btn.primary').attributes('disabled')).toBeDefined();
  });

  it('点击提交先二次确认，确认后执行 git add + git commit 并显示输出面板', async () => {
    const { wrapper, confirm, find } = await mountPanel();

    await wrapper.findAll('.pf-git-row input[type="checkbox"]')[0].setValue(true);
    await wrapper.find('textarea.pf-git-message').setValue('feat: 面板提交');
    await wrapper.find('.pf-git-btn.primary').trigger('click');
    await settle();

    expect(confirm).toHaveBeenCalledTimes(1);
    const payload = confirm.mock.calls[0][0];
    expect(payload.title).toBe('确认提交');
    expect(payload.message).toContain('feat: 面板提交');

    const adds = find(/git add --/);
    const commits = find(/git commit -m/);
    expect(adds).toHaveLength(1);
    expect(commits).toHaveLength(1);
    expect(commits[0].command).toContain("git commit -m 'feat: 面板提交'");
    expect(wrapper.find('.pf-git-op').exists()).toBe(true);
    expect(wrapper.find('.pf-git-op-title').text()).toContain('提交完成');
    expect(wrapper.find('.pf-git-op-text').text()).toContain('面板提交');
    expect((wrapper.find('textarea.pf-git-message').element as HTMLTextAreaElement).value).toBe('');
  });

  it('二次确认取消时不会执行任何写命令', async () => {
    const { wrapper, confirm, find } = await mountPanel({ confirmResult: false });
    await wrapper.findAll('.pf-git-row input[type="checkbox"]')[0].setValue(true);
    await wrapper.find('textarea.pf-git-message').setValue('feat: 取消提交');
    await wrapper.find('.pf-git-btn.primary').trigger('click');
    await settle();

    expect(confirm).toHaveBeenCalledTimes(1);
    expect(find(/git add --/)).toHaveLength(0);
    expect(find(/git commit -m/)).toHaveLength(0);
  });

  it('点击推送先二次确认，确认后执行 git push 并展示输出', async () => {
    const { wrapper, confirm, find } = await mountPanel();
    const pushButton = wrapper.findAll('.pf-git-btn')[1];
    expect(pushButton.text()).toContain('推送');
    await pushButton.trigger('click');
    await settle();

    expect(confirm).toHaveBeenCalledTimes(1);
    const payload = confirm.mock.calls[0][0];
    expect(payload.title).toBe('确认推送');
    expect(find(/git push/)).toHaveLength(1);
    expect(wrapper.find('.pf-git-op-title').text()).toContain('推送完成');
  });
});

describe('PfGitPanel diff 子页', () => {
  it('点击未跟踪目录时展示目录提示而非执行 diff 命令', async () => {
    const { wrapper, find } = await mountPanel({
      snapshot: snapshotOutput({ status: '?? untracked-dir/\u0000' }),
    });
    const target = wrapper
      .findAll('.pf-git-open')
      .find((item) => item.text().includes('untracked-dir'));
    expect(target).toBeTruthy();
    await target!.trigger('click');
    await settle();

    expect(wrapper.find('.pf-git-diff-state').text()).toContain('未跟踪的目录');
    expect(find(/git --no-pager diff/)).toHaveLength(0);
  });

  it('点文件打开 diff 子页，返回后回到变更列表', async () => {
    const { wrapper, find } = await mountPanel();

    const target = wrapper
      .findAll('.pf-git-open')
      .find((item) => item.text().includes('changed.txt'));
    expect(target).toBeTruthy();
    await target!.trigger('click');
    await settle();

    expect(wrapper.find('.pf-git-diff').exists()).toBe(true);
    expect(wrapper.find('.pf-git-diff-title').text()).toBe('changed.txt');
    expect(wrapper.find('.pf-git-diff-body').text()).toContain('const b = 3;');
    const diffCalls = find(/git --no-pager diff --no-color --/);
    expect(diffCalls.length).toBeGreaterThanOrEqual(1);

    await wrapper.find('.pf-git-back').trigger('click');
    await settle();
    expect(wrapper.find('.pf-git-diff').exists()).toBe(false);
    expect(wrapper.findAll('.pf-git-group').length).toBeGreaterThan(0);
  });
});
