/**
 * `useProjectForgeGit` 运行时单测（注入假 exec，不触网）。
 *
 * 覆盖：工作区 ListWorkspaces → `git -C ... rev-parse --is-inside-work-tree` 过滤、手动路径兜底、
 * status/分支/ahead-behind/无上游容错、diff 命令参数、勾选与全选、提交/推送的二次确认与命令顺序、
 * 失败输出展示、桥接异常与路径不可访问。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises } from '@vue/test-utils';
import {
  createProjectForgeGit,
  type PfGitConfirmOptions,
  type PfGitExec,
  type PfGitRuntimeDeps,
} from '../../../../features/projectforge/useProjectForgeGit';
import { FULL_PORCELAIN, probeOutput, ptyOutput, SAMPLE_DIFF, snapshotOutput } from './fixtures';

interface RecordedCall {
  command: string;
  timeoutMs?: number;
}

interface FakeResponse {
  text?: string;
  ok?: boolean;
  exitCode?: number | null;
  throws?: Error;
}

function createFakeExec(handler: (command: string, call: RecordedCall) => FakeResponse) {
  const calls: RecordedCall[] = [];
  const exec: PfGitExec = async (command, options) => {
    const call: RecordedCall = { command, timeoutMs: options?.timeoutMs };
    calls.push(call);
    const response = handler(command, call);
    if (response.throws) throw response.throws;
    return { text: response.text ?? '', ok: response.ok ?? true, exitCode: response.exitCode ?? null };
  };
  const find = (pattern: RegExp): RecordedCall[] => calls.filter((call) => pattern.test(call.command));
  const writeCalls = (): RecordedCall[] => find(/git (add|commit|push) /);
  return { exec, calls, find, writeCalls };
}

interface RouterOptions {
  probe?: string;
  snapshot?: string;
  diff?: string;
  add?: FakeResponse;
  commit?: FakeResponse;
  push?: FakeResponse;
}

/** 默认命令路由：探测 → 仓库；快照 → 全分组；diff/写命令返回成功输出。 */
function defaultRouter(options: RouterOptions = {}) {
  return (command: string): FakeResponse => {
    if (command.includes('rev-parse --is-inside-work-tree')) {
      return { text: options.probe ?? probeOutput([0]) };
    }
    if (command.includes('git status --porcelain=v1 -z')) {
      return { text: options.snapshot ?? snapshotOutput({ status: FULL_PORCELAIN }) };
    }
    if (command.includes('git --no-pager diff')) {
      return {
        text: options.diff ?? ptyOutput(SAMPLE_DIFF, command.includes('--no-index') ? 1 : 0),
        exitCode: command.includes('--no-index') ? 1 : 0,
      };
    }
    if (command.includes('git add --')) {
      return options.add ?? { text: ptyOutput(''), exitCode: 0 };
    }
    if (command.includes('git commit -m')) {
      return options.commit ?? { text: ptyOutput('[main abc1234] test: 提交'), exitCode: 0 };
    }
    if (command.includes('git push')) {
      return options.push ?? { text: ptyOutput('Everything up-to-date'), exitCode: 0 };
    }
    return { text: ptyOutput(''), exitCode: 0 };
  };
}

async function settle(rounds = 8): Promise<void> {
  for (let index = 0; index < rounds; index += 1) await flushPromises();
}

interface SeedOptions extends RouterOptions {
  workspacePath?: string;
  alias?: string;
  confirmResult?: boolean;
}

/** 建一个已加载完成（工作区选中 + 首次状态刷新）的运行时。 */
async function seedRuntime(options: SeedOptions = {}) {
  const fake = createFakeExec(defaultRouter(options));
  const confirm = vi.fn(async (_options: PfGitConfirmOptions) => options.confirmResult ?? true);
  const listWorkspaces = vi.fn(async () => [
    { id: 'w1', alias: options.alias ?? 'demo', path: options.workspacePath ?? '/repos/demo' },
  ]);
  const runtime: PfGitRuntimeDeps = { exec: fake.exec, listWorkspaces, confirm };
  const git = createProjectForgeGit(runtime);
  await git.loadWorkspaces();
  await settle();
  return { git, ...fake, confirm, listWorkspaces };
}

