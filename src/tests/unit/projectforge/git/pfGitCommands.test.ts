/**
 * Git 模式命令构造 / 输出解析 / 安全白名单的纯函数单测。
 *
 * 覆盖：Windows→POSIX 路径转换、shell 转义、porcelain -z 分组解析（含重命名/冲突/MM/未跟踪）、
 * 快照解析（分支/上游/ahead-behind/空仓库/分离 HEAD/非仓库/子目录前缀）、命令构造内容、
 * 破坏性命令拦截；另有 `PF_GIT_REAL_SHELL=1` 时启用的真实 bash 集成用例（默认跳过）。
 */
// Vitest 运行在 Node 环境；应用 tsconfig 有意不引入 Node 类型（同 MobileUiCompatibility 测试约定）。
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';
import { describe, expect, it } from 'vitest';
import {
  assertSafeCommand,
  assertSafeGhArgs,
  assertSafeGitArgs,
  buildGhCommand,
  applyDisplayPrefix,
  buildDiffCommand,
  buildGitCommand,
  buildSnapshotCommand,
  buildWorkspaceProbeCommand,
  defaultNewBranchName,
  detectGithubContext,
  extractPullRequestUrl,
  formatGithubCompareUrl,
  isValidGitBranchName,
  parseGhJsonOutput,
  parseGithubRemoteUrl,
  parseRemoteUrls,
  parseGitCommandOutput,
  parsePorcelainV1Z,
  parseRepoSnapshot,
  parseWorkspaceProbe,
  relativePrefix,
  shellQuote,
  splitSnapshotSections,
  toShellPath,
} from '../../../../features/projectforge/useProjectForgeGit';
import { FULL_PORCELAIN, probeOutput, snapshotOutput } from './fixtures';

describe('toShellPath（Windows → Git Bash POSIX 路径）', () => {
  it('转换盘符路径并统一正斜杠、盘符小写', () => {
    expect(toShellPath('D:\\Foo\\Bar')).toBe('/d/Foo/Bar');
    expect(toShellPath('C:/Users/admin')).toBe('/c/Users/admin');
    expect(toShellPath('E:\\')).toBe('/e');
    expect(toShellPath('  "D:\\Workspace\\Project"  ')).toBe('/d/Workspace/Project');
  });

  it('POSIX 路径原样保留，UNC 转 //server/share，相对路径统一斜杠', () => {
    expect(toShellPath('/home/user/示例工程')).toBe('/home/user/示例工程');
    expect(toShellPath('/d/Workspace/')).toBe('/d/Workspace');
    expect(toShellPath('\\\\server\\share\\x')).toBe('//server/share/x');
    expect(toShellPath('relative\\dir')).toBe('relative/dir');
    expect(toShellPath('')).toBe('');
    expect(toShellPath(undefined)).toBe('');
  });
});

describe('shellQuote / relativePrefix', () => {
  it('单引号安全转义包含空格与单引号的参数', () => {
    expect(shellQuote('a b')).toBe("'a b'");
    expect(shellQuote("it's")).toBe("'it'\\''s'");
  });

  it('计算子目录工作区相对仓库根的展示前缀', () => {
    expect(relativePrefix('/d/Foo', '/d/Foo')).toBe('');
    expect(relativePrefix('/d/Foo', '/d/Foo/sub')).toBe('sub');
    expect(relativePrefix('/d/Foo', '/d/FOO/sub')).toBe('sub');
    expect(relativePrefix('/d/Foo', '/d/FooBar')).toBe('');
    expect(relativePrefix('/d/FooBar', '/d/Foo')).toBe('');
    expect(relativePrefix('', '/d/Foo')).toBe('');
  });
});

