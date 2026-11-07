/**
 * Git 模式单测公共 fixture：模拟 PTYShellExecutor 经桥接返回的命令输出。
 *
 * 真实链路里命令被包装成：
 *   printf '__PF_GIT_BEGIN__\n'
 *   ... 输出 ...
 *   printf '\n__PF_GIT_END__:<exit>\n'
 * 这里按同样形状构造原始文本，供解析与运行时测试使用。
 */

export interface SnapshotFixtureOptions {
  topLevel?: string;
  /** rev-parse --abbrev-ref HEAD 的输出（默认同 branch）。 */
  branchRaw?: string;
  /** symbolic-ref --short HEAD 的输出（默认同 branch）。 */
  branch?: string;
  /** 上游；null 表示无上游（输出 fatal 文本）。 */
  upstream?: string | null;
  /** rev-list --left-right --count 输出（「落后\\t领先」）；null 表示无上游。 */
  ab?: string | null;
  /** porcelain -z 原始文本。 */
  status?: string;
  /** git log --oneline 输出；false 表示空仓库 fatal。 */
  log?: string | false;
  /** git remote -v 输出；默认指向自己的 GitHub 仓库。 */
  remotes?: string | null;
}

/** 构造带 BEGIN/END 标记的 PTY 原始输出。 */
export function ptyOutput(body: string, exitCode = 0): string {
  return `${'__PF_GIT_BEGIN__'}\n${body}\n__PF_GIT_END__:${exitCode}\n`;
}

/** 构造带分节的只读快照输出。 */
export function snapshotOutput(options: SnapshotFixtureOptions = {}): string {
  const branch = options.branch ?? 'main';
  const sections: string[] = [`__PF_GIT_BEGIN__`];
  const push = (name: string, content: string): void => {
    sections.push(`\n__PF_SEC__:${name}\n${content}`);
  };
  push('TOPLEVEL', options.topLevel ?? '/repos/demo');
  push('BRANCH', options.branchRaw ?? branch);
  push('SYMREF', branch);
  if (options.upstream === null) {
    push('UPSTREAM', "fatal: no upstream configured for branch 'main'");
    push('AB', "fatal: no upstream configured for branch 'main'");
  } else {
    push('UPSTREAM', options.upstream ?? 'origin/main');
    push('AB', options.ab ?? '1\t2');
  }
  push('STATUS', options.status ?? '');
  push(
    'LOG',
    options.log === false
      ? "fatal: your current branch 'main' does not have any commits yet"
      : options.log ?? 'abcdef0 init commit\n1234567 second commit',
  );
  push(
    'REMOTE',
    options.remotes === null
      ? ''
      : options.remotes ??
        'origin\thttps://github.com/octocat/example-repo.git (fetch)\norigin\thttps://github.com/octocat/example-repo.git (push)',
  );
  sections.push(`\n${'__PF_GIT_END__'}:0\n`);
  return sections.join('');
}

/** 构造工作区探测输出：indices 为「是 git 仓库」的候选下标。 */
export function probeOutput(indices: number[]): string {
  const lines = indices.map((index) => `__PF_GIT_REPO__${index}`);
  return ptyOutput(lines.join('\n'));
}

/** 一组覆盖所有分组的 porcelain -z 文本。 */
export const FULL_PORCELAIN =
  ' M changed.txt\u0000' +
  'MM both.txt\u0000' +
  'A  added.txt\u0000' +
  'D  deleted.txt\u0000' +
  'R  renamed.txt\u0000old name.txt\u0000' +
  '?? new file.txt\u0000' +
  'UU conflict.txt\u0000';

/** 典型 unified diff 文本。 */
export const SAMPLE_DIFF = [
  'diff --git a/src/app.ts b/src/app.ts',
  'index 1111111..2222222 100644',
  '--- a/src/app.ts',
  '+++ b/src/app.ts',
  '@@ -1,3 +1,4 @@',
  ' const a = 1;',
  '-const b = 2;',
  '+const b = 3;',
  '+const c = 4;',
  ' export {};',
  '',
].join('\n');