beforeEach(() => {
  window.localStorage.clear();
});

describe('工作区加载（ListWorkspaces → git 仓库过滤）', () => {
  it('只保留 rev-parse 判定为 Git 仓库的工作区，并自动选中第一个并读取状态', async () => {
    const fake = createFakeExec((command) => {
      if (command.includes('rev-parse --is-inside-work-tree')) return { text: probeOutput([0, 2]) };
      if (command.includes('git status --porcelain=v1 -z')) {
        return { text: snapshotOutput({ status: ' M changed.txt\u0000', branch: 'dev' }) };
      }
      return { text: ptyOutput('') };
    });
    const listWorkspaces = vi.fn(async () => [
      { id: 'w1', alias: '仓库A', path: 'D:\\repo-a' },
      { id: 'w2', alias: '普通目录', path: 'D:\\plain' },
      { id: 'w3', alias: '仓库B', path: '/home/b/repo-b' },
    ]);
    const git = createProjectForgeGit({ exec: fake.exec, listWorkspaces });

    await git.loadWorkspaces();
    await settle();

    expect(listWorkspaces).toHaveBeenCalledTimes(1);
    expect(git.workspaces.value.map((workspace) => workspace.alias)).toEqual(['仓库A', '仓库B']);
    expect(git.workspaces.value.map((workspace) => workspace.shellPath)).toEqual([
      '/d/repo-a',
      '/home/b/repo-b',
    ]);
    expect(git.currentWorkspace.value?.alias).toBe('仓库A');
    expect(git.isRepo.value).toBe(true);
    expect(git.status.value?.branch.head).toBe('dev');
    expect(git.status.value?.groups.changes.map((entry) => entry.path)).toEqual(['changed.txt']);
    // 探测命令使用 POSIX 路径 + git -C + rev-parse --is-inside-work-tree
    const probe = fake.find(/rev-parse --is-inside-work-tree/)[0];
    expect(probe.command).toContain("'/d/repo-a'");
    expect(probe.command).toContain("'/home/b/repo-b'");
    // 快照命令符合防挂起要求
    const snapshot = fake.find(/git status --porcelain=v1 -z/)[0];
    expect(snapshot.command).toContain("cd '/d/repo-a'");
    expect(snapshot.command).toContain('GIT_TERMINAL_PROMPT=0');
    expect(snapshot.command).toContain('LC_ALL=C');
    expect(snapshot.command).toContain('log --oneline -20');
  });

  it('探测命令无输出（桥接离线）时给出中文错误，不保留未验证工作区', async () => {
    const fake = createFakeExec((command) => {
      if (command.includes('rev-parse --is-inside-work-tree')) {
        return { text: 'HTTP 502 Bad Gateway', ok: false, exitCode: null };
      }
      return { text: ptyOutput('') };
    });
    const git = createProjectForgeGit({
      exec: fake.exec,
      listWorkspaces: async () => [{ id: 'w1', alias: '远端仓库', path: 'D:\\repo-a' }],
    });

    await git.loadWorkspaces();
    await settle();

    expect(git.workspaces.value).toEqual([]);
    expect(git.workspacesError.value).toContain('检测 Git 工作区失败');
    expect(git.workspacesError.value).toContain('502');
  });

  it('ListWorkspaces 抛错时展示加载失败错误', async () => {
    const fake = createFakeExec(defaultRouter());
    const git = createProjectForgeGit({
      exec: fake.exec,
      listWorkspaces: async () => {
        throw new Error('桥接请求失败：连接超时');
      },
    });
    await git.loadWorkspaces();
    expect(git.workspacesError.value).toContain('加载工作区失败');
    expect(git.workspacesError.value).toContain('连接超时');
  });
});