describe('parsePorcelainV1Z（status 分组解析）', () => {
  it('解析 staged/changes/untracked/conflicts 并保留重命名原路径', () => {
    const groups = parsePorcelainV1Z(FULL_PORCELAIN);
    expect(groups.staged.map((entry) => entry.path)).toEqual([
      'both.txt',
      'added.txt',
      'deleted.txt',
      'renamed.txt',
    ]);
    expect(groups.changes.map((entry) => entry.path)).toEqual(['changed.txt', 'both.txt']);
    expect(groups.untracked.map((entry) => entry.path)).toEqual(['new file.txt']);
    expect(groups.conflicts.map((entry) => entry.path)).toEqual(['conflict.txt']);
    const renamed = groups.staged.find((entry) => entry.path === 'renamed.txt');
    expect(renamed?.origPath).toBe('old name.txt');
    expect(renamed?.status).toBe('R');
    expect(renamed?.label).toBe('已重命名');
    expect(groups.conflicts[0].status).toBe('!');
    expect(groups.conflicts[0].label).toBe('冲突');
  });

  it('容错换行格式（桥接丢失 NUL 时）：引号路径与 rename 箭头', () => {
    const text = ' M "a b.txt"\nR  "old name.txt" -> "new name.txt"\n?? "d dir/"\nUU conflict.txt';
    const groups = parsePorcelainV1Z(text);
    expect(groups.changes[0].path).toBe('a b.txt');
    expect(groups.staged[0].path).toBe('new name.txt');
    expect(groups.staged[0].origPath).toBe('old name.txt');
    expect(groups.untracked[0].path).toBe('d dir/');
    expect(groups.conflicts[0].path).toBe('conflict.txt');
  });

  it('空输出返回空分组', () => {
    const groups = parsePorcelainV1Z('');
    expect(groups.staged).toEqual([]);
    expect(groups.changes).toEqual([]);
    expect(groups.untracked).toEqual([]);
    expect(groups.conflicts).toEqual([]);
  });

  it('applyDisplayPrefix 裁剪子目录工作区路径（含重命名原路径）', () => {
    const groups = parsePorcelainV1Z(' M sub/a.txt\u0000R  sub/new.txt\u0000sub/old.txt\u0000');
    applyDisplayPrefix(groups, 'sub');
    expect(groups.changes[0].displayPath).toBe('a.txt');
    expect(groups.staged[0].displayPath).toBe('new.txt');
    expect(groups.staged[0].origDisplayPath).toBe('old.txt');
    // 未匹配前缀的路径保持原样
    const other = parsePorcelainV1Z(' M other/x.txt\u0000');
    applyDisplayPrefix(other, 'sub');
    expect(other.changes[0].displayPath).toBe('other/x.txt');
  });
});

describe('parseGitCommandOutput（PTY 回显/ANSI/退出码裁剪）', () => {
  it('只取 BEGIN/END 之间的正文，带 ANSI 与 CRLF 也能解析退出码', () => {
    const raw =
      "$ printf '__PF_GIT_BEGIN__\\n'\r\n" +
      '\u001b[32m__PF_GIT_BEGIN__\u001b[0m\r\n' +
      '\u001b[31mfatal: 出错了\u001b[0m\r\n' +
      'second line\r\n' +
      '__PF_GIT_END__:3\r\n' +
      'admin@MINGW /d/Foo $ ';
    const parsed = parseGitCommandOutput(raw);
    expect(parsed.hasMarkers).toBe(true);
    expect(parsed.exitCode).toBe(3);
    expect(parsed.text).toBe('fatal: 出错了\nsecond line');
  });

  it('没有标记时原样返回 trim 文本且退出码为 null', () => {
    const parsed = parseGitCommandOutput('  plain output  \n');
    expect(parsed.hasMarkers).toBe(false);
    expect(parsed.exitCode).toBeNull();
    expect(parsed.text).toBe('plain output');
  });
});

describe('parseRepoSnapshot（只读快照解析）', () => {
  it('解析分支/上游/领先落后/status/log', () => {
    const raw = snapshotOutput({
      topLevel: '/repos/demo',
      branch: 'feature/x',
      upstream: 'origin/main',
      ab: '3\t5',
      status: FULL_PORCELAIN,
    });
    const parsed = parseGitCommandOutput(raw);
    const snapshot = parseRepoSnapshot(parsed.text, '/repos/demo');
    expect(snapshot.isRepo).toBe(true);
    expect(snapshot.topLevel).toBe('/repos/demo');
    expect(snapshot.branch.head).toBe('feature/x');
    expect(snapshot.branch.upstream).toBe('origin/main');
    expect(snapshot.branch.behind).toBe(3);
    expect(snapshot.branch.ahead).toBe(5);
    expect(snapshot.branch.detached).toBe(false);
    expect(snapshot.log).toHaveLength(2);
    expect(snapshot.log[0]).toEqual({ hash: 'abcdef0', subject: 'init commit' });
    expect(snapshot.total).toBe(8);
  });

  it('无上游时容错：upstream 为 null、ahead/behind 为 0', () => {
    const snapshot = parseRepoSnapshot(
      parseGitCommandOutput(snapshotOutput({ upstream: null })).text,
      '/repos/demo',
    );
    expect(snapshot.isRepo).toBe(true);
    expect(snapshot.branch.upstream).toBeNull();
    expect(snapshot.branch.ahead).toBe(0);
    expect(snapshot.branch.behind).toBe(0);
  });

  it('空仓库：initial=true、log 为空', () => {
    const snapshot = parseRepoSnapshot(
      parseGitCommandOutput(snapshotOutput({ log: false, branch: 'master' })).text,
      '/repos/demo',
    );
    expect(snapshot.isRepo).toBe(true);
    expect(snapshot.branch.initial).toBe(true);
    expect(snapshot.log).toEqual([]);
  });

  it('分离 HEAD：branch 输出 HEAD 且无 symref 时标记 detached', () => {
    const snapshot = parseRepoSnapshot(
      parseGitCommandOutput(snapshotOutput({ branchRaw: 'HEAD', branch: '' })).text,
      '/repos/demo',
    );
    expect(snapshot.isRepo).toBe(true);
    expect(snapshot.branch.detached).toBe(true);
    expect(snapshot.branch.head).toBe('');
  });

  it('非仓库：TOPLEVEL 报 fatal 时 isRepo=false 并带提示', () => {
    const raw = snapshotOutput({ topLevel: 'fatal: not a git repository (or any of the parent directories): .git' });
    const snapshot = parseRepoSnapshot(parseGitCommandOutput(raw).text, '/repos/demo');
    expect(snapshot.isRepo).toBe(false);
    expect(snapshot.message).toContain('not a git repository');
    expect(snapshot.total).toBe(0);
  });

  it('子目录工作区：status 路径裁剪为工作区相对路径', () => {
    const snapshot = parseRepoSnapshot(
      parseGitCommandOutput(snapshotOutput({ topLevel: '/repos/demo', status: ' M sub/a.txt\u0000' })).text,
      '/repos/demo/sub',
    );
    expect(snapshot.prefix).toBe('sub');
    expect(snapshot.groups.changes[0].path).toBe('sub/a.txt');
    expect(snapshot.groups.changes[0].displayPath).toBe('a.txt');
  });

  it('容忍 PTY 回显命令文本（标记出现在命令里）不误判分节', () => {
    const body = [
      '__PF_SEC__:TOPLEVEL',
      "printf '\\n__PF_SEC__:BRANCH\\n'",
      '/repos/demo',
      '__PF_SEC__:SYMREF',
      'main',
      '__PF_SEC__:BRANCH',
      'main',
      '__PF_SEC__:UPSTREAM',
      "fatal: no upstream configured for branch 'main'",
      '__PF_SEC__:AB',
      "fatal: no upstream configured for branch 'main'",
      '__PF_SEC__:STATUS',
      ' M a.txt\u0000',
      '__PF_SEC__:LOG',
      '2026-01-01 00:00:00 +0800',
      'abcdef0 init',
    ].join('\n');
    const snapshot = parseRepoSnapshot(body, '/repos/demo');
    expect(snapshot.isRepo).toBe(true);
    expect(snapshot.topLevel).toBe('/repos/demo');
    expect(snapshot.branch.head).toBe('main');
    expect(snapshot.branch.upstream).toBeNull();
    expect(snapshot.groups.changes.map((entry) => entry.path)).toEqual(['a.txt']);
    expect(snapshot.log).toHaveLength(1);
    expect(snapshot.log[0].hash).toBe('abcdef0');
  });

  it('splitSnapshotSections 能拆出分节', () => {
    const sections = splitSnapshotSections(
      parseGitCommandOutput(snapshotOutput({ branch: 'dev' })).text,
    );
    expect(Object.keys(sections)).toEqual(
      expect.arrayContaining(['TOPLEVEL', 'BRANCH', 'SYMREF', 'UPSTREAM', 'AB', 'STATUS', 'LOG']),
    );
    expect(sections.BRANCH.trim()).toBe('dev');
  });
});