describe('手动输入路径兜底', () => {
  it('合法 Git 仓库路径会加入列表并选中，同时转换 Windows 路径', async () => {
    const fake = createFakeExec(defaultRouter());
    const git = createProjectForgeGit({ exec: fake.exec, listWorkspaces: async () => [] });
    await git.loadWorkspaces();

    const result = await git.addWorkspaceByPath('D:\\Workspace\\Project');
    await settle();

    expect(result.ok).toBe(true);
    expect(result.message).toContain('已添加');
    expect(git.workspaces.value).toHaveLength(1);
    expect(git.workspaces.value[0].shellPath).toBe('/d/Workspace/Project');
    expect(git.workspaces.value[0].manual).toBe(true);
    expect(git.currentShellPath.value).toBe('/d/Workspace/Project');
    const probe = fake.find(/rev-parse --is-inside-work-tree/)[0];
    expect(probe.command).toContain("'/d/Workspace/Project'");
  });

  it('非 Git 仓库路径返回中文失败原因', async () => {
    const fake = createFakeExec((command) => {
      if (command.includes('rev-parse --is-inside-work-tree')) return { text: probeOutput([]) };
      return { text: ptyOutput('') };
    });
    const git = createProjectForgeGit({ exec: fake.exec, listWorkspaces: async () => [] });
    await git.loadWorkspaces();

    const result = await git.addWorkspaceByPath('/home/user/plain-dir');

    expect(result.ok).toBe(false);
    expect(result.message).toContain('不是 Git 仓库');
    expect(git.workspaces.value).toHaveLength(0);
  });
});