describe('命令构造（build*Command）', () => {
  it('快照命令包含全部只读 git 命令、防挂起环境与 POSIX cd', () => {
    const command = buildSnapshotCommand('D:\\Workspace\\Project');
    expect(command).toContain("cd '/d/Workspace/Project'");
    expect(command).toContain('GIT_TERMINAL_PROMPT=0');
    expect(command).toContain('LC_ALL=C');
    expect(command).toContain('git rev-parse --show-toplevel');
    expect(command).toContain('git rev-parse --abbrev-ref HEAD');
    expect(command).toContain('git rev-parse --abbrev-ref --symbolic-full-name');
    expect(command).toContain("git rev-list --left-right --count '@{upstream}...HEAD'");
    expect(command).toContain('git status --porcelain=v1 -z');
    expect(command).toContain('git --no-pager log --oneline -20');
    expect(command).toContain('__PF_GIT_BEGIN__');
    expect(command).toContain('__PF_GIT_END__');
  });

  it('diff 命令按分组选择 --cached / --no-index，路径安全转义', () => {
    const staged = buildDiffCommand('/d/Foo', 'staged', 'dir/a b.txt');
    expect(staged).toContain('git --no-pager diff --no-color --cached --');
    expect(staged).toContain("'dir/a b.txt'");

    const changes = buildDiffCommand('/d/Foo', 'changes', 'a.txt');
    expect(changes).toContain('git --no-pager diff --no-color -- ');
    expect(changes).not.toContain('--cached');

    const conflict = buildDiffCommand('/d/Foo', 'conflicts', 'conflict.txt');
    expect(conflict).toContain('git --no-pager diff --no-color --');
    expect(conflict).not.toContain('--cached');

    const untracked = buildDiffCommand('/d/Foo', 'untracked', 'new file.txt');
    expect(untracked).toContain('git --no-pager diff --no-color --no-index -- /dev/null');
    expect(untracked).toContain("'new file.txt'");
  });

  it('工作区探测命令使用 git -C + rev-parse --is-inside-work-tree，并解析仓库下标', () => {
    const command = buildWorkspaceProbeCommand(['D:\\a', '/home/b']);
    expect(command).toContain('git -C "$p" rev-parse --is-inside-work-tree');
    expect(command).toContain("'/d/a'");
    expect(command).toContain("'/home/b'");
    expect(parseWorkspaceProbe(probeOutput([0, 2]))).toEqual([0, 2]);
    expect(parseWorkspaceProbe(probeOutput([]))).toEqual([]);
    expect(buildWorkspaceProbeCommand([])).toBe(':');
  });

  it('通用 git 命令包装：BEGIN/END、退出码捕获、stderr 合并', () => {
    const command = buildGitCommand('/repo', 'status --porcelain=v1 -z');
    expect(command).toContain('__PF_GIT_BEGIN__');
    expect(command).toContain('2>&1');
    expect(command).toContain('__PF_GIT_END__:%s');
    expect(command).toContain('"$?"');
  });
});

describe('安全白名单（禁止破坏性命令）', () => {
  it('assertSafeGitArgs 拒绝 reset/checkout/clean/restore/rebase 等', () => {
    for (const args of [
      'reset --hard HEAD',
      'checkout -- .',
      'restore a.txt',
      'clean -fd',
      'rebase -i main',
      'switch main',
      'stash drop',
    ]) {
      expect(() => assertSafeGitArgs(args), args).toThrow(/白名单|拒绝执行/);
    }
  });

  it('assertSafeGitArgs 拒绝强推，但允许普通 push/add/commit/diff', () => {
    expect(() => assertSafeGitArgs('push --force origin main')).toThrow(/强推|白名单/);
    expect(() => assertSafeGitArgs('push -f')).toThrow(/强推|白名单/);
    expect(() => assertSafeGitArgs('push')).not.toThrow();
    expect(() => assertSafeGitArgs('push --set-upstream origin main')).not.toThrow();
    expect(() => assertSafeGitArgs('add -- a.txt b.txt')).not.toThrow();
    expect(() => assertSafeGitArgs('commit -m "fix: 不要 git reset --hard"')).not.toThrow();
    expect(() => assertSafeGitArgs('--no-pager diff --no-color -- a.txt')).not.toThrow();
    expect(() => assertSafeGitArgs('status --porcelain=v1 -z')).not.toThrow();
  });

  it('buildGitCommand 对破坏性参数直接抛错', () => {
    expect(() => buildGitCommand('/repo', 'reset --hard')).toThrow();
    expect(() => buildGitCommand('/repo', 'checkout -- .')).toThrow();
    expect(() => buildGitCommand('/repo', 'clean -fd')).toThrow();
  });

  it('assertSafeCommand 拦截破坏性命令，但不误伤提交信息里的关键字', () => {
    expect(() => assertSafeCommand('git reset --hard HEAD')).toThrow();
    expect(() => assertSafeCommand('git clean -fdx')).toThrow();
    expect(() => assertSafeCommand('git checkout -- .')).toThrow();
    expect(() => assertSafeCommand('git push --force origin main')).toThrow();
    expect(() =>
      assertSafeCommand(buildGitCommand('/repo', `commit -m ${shellQuote('备注：不要 git reset --hard')}`)),
    ).not.toThrow();
    expect(() => assertSafeCommand(buildSnapshotCommand('/repo'))).not.toThrow();
  });
});

// ---------------------------------------------------------------------------
// 真实 bash 集成用例：默认跳过，本机/联调时 `PF_GIT_REAL_SHELL=1` 启用。
// ---------------------------------------------------------------------------
const REAL_SHELL_ENABLED = process.env.PF_GIT_REAL_SHELL === '1';
const realShellAvailable =
  REAL_SHELL_ENABLED &&
  (() => {
    try {
      return execFileSync('bash', ['-c', 'echo ok'], { encoding: 'utf8' }).trim() === 'ok';
    } catch {
      return false;
    }
  })();

function runScript(script: string): string {
  return execFileSync('bash', ['-s'], {
    input: script,
    encoding: 'utf8',
    maxBuffer: 16 * 1024 * 1024,
  });
}