describe('状态刷新（分支 / ahead-behind / diff）', () => {
  it('解析分支、上游与领先/落后数量', async () => {
    const { git, find } = await seedRuntime({
      snapshot: snapshotOutput({
        branch: 'feature/pf',
        upstream: 'origin/feature/pf',
        ab: '2\t4',
        status: ' M a.txt\u0000',
      }),
    });
    expect(git.isRepo.value).toBe(true);
    expect(git.status.value?.branch.head).toBe('feature/pf');
    expect(git.status.value?.branch.upstream).toBe('origin/feature/pf');
    expect(git.status.value?.branch.ahead).toBe(4);
    expect(git.status.value?.branch.behind).toBe(2);
    expect(find(/git status --porcelain=v1 -z/).length).toBeGreaterThanOrEqual(1);
  });

  it('无上游时容错（upstream=null、ahead/behind=0），仍可推送', async () => {
    const { git } = await seedRuntime({ snapshot: snapshotOutput({ upstream: null, status: '' }) });
    expect(git.status.value?.branch.upstream).toBeNull();
    expect(git.status.value?.branch.ahead).toBe(0);
    expect(git.status.value?.branch.behind).toBe(0);
    expect(git.canPush.value).toBe(true);
    expect(git.status.value?.total).toBe(0);
  });

  it('分离 HEAD 时禁止推送', async () => {
    const { git } = await seedRuntime({
      snapshot: snapshotOutput({ branchRaw: 'HEAD', branch: '' }),
    });
    expect(git.status.value?.branch.detached).toBe(true);
    expect(git.canPush.value).toBe(false);
  });

  it('空仓库（无提交）标记 initial 且禁止推送', async () => {
    const { git } = await seedRuntime({ snapshot: snapshotOutput({ log: false, status: '' }) });
    expect(git.status.value?.branch.initial).toBe(true);
    expect(git.canPush.value).toBe(false);
    expect(git.totalChanges.value).toBe(0);
  });

  it('staged 文件 diff 使用 --cached，未跟踪文件使用 --no-index 且退出码 1 视为成功', async () => {
    const { git, find } = await seedRuntime();
    await git.openDiff('staged', 'added.txt');
    expect(git.diffError.value).toBe('');
    expect(git.diffText.value).toContain('+const b = 3;');
    expect(find(/git --no-pager diff --no-color --cached -- 'added.txt'/)).toHaveLength(1);

    await git.openDiff('untracked', 'new file.txt');
    expect(git.diffError.value).toBe('');
    expect(git.diffText.value).toContain('+const c = 4;');
    expect(find(/git --no-pager diff --no-color --no-index -- \/dev\/null 'new file\.txt'/)).toHaveLength(1);
  });

  it('未跟踪目录（porcelain 聚合为 dir/）打开 diff 不发命令，交给空态提示', async () => {
    const { git, find } = await seedRuntime({
      snapshot: snapshotOutput({ status: '?? untracked-dir/\u0000' }),
    });
    await git.openDiff('untracked', 'untracked-dir/');
    expect(git.activeDiff.value?.path).toBe('untracked-dir/');
    expect(git.diffLoading.value).toBe(false);
    expect(git.diffError.value).toBe('');
    expect(git.diffText.value).toBe('');
    expect(find(/git --no-pager diff/)).toHaveLength(0);
  });

  it('exec 抛异常时显示中文错误（不抛出）', async () => {
    const fake = createFakeExec((command) => {
      if (command.includes('rev-parse --is-inside-work-tree')) return { text: probeOutput([0]) };
      if (command.includes('git status --porcelain=v1 -z')) return { throws: new Error('桥接不可达：HTTP 502') };
      return { text: ptyOutput('') };
    });
    const git = createProjectForgeGit({
      exec: fake.exec,
      listWorkspaces: async () => [{ id: 'w1', alias: 'demo', path: '/repos/demo' }],
    });
    await git.loadWorkspaces();
    await settle();

    expect(git.statusError.value).toContain('读取 Git 状态失败');
    expect(git.statusError.value).toContain('桥接不可达');
    expect(git.status.value).toBeNull();
  });

  it('仓库路径不可访问（退出码 97）时给出明确提示', async () => {
    const fake = createFakeExec((command) => {
      if (command.includes('rev-parse --is-inside-work-tree')) return { text: probeOutput([0]) };
      return { text: ptyOutput('', 97), exitCode: 97 };
    });
    const git = createProjectForgeGit({
      exec: fake.exec,
      listWorkspaces: async () => [{ id: 'w1', alias: 'demo', path: '/repos/demo' }],
    });
    await git.loadWorkspaces();
    await settle();

    expect(git.statusError.value).toContain('仓库路径不可访问');
    expect(git.status.value).toBeNull();
  });
});

describe('勾选与全选', () => {
  it('逐文件勾选、分组全选、MM 文件去重后只提交一次', async () => {
    const { git } = await seedRuntime();
    expect(git.selectedCount.value).toBe(0);
    expect(git.canCommit.value).toBe(false);

    git.toggleFile('changes', 'both.txt', true);
    git.toggleFile('staged', 'both.txt', true);
    expect(git.selectedCount.value).toBe(1);
    expect(git.selectedPaths.value).toEqual(['both.txt']);

    git.setGroupSelected('changes', true);
    expect(git.groupSelection('changes').all).toBe(true);
    expect(git.selectedPaths.value).toEqual(expect.arrayContaining(['changed.txt', 'both.txt']));

    git.setGroupSelected('changes', false);
    expect(git.groupSelection('changes').count).toBe(0);
    expect(git.selectedPaths.value).toEqual(['both.txt']);

    git.clearSelection();
    expect(git.selectedCount.value).toBe(0);
  });

  it('刷新后自动清理已不存在的勾选', async () => {
    let snapshotRound = 0;
    const fake = createFakeExec((command) => {
      if (command.includes('rev-parse --is-inside-work-tree')) return { text: probeOutput([0]) };
      if (command.includes('git status --porcelain=v1 -z')) {
        snapshotRound += 1;
        return { text: snapshotOutput({ status: snapshotRound === 1 ? ' M a.txt\u0000' : '' }) };
      }
      return { text: ptyOutput('') };
    });
    const git = createProjectForgeGit({
      exec: fake.exec,
      listWorkspaces: async () => [{ id: 'w1', alias: 'demo', path: '/repos/demo' }],
    });
    await git.loadWorkspaces();
    await settle();
    git.toggleFile('changes', 'a.txt', true);
    expect(git.selectedPaths.value).toEqual(['a.txt']);

    // 下一次刷新 a.txt 已被提交（不在 status 中）→ 自动清理勾选
    await git.refreshStatus();
    await settle();
    expect(git.selectedCount.value).toBe(0);
  });
});