describe.skipIf(!realShellAvailable)('真实 bash：命令 + 解析端到端', () => {
  it('在临时仓库上执行快照/探测/diff 命令并解析', () => {
    const base = mkdtempSync(join(tmpdir(), 'pf-git-real-'));
    const repo = join(base, 'repo space');
    mkdirSync(repo, { recursive: true });
    try {
      runScript(
        `cd ${shellQuote(repo)} && git init -q -b main && git config user.email t@t && git config user.name t ` +
          `&& printf 'a\\n' > a.txt && git add -A && git commit -qm init ` +
          `&& printf 'b\\n' >> a.txt && printf 'x\\n' > 'new file.txt'`,
      );

      const snapshot = parseRepoSnapshot(parseGitCommandOutput(runScript(buildSnapshotCommand(repo))).text, repo);
      expect(snapshot.isRepo).toBe(true);
      expect(snapshot.branch.head).toBe('main');
      expect(snapshot.groups.changes.map((entry) => entry.path)).toContain('a.txt');
      expect(snapshot.groups.untracked.map((entry) => entry.path)).toContain('new file.txt');
      expect(snapshot.log[0]?.subject).toBe('init');

      const changed = parseGitCommandOutput(runScript(buildDiffCommand(repo, 'changes', 'a.txt')));
      expect(changed.exitCode).toBe(0);
      expect(changed.text).toContain('+b');

      // --no-index 有差异时退出码 1，数据层把 0/1 都视为成功
      const untracked = parseGitCommandOutput(runScript(buildDiffCommand(repo, 'untracked', 'new file.txt')));
      expect(untracked.exitCode).toBe(1);
      expect(untracked.text).toContain('+x');

      const notRepo = join(base, 'not-a-repo');
      mkdirSync(notRepo, { recursive: true });
      const probe = parseGitCommandOutput(runScript(buildWorkspaceProbeCommand([repo, notRepo])));
      expect(parseWorkspaceProbe(probe.text)).toEqual([0]);
    } finally {
      rmSync(base, { recursive: true, force: true });
    }
  });
});

describe('GitHub 上下文 / 分支名 / PR 辅助', () => {
  it('parseGithubRemoteUrl 支持 https / ssh / scp 形态，并拒绝非 GitHub', () => {
    expect(parseGithubRemoteUrl('https://github.com/octocat/example-repo.git')).toEqual({
      host: 'github.com',
      owner: 'octocat',
      repo: 'example-repo',
    });
    expect(parseGithubRemoteUrl('git@github.com:owner/repo.git')?.repo).toBe('repo');
    expect(parseGithubRemoteUrl('ssh://git@github.com/owner/repo.git')?.owner).toBe('owner');
    expect(parseGithubRemoteUrl('https://gitlab.com/a/b.git')).toBeNull();
    expect(parseGithubRemoteUrl('')).toBeNull();
  });

  it('parseRemoteUrls / detectGithubContext 识别 fork（upstream 与 origin 不同）', () => {
    const remotes = [
      'origin\thttps://github.com/me/forked-repo.git (fetch)',
      'upstream\thttps://github.com/original/repo.git (fetch)',
    ].join('\n');
    expect(parseRemoteUrls(remotes).upstream).toContain('original/repo');
    const fork = detectGithubContext(remotes);
    expect(fork?.isFork).toBe(true);
    expect(fork?.baseRepo.owner).toBe('original');
    expect(fork?.origin.owner).toBe('me');

    const own = detectGithubContext('origin\tgit@github.com:me/repo.git (fetch)');
    expect(own?.isFork).toBe(false);
    expect(own?.baseRepo.owner).toBe('me');
    expect(detectGithubContext('origin\thttps://gitlab.com/a/b.git (fetch)')).toBeNull();
  });

  it('isValidGitBranchName 拒绝空格 / .. / 非法字符，接受 vcp/时间戳', () => {
    expect(isValidGitBranchName('vcp/20261107-183005')).toBe(true);
    expect(isValidGitBranchName('feature/foo-bar_1')).toBe(true);
    expect(isValidGitBranchName('')).toBe(false);
    expect(isValidGitBranchName('-bad')).toBe(false);
    expect(isValidGitBranchName('a..b')).toBe(false);
    expect(isValidGitBranchName('a b')).toBe(false);
    expect(isValidGitBranchName('a~b')).toBe(false);
    expect(isValidGitBranchName('a.lock')).toBe(false);
    expect(isValidGitBranchName('a/b/')).toBe(false);
  });

  it('defaultNewBranchName 生成 vcp/YYYYMMDD-HHMMSS', () => {
    expect(defaultNewBranchName(new Date(2026, 10, 7, 18, 30, 5))).toBe('vcp/20261107-183005');
  });

  it('formatGithubCompareUrl 自己仓库与 fork 两种 morph', () => {
    const own = detectGithubContext('origin\thttps://github.com/me/repo.git (fetch)')!;
    expect(formatGithubCompareUrl(own, 'main', 'vcp/abc')).toBe(
      'https://github.com/me/repo/compare/main...vcp%2Fabc?expand=1',
    );
    const fork = detectGithubContext(
      ['origin\thttps://github.com/me/fork.git (fetch)', 'upstream\thttps://github.com/up/repo.git (fetch)'].join('\n'),
    )!;
    expect(formatGithubCompareUrl(fork, 'main', 'vcp/abc')).toBe(
      'https://github.com/up/repo/compare/main...me:vcp%2Fabc?expand=1',
    );
  });

  it('extractPullRequestUrl / parseGhJsonOutput', () => {
    expect(
      extractPullRequestUrl('Creating pull request for vcp/abc into main\nhttps://github.com/up/repo/pull/42\n'),
    ).toBe('https://github.com/up/repo/pull/42');
    expect(extractPullRequestUrl('no url here')).toBeNull();
    expect(parseGhJsonOutput('{"defaultBranchRef":{"name":"main"}}')).toEqual({
      defaultBranchRef: { name: 'main' },
    });
  });

  it('assertSafeGhArgs 只允许白名单命令', () => {
    expect(() => assertSafeGhArgs('--version')).not.toThrow();
    expect(() => assertSafeGhArgs('auth status')).not.toThrow();
    expect(() => assertSafeGhArgs('repo view up/repo --json defaultBranchRef')).not.toThrow();
    expect(() => assertSafeGhArgs('pr create --fill --base main --head me:vcp/x --repo up/repo')).not.toThrow();
    expect(() => assertSafeGhArgs('pr merge 42')).toThrow();
    expect(() => assertSafeGhArgs('api /user')).toThrow();
    expect(() => assertSafeGhArgs('pr create --fill; rm -rf x')).toThrow();
  });

  it('buildGhCommand 输出带 BEGIN/END 的包装命令', () => {
    const command = buildGhCommand('/repos/demo', '--version');
    expect(command).toContain('__PF_GIT_BEGIN__');
    expect(command).toContain('gh --version 2>&1');
    expect(() => buildGhCommand('/repos/demo', 'pr merge 1')).toThrow();
  });
});