describe('提交（二次确认 + git add + git commit）', () => {
  it('未勾选或未填提交信息时不弹确认、不执行命令', async () => {
    const { git, confirm, writeCalls } = await seedRuntime();
    expect(await git.confirmAndCommit()).toBe(false);
    expect(confirm).not.toHaveBeenCalled();
    expect(writeCalls()).toHaveLength(0);
    expect(git.notice.value?.text).toContain('请先勾选');

    git.toggleFile('changes', 'changed.txt', true);
    expect(await git.confirmAndCommit()).toBe(false);
    expect(confirm).not.toHaveBeenCalled();
    expect(git.notice.value?.text).toContain('请填写提交信息');
    expect(writeCalls()).toHaveLength(0);
  });

  it('确认框展示勾选文件与提交信息；取消后不执行任何写命令', async () => {
    const { git, confirm, writeCalls } = await seedRuntime({ confirmResult: false });
    git.toggleFile('changes', 'changed.txt', true);
    git.toggleFile('staged', 'added.txt', true);
    git.commitMessage.value = 'feat: 提交两个文件';

    const result = await git.confirmAndCommit();

    expect(result).toBe(false);
    expect(confirm).toHaveBeenCalledTimes(1);
    const confirmPayload = confirm.mock.calls[0][0];
    expect(confirmPayload.title).toBe('确认提交');
    expect(confirmPayload.message).toContain('changed.txt');
    expect(confirmPayload.message).toContain('added.txt');
    expect(confirmPayload.message).toContain('feat: 提交两个文件');
    expect(writeCalls()).toHaveLength(0);
    expect(git.notice.value?.text).toContain('已取消提交');
  });

  it('确认后依次执行 git add -- <paths> 与 git commit -m <msg>，成功后清空输入并刷新', async () => {
    const { git, confirm, find, calls } = await seedRuntime();
    git.toggleFile('changes', 'changed.txt', true);
    git.toggleFile('staged', 'added.txt', true);
    git.commitMessage.value = 'fix: 修复 a b';

    const result = await git.confirmAndCommit();
    await settle();

    expect(result).toBe(true);
    expect(confirm).toHaveBeenCalledTimes(1);
    const commits = find(/git commit -m/);
    expect(commits).toHaveLength(1);
    expect(commits[0].command).toContain("git commit -m 'fix: 修复 a b'");
    const adds = find(/git add --/);
    expect(adds).toHaveLength(1);
    expect(adds[0].command).toContain("'changed.txt'");
    expect(adds[0].command).toContain("'added.txt'");
    // add 必须早于 commit
    expect(calls.indexOf(adds[0])).toBeLessThan(calls.indexOf(commits[0]));
    expect(git.commitMessage.value).toBe('');
    expect(git.selectedCount.value).toBe(0);
    expect(git.opOutput.value?.title).toBe('提交完成');
    expect(git.opOutput.value?.ok).toBe(true);
    expect(git.opOutput.value?.text).toContain('test: 提交');
    expect(git.notice.value?.text).toContain('提交成功');
  });

  it('git add 失败时不再 commit，输出面板展示原始 stderr 且保留提交信息', async () => {
    const { git, find } = await seedRuntime({
      add: { text: ptyOutput("fatal: pathspec 'missing.txt' did not match any files", 128), exitCode: 128 },
    });
    git.toggleFile('changes', 'changed.txt', true);
    git.commitMessage.value = 'fix: 失败用例';

    const result = await git.confirmAndCommit();

    expect(result).toBe(false);
    expect(find(/git commit -m/)).toHaveLength(0);
    expect(git.opOutput.value?.title).toBe('暂存失败');
    expect(git.opOutput.value?.ok).toBe(false);
    expect(git.opOutput.value?.text).toContain('did not match any files');
    expect(git.notice.value?.text).toContain('暂存失败');
    expect(git.commitMessage.value).toBe('fix: 失败用例');
  });

  it('git commit 失败时输出面板展示 stderr 并保留勾选', async () => {
    const { git } = await seedRuntime({
      commit: { text: ptyOutput('fatal: 空提交信息', 1), exitCode: 1 },
    });
    git.toggleFile('changes', 'changed.txt', true);
    git.commitMessage.value = 'fix: 失败用例';

    const result = await git.confirmAndCommit();

    expect(result).toBe(false);
    expect(git.opOutput.value?.title).toBe('提交失败');
    expect(git.opOutput.value?.text).toContain('fatal: 空提交信息');
    expect(git.notice.value?.text).toContain('提交失败');
    expect(git.selectedCount.value).toBe(1);
    expect(git.commitMessage.value).toBe('fix: 失败用例');
  });
});

describe('推送（二次确认 + git push）', () => {
  it('有上游：确认后执行 git push，成功后输出面板展示远端信息', async () => {
    const { git, confirm, find } = await seedRuntime({
      snapshot: snapshotOutput({ upstream: 'origin/main', ab: '0\t2', status: '' }),
      push: { text: ptyOutput('To origin/main\n   1234567..abcdef0  main -> main'), exitCode: 0 },
    });
    const result = await git.confirmAndPush();
    await settle();

    expect(result).toBe(true);
    expect(confirm).toHaveBeenCalledTimes(1);
    const payload = confirm.mock.calls[0][0];
    expect(payload.title).toBe('确认推送');
    expect(payload.message).toContain('origin/main');
    const pushes = find(/git push/);
    expect(pushes).toHaveLength(1);
    expect(pushes[0].command).toContain('git push');
    expect(pushes[0].command).not.toContain('--set-upstream');
    expect(git.opOutput.value?.title).toBe('推送完成');
    expect(git.opOutput.value?.text).toContain('main -> main');
    expect(git.notice.value?.text).toContain('推送成功');
  });

  it('无上游：确认后执行 git push --set-upstream origin <branch>', async () => {
    const { git, confirm, find } = await seedRuntime({
      snapshot: snapshotOutput({ upstream: null, status: '' }),
    });
    const result = await git.confirmAndPush();
    await settle();

    expect(result).toBe(true);
    const payload = confirm.mock.calls[0][0];
    expect(payload.title).toBe('推送到新上游');
    expect(payload.message).toContain('--set-upstream origin main');
    const pushes = find(/git push/);
    expect(pushes).toHaveLength(1);
    expect(pushes[0].command).toContain("git push --set-upstream origin 'main'");
  });

  it('取消确认时不执行推送命令', async () => {
    const { git, writeCalls } = await seedRuntime({ confirmResult: false });
    const result = await git.confirmAndPush();
    expect(result).toBe(false);
    expect(writeCalls().filter((call) => /git push/.test(call.command))).toHaveLength(0);
    expect(git.notice.value?.text).toContain('已取消推送');
  });

  it('推送失败时输出面板展示原始 stderr', async () => {
    const { git } = await seedRuntime({
      snapshot: snapshotOutput({ upstream: 'origin/main', status: '' }),
      push: { text: ptyOutput('fatal: unable to access remote: 443', 128), exitCode: 128 },
    });
    const result = await git.confirmAndPush();
    expect(result).toBe(false);
    expect(git.opOutput.value?.title).toBe('推送失败');
    expect(git.opOutput.value?.text).toContain('unable to access remote');
    expect(git.notice.value?.text).toContain('推送失败');
  });
});