describe('分支 + PR 的 git 白名单与快照扩展', () => {
  it('git switch 仅允许 -c <新分支>，禁止切换已有分支', () => {
    expect(() => assertSafeGitArgs("switch -c 'vcp/20261107-183005'")).not.toThrow();
    expect(() => assertSafeGitArgs('switch main')).toThrow();
    expect(() => assertSafeGitArgs('switch --discard-changes main')).toThrow();
    expect(() => assertSafeCommand(buildGitCommand('/r', "switch -c 'vcp/x'"))).not.toThrow();
  });

  it('check-ref-format / remote 仅允许只读参数', () => {
    expect(() => assertSafeGitArgs("check-ref-format --branch 'vcp/x'")).not.toThrow();
    expect(() => assertSafeGitArgs('check-ref-format && rm -rf x')).toThrow();
    expect(() => assertSafeGitArgs('remote -v')).not.toThrow();
    expect(() => assertSafeGitArgs('remote add evil https://x')).toThrow();
  });

  it('快照命令包含 git remote -v，并解析到 remotes 字段', () => {
    const command = buildSnapshotCommand('/repos/demo');
    expect(command).toContain('git remote -v');
    const snapshot = parseRepoSnapshot(
      snapshotOutput({
        remotes: 'origin\tgit@github.com:me/repo.git (fetch)\nupstream\tgit@github.com:up/repo.git (fetch)',
      }),
    );
    expect(snapshot.remotes).toContain('github.com:up/repo.git');
  });
});