describe('分支 + PR（2026-11-07）', () => {
  it('勾选「新建修改分支」后提交：先 git switch -c，再 add/commit，并提示新分支名', async () => {
    let branch = 'main';
    const fake = createFakeExec((command) => {
      if (command.includes('rev-parse --is-inside-work-tree')) return { text: probeOutput([0]) };
      if (command.includes('rev-parse --verify --quiet refs/heads/')) return { text: ptyOutput('', 1), exitCode: 1 };
      if (command.includes('git switch -c')) {
        branch = 'vcp/20261107-183005';
        return { text: ptyOutput(''), exitCode: 0 };
      }
      if (command.includes('git add --')) return { text: ptyOutput(''), exitCode: 0 };
      if (command.includes('git commit -m')) {
        return { text: ptyOutput('[vcp/20261107-183005 abc1234] test: 新分支提交'), exitCode: 0 };
      }
      if (command.includes('git status --porcelain=v1 -z')) {
        return { text: snapshotOutput({ status: ' M changed.txt\u0000', branch, upstream: null }) };
      }
      return { text: ptyOutput(''), exitCode: 0 };
    });
    const git = createProjectForgeGit({
      exec: fake.exec,
      listWorkspaces: async () => [{ id: 'w1', alias: 'demo', path: '/repos/demo' }],
      confirm: async () => true,
    });
    await git.loadWorkspaces();
    await settle();

    git.toggleFile('changes', 'changed.txt', true);
    git.enableNewBranchMode(true);
    git.setNewBranchName('vcp/20261107-183005');
    const ok = await git.stageAndCommit(['changed.txt'], 'test: 新分支提交');
    await settle();

    expect(ok).toBe(true);
    const switchCall = fake.calls.findIndex((call) => call.command.includes('git switch -c'));
    const addCall = fake.calls.findIndex((call) => call.command.includes('git add --'));
    const commitCall = fake.calls.findIndex((call) => call.command.includes('git commit -m'));
    expect(switchCall).toBeGreaterThanOrEqual(0);
    expect(fake.calls[switchCall].command).toContain("git switch -c 'vcp/20261107-183005'");
    expect(addCall).toBeGreaterThan(switchCall);
    expect(commitCall).toBeGreaterThan(addCall);
    expect(git.notice.value?.text).toContain('vcp/20261107-183005');
  });

  it('推送新分支后自动创建 PR（fork → upstream，gh 可用）', async () => {
    const remotes = [
      'origin\thttps://github.com/octocat/demo-repo.git (fetch)',
      'upstream\thttps://github.com/up/demo-repo.git (fetch)',
    ].join('\n');
    let upstream: string | null = null;
    const fake = createFakeExec((command) => {
      if (command.includes('gh --version')) return { text: ptyOutput('gh version 2.103.3') };
      if (command.includes('gh auth status')) {
        return { text: ptyOutput('github.com\n  ✓ Logged in to github.com account octocat (keyring)') };
      }
      if (command.includes('gh repo view')) return { text: ptyOutput('{"defaultBranchRef":{"name":"main"}}') };
      if (command.includes('gh pr create')) return { text: ptyOutput('https://github.com/up/demo-repo/pull/42') };
      if (command.includes('rev-parse --is-inside-work-tree')) return { text: probeOutput([0]) };
      if (command.includes('git push')) {
        upstream = 'origin/vcp/pr-test';
        return { text: ptyOutput('branch pushed') };
      }
      if (command.includes('git status --porcelain=v1 -z')) {
        return { text: snapshotOutput({ status: '', branch: 'vcp/pr-test', upstream, remotes }) };
      }
      return { text: ptyOutput(''), exitCode: 0 };
    });
    const git = createProjectForgeGit({
      exec: fake.exec,
      listWorkspaces: async () => [{ id: 'w1', alias: 'demo', path: '/repos/demo' }],
      confirm: async () => true,
    });
    await git.loadWorkspaces();
    await settle();

    git.enableNewBranchMode(true);
    git.setAutoCreatePr(true);
    const ok = await git.push();
    await settle();

    expect(ok).toBe(true);
    const pushCall = fake.calls.find((call) => call.command.includes('git push'));
    expect(pushCall?.command).toContain("push --set-upstream origin 'vcp/pr-test'");
    const prCall = fake.calls.find((call) => call.command.includes('gh pr create'));
    expect(prCall?.command).toContain('--repo up/demo-repo');
    expect(prCall?.command).toContain('--head octocat:vcp/pr-test');
    expect(prCall?.command).toContain('--base main');
    expect(prCall?.command).toContain('--fill');
    expect(git.prResult.value?.ok).toBe(true);
    expect(git.prResult.value?.url).toBe('https://github.com/up/demo-repo/pull/42');
    expect(git.notice.value?.text).toContain('PR 已创建');
  });

  it('gh 不可用时推送仍成功，并降级给出 GitHub compare 链接', async () => {
    const remotes = 'origin\thttps://github.com/octocat/demo-repo.git (fetch)';
    let upstream: string | null = null;
    const fake = createFakeExec((command) => {
      if (command.includes('gh --version')) return { text: ptyOutput('gh: not found', 127), exitCode: 127 };
      if (command.includes('rev-parse --is-inside-work-tree')) return { text: probeOutput([0]) };
      if (command.includes('git push')) {
        upstream = 'origin/vcp/pr-test';
        return { text: ptyOutput('branch pushed') };
      }
      if (command.includes('git status --porcelain=v1 -z')) {
        return { text: snapshotOutput({ status: '', branch: 'vcp/pr-test', upstream, remotes }) };
      }
      return { text: ptyOutput(''), exitCode: 0 };
    });
    const git = createProjectForgeGit({
      exec: fake.exec,
      listWorkspaces: async () => [{ id: 'w1', alias: 'demo', path: '/repos/demo' }],
      confirm: async () => true,
    });
    await git.loadWorkspaces();
    await settle();

    git.enableNewBranchMode(true);
    git.setAutoCreatePr(true);
    await git.push();
    await settle();

    expect(fake.calls.some((call) => call.command.includes('gh pr create'))).toBe(false);
    expect(git.prResult.value?.ok).toBe(false);
    expect(git.prResult.value?.url).toContain('github.com/octocat/demo-repo/compare');
    expect(git.prResult.value?.message).toContain('compare');
  });

  it('非 GitHub 远端时推送不触发 PR（无 gh 命令）', async () => {
    let upstream: string | null = null;
    const fake = createFakeExec((command) => {
      if (command.includes('rev-parse --is-inside-work-tree')) return { text: probeOutput([0]) };
      if (command.includes('git push')) {
        upstream = 'origin/vcp/pr-test';
        return { text: ptyOutput('branch pushed') };
      }
      if (command.includes('git status --porcelain=v1 -z')) {
        return {
          text: snapshotOutput({
            status: '',
            branch: 'vcp/pr-test',
            upstream,
            remotes: 'origin\thttps://gitlab.com/me/repo.git (fetch)',
          }),
        };
      }
      return { text: ptyOutput(''), exitCode: 0 };
    });
    const git = createProjectForgeGit({
      exec: fake.exec,
      listWorkspaces: async () => [{ id: 'w1', alias: 'demo', path: '/repos/demo' }],
      confirm: async () => true,
    });
    await git.loadWorkspaces();
    await settle();

    git.setAutoCreatePr(true);
    await git.push();
    await settle();

    expect(fake.calls.some((call) => call.command.includes('gh '))).toBe(false);
    expect(git.prResult.value).toBeNull();
    expect(git.githubContext.value).toBeNull();
  });
});
