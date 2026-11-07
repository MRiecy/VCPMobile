/**
 * V工程 · Git 模式数据逻辑（对齐桌面 VCPChat「V工程 / Git」侧栏）。
 *
 * 数据通道：
 *   桌面 VCPChat 的 Git 面板走 Electron 内部 IPC（手机桥不暴露），手机端改用桌面插件
 *   `PTYShellExecutor`（Windows 侧已修复为 Git Bash）经 MobilePluginBridge `/invoke` 执行 git 命令。
 *   本模块只负责「构造命令 + 解析输出 + 维护状态」，真正的执行器由调用方注入
 *   （默认由 `PfGitPanel.vue` 注入 `projectForgeApi.ptyExec`），因此单测可用假执行器覆盖。
 *
 * 安全约束（详见 assertSafeCommand / assertSafeGitArgs）：
 *   - 只允许只读命令（rev-parse / rev-list / status / diff / log / symbolic-ref）与用户明确
 *     二次确认后的写命令（add / commit / push）；
 *   - reset / checkout / restore / clean / rebase / merge / switch / stash 等破坏性命令一律拒绝；
 *   - 每条命令在目标仓库内执行并前置 `GIT_TERMINAL_PROMPT=0 LC_ALL=C GIT_PAGER=cat`，
 *     防止凭据交互、分页器导致 PTY 挂起；
 *   - Windows 路径统一转成 Git Bash 的 POSIX 形式（`D:\Foo` → `/d/Foo`）。
 *
 * 输出协议（防止 PTY 回显/prompt 污染数据）：
 *   命令由本模块包装为 `printf '__PF_GIT_BEGIN__\n' ... printf '\n__PF_GIT_END__:<code>\n'`，
 *   解析时只取 BEGIN/END 之间的内容并读取退出码；兼容 PTY 的 ANSI 转义与 CRLF。
 */
import { computed, reactive, ref } from 'vue';

// ============================ 常量 ============================

/** 命令输出开始标记（PTY 回显里也会出现，解析时取最后一次）。 */
export const PF_GIT_BEGIN = '__PF_GIT_BEGIN__';
/** 命令输出结束标记，尾部带真实退出码。 */
export const PF_GIT_END = '__PF_GIT_END__';
/** 只读快照的分节标记。 */
export const PF_GIT_SECTION = '__PF_SEC__';
/** 工作区扫描结果行前缀（值是被检测路径在候选数组中的下标）。 */
export const PF_GIT_REPO = '__PF_GIT_REPO__';
/** 工作区选择持久化 key。 */
export const PF_GIT_STORAGE_KEY = 'vcp.projectForge.git.workspace.v1';
/** 快照/差异等只读命令超时。 */
const READ_TIMEOUT_MS = 45_000;
/** 工作区批量探测超时。 */
const PROBE_TIMEOUT_MS = 60_000;
/** 提交（add+commit）超时。 */
const COMMIT_TIMEOUT_MS = 120_000;
/** 推送超时（网络慢时可能较久）。 */
const PUSH_TIMEOUT_MS = 180_000;

/** 桌面端分组顺序。 */
export const PF_GIT_GROUP_ORDER: readonly PfGitGroup[] = ['conflicts', 'staged', 'changes', 'untracked'] as const;

/** 分组元信息（标题与差异图例）。 */
export const PF_GIT_GROUPS: readonly { key: PfGitGroup; title: string; legendLeft: string; legendRight: string }[] = [
  { key: 'conflicts', title: '合并冲突', legendLeft: '暂存区（ours）', legendRight: '工作区' },
  { key: 'staged', title: '暂存的更改', legendLeft: 'HEAD', legendRight: '暂存区' },
  { key: 'changes', title: '更改', legendLeft: '暂存区', legendRight: '工作区' },
  { key: 'untracked', title: '未跟踪文件', legendLeft: '（无）', legendRight: '新建文件' },
];

/** porcelain 状态字母 → 中文。 */
const STATUS_LABELS: Record<string, string> = {
  M: '已修改',
  A: '已添加',
  D: '已删除',
  R: '已重命名',
  C: '已复制',
  T: '类型变更',
  U: '冲突',
  '?': '未跟踪',
  '!': '冲突',
};

/** git 中代表「未合并冲突」的状态对。 */
const CONFLICT_PAIRS = new Set(['DD', 'AU', 'UD', 'UA', 'DU', 'AA', 'UU']);

/** 允许的 git 子命令白名单（其余一律拒绝）。 */
const ALLOWED_GIT_SUBCOMMANDS = new Set([
  'rev-parse',
  'rev-list',
  'status',
  'diff',
  'log',
  'add',
  'commit',
  'push',
  'symbolic-ref',
  // 2026-11-07：分支+PR 工作流（参数级白名单见 assertSafeGitArgs）
  'switch',
  'check-ref-format',
  'remote',
]);

/** 破坏性命令模式（在去掉单引号内容后的命令骨架上匹配）。 */
const FORBIDDEN_COMMAND_PATTERNS: readonly RegExp[] = [
  /\bgit\s+(?:-\S+\s+)*(?:reset|checkout|restore|clean|rebase|merge|stash|revert|cherry-pick|filter-branch|update-ref|gc|prune|rm|mv|am|apply|worktree|submodule)\b/i,
  /\bgit\s+push\b[^|;&]*\s(?:--force(?:-with-lease)?|-f)(?:\s|$)/i,
  /\brm\s+-[a-zA-Z]*[rf][a-zA-Z]*\b/i,
];

/** 只读快照里包含的分节。 */
type SnapshotSection = 'TOPLEVEL' | 'BRANCH' | 'SYMREF' | 'UPSTREAM' | 'AB' | 'STATUS' | 'LOG' | 'REMOTE';

// ============================ 类型 ============================

export type PfGitGroup = 'conflicts' | 'staged' | 'changes' | 'untracked';

/** 单个变更文件（可能同时出现在 staged 与 changes，例如 `MM`）。 */
export interface PfGitFileEntry {
  /** 仓库根相对路径（porcelain 输出的原始路径，git 命令直接使用）。 */
  path: string;
  /** UI 展示路径：子目录工作区时裁掉仓库根前缀。 */
  displayPath: string;
  /** 重命名/复制的原路径（仓库根相对）。 */
  origPath?: string;
  /** 重命名/复制的原展示路径。 */
  origDisplayPath?: string;
  /** 展示用状态字母（M/A/D/R/C/T/U/!）。 */
  status: string;
  /** 状态中文名。 */
  label: string;
  /** 所属分组。 */
  group: PfGitGroup;
}

export interface PfGitGroups {
  conflicts: PfGitFileEntry[];
  staged: PfGitFileEntry[];
  changes: PfGitFileEntry[];
  untracked: PfGitFileEntry[];
}

export interface PfGitBranchInfo {
  /** 分支名；分离 HEAD 时为空。 */
  head: string;
  /** 上游分支全名（如 origin/main），无上游为 null。 */
  upstream: string | null;
  /** 是否处于分离 HEAD。 */
  detached: boolean;
  /** 是否尚无任何提交（空仓库）。 */
  initial: boolean;
  /** 领先上游的提交数。 */
  ahead: number;
  /** 落后上游的提交数。 */
  behind: number;
}

export interface PfGitLogEntry {
  hash: string;
  subject: string;
}

export interface PfGitRepoSnapshot {
  isRepo: boolean;
  /** 仓库根目录（Git Bash/POSIX 路径）；不是仓库时为空。 */
  topLevel: string;
  /** 工作区相对仓库根的展示前缀（子目录工作区时非空）。 */
  prefix: string;
  groups: PfGitGroups;
  branch: PfGitBranchInfo;
  log: PfGitLogEntry[];
  total: number;
  /** 不是仓库/读取失败时的提示。 */
  message?: string;
  /** `git remote -v` 原始输出（识别 GitHub / fork 上下文用）。 */
  remotes?: string;
}

export interface PfGitWorkspaceRef {
  id?: string;
  alias?: string;
  path: string;
}

/** 已确认的 Git 工作区。 */
export interface PfGitWorkspace {
  id: string;
  alias: string;
  /** 原始路径（ListWorkspaces 或手动输入，用于展示）。 */
  path: string;
  /** 转换后的 POSIX 路径（用于 cd / git -C）。 */
  shellPath: string;
  /** 是否手动输入的路径。 */
  manual: boolean;
}

/** 执行器返回（结构上兼容 projectForgeApi.ptyExec 的返回值）。 */
export interface PfGitExecResult {
  text?: string;
  ok?: boolean;
  exitCode?: number | null;
}

/** 命令执行器：由调用方注入（默认 projectForgeApi.ptyExec）。 */
export type PfGitExec = (command: string, options?: { timeoutMs?: number }) => Promise<PfGitExecResult>;

/** 二次确认（默认由 PfGitPanel 注入全局确认弹层；未注入时默认拒绝，避免误写）。 */
export interface PfGitConfirmOptions {
  title: string;
  message: string;
  isDanger?: boolean;
}

export type PfGitConfirm = (options: PfGitConfirmOptions) => Promise<boolean>;

export interface PfGitRuntimeDeps {
  /** 必填：命令执行器。 */
  exec: PfGitExec;
  /** 可选：列出 ProjectForge 工作区（ListWorkspaces）。 */
  listWorkspaces?: () => Promise<PfGitWorkspaceRef[]>;
  /** 可选：二次确认；缺省时确认类操作一律取消。 */
  confirm?: PfGitConfirm;
}

export interface PfGitNotice {
  type: 'info' | 'success' | 'warning' | 'error';
  text: string;
}

export interface PfGitOpOutput {
  title: string;
  /** 合并后的 stdout/stderr 原文。 */
  text: string;
  ok: boolean;
  /** 人类可读的命令摘要（用于输出面板展示）。 */
  command: string;
  finishedAt: number;
}

export interface PfGitActiveDiff {
  group: PfGitGroup;
  /** 仓库根相对路径（git 命令用）。 */
  path: string;
  displayPath: string;
  status: string;
  label: string;
}

/** 单条命令的解析结果。 */
export interface PfGitCommandOutput {
  /** BEGIN/END 之间的正文（已去 ANSI、统一 LF、去首尾空行）。 */
  text: string;
  /** 真实退出码；没有 END 标记时为 null。 */
  exitCode: number | null;
  /** 是否找到了本模块的输出标记。 */
  hasMarkers: boolean;
}

// ============================ 基础工具 ============================

/** 把任意异常转成可读文案。 */
export function errorMessageOf(error: unknown): string {
  if (error instanceof Error) return error.message || error.name;
  if (typeof error === 'string') return error;
  if (error && typeof error === 'object') {
    const message = (error as { message?: unknown }).message;
    if (typeof message === 'string') return message;
  }
  return String(error ?? '未知错误');
}

/** 去掉终端 ANSI 转义（PTY 输出可能带颜色/光标控制）。 */
export function stripAnsi(input: string): string {
  return String(input ?? '')
    .replace(/\u001B\][^\u0007]*(?:\u0007|\u001B\\)/g, '')
    .replace(/\u001B\[[0-9;?]*[ -/]*[@-~]/g, '')
    .replace(/\u001B[@-_]/g, '');
}

/** 取字符串第一行（报错摘要用）。 */
export function firstLineOf(input: unknown): string {
  const text = String(input ?? '').trim();
  if (!text) return '';
  return text.split('\n')[0].trim();
}

/** 路径最后一段（工作区别名兜底）。 */
export function lastSegmentOfPath(input: string): string {
  const normalized = String(input ?? '').trim().replace(/[\\/]+$/, '');
  const parts = normalized.split(/[\\/]/).filter(Boolean);
  return parts.length ? parts[parts.length - 1] : '';
}

/**
 * 路径统一转换成 shell 可用形式：
 *   - Windows 盘符：`D:\Foo\Bar` / `D:/Foo/Bar` → `/d/Foo/Bar`（Git Bash / MSYS 形式）；
 *   - UNC：`\\server\share\x` → `//server/share/x`；
 *   - 已是 POSIX 绝对路径（`/home/...`、`/d/...`）或 `~/` 原样保留；
 *   - 其他把反斜杠换成正斜杠，并去掉末尾多余斜杠。
 * 空路径返回空字符串。
 */
export function toShellPath(raw: unknown): string {
  let value = String(raw ?? '').trim();
  if (!value) return '';
  // 去掉用户粘进来的成对引号
  if (
    (value.startsWith('"') && value.endsWith('"') && value.length >= 2) ||
    (value.startsWith("'") && value.endsWith("'") && value.length >= 2)
  ) {
    value = value.slice(1, -1).trim();
  }
  if (!value) return '';

  const drive = /^([A-Za-z]):(?:[\\/]|$)/.exec(value);
  if (drive) {
    const rest = value
      .slice(2)
      .replace(/\\/g, '/')
      .replace(/^\/+/, '')
      .replace(/\/+$/, '');
    return `/${drive[1].toLowerCase()}${rest ? `/${rest}` : ''}`;
  }

  if (/^\\\\/.test(value)) {
    const rest = value.slice(2).replace(/\\/g, '/').replace(/\/+$/, '');
    return `//${rest}`;
  }

  if (value.startsWith('/') || value.startsWith('~/')) {
    if (value.length <= 1) return value;
    return value.replace(/\/+$/, '') || '/';
  }

  return value.replace(/\\/g, '/').replace(/\/+$/, '');
}

/** 单引号安全转义（用于 shell 参数）。 */
export function shellQuote(value: unknown): string {
  return `'${String(value ?? '').replace(/'/g, `'\\''`)}'`;
}

/** 计算工作区相对仓库根的展示前缀（工作区是仓库子目录时非空，否则空字符串）。 */
export function relativePrefix(topLevel: string, workspaceShellPath: string): string {
  const top = String(topLevel ?? '').replace(/\\/g, '/').replace(/\/+$/, '');
  const workspace = String(workspaceShellPath ?? '').replace(/\\/g, '/').replace(/\/+$/, '');
  if (!top || !workspace) return '';
  if (workspace.toLowerCase() === top.toLowerCase()) return '';
  const lowerTop = top.toLowerCase();
  const lowerWorkspace = workspace.toLowerCase();
  if (lowerWorkspace.startsWith(`${lowerTop}/`)) {
    return workspace.slice(top.length + 1);
  }
  // 工作区是仓库父目录（仓库在工作区里面）：路径本身已相对仓库根，不需要裁剪
  return '';
}

/** 去掉单引号包裹的内容，只保留「命令骨架」用于安全扫描（避免提交信息里的关键字误伤）。 */
function commandSkeleton(command: string): string {
  const segments = String(command ?? '').split("'");
  return segments.map((segment, index) => (index % 2 === 0 ? segment : "''")).join("'");
}

/** 是否为 git/系统错误行。 */
function isErrorLine(line: string): boolean {
  const text = String(line ?? '').trim();
  if (!text) return false;
  return /^(fatal|error|warning|hint|usage|note):/i.test(text) || /^git:/i.test(text);
}

/** 取分节内容里第一个「非空且非错误行」的文本。 */
function firstCleanLine(section: string | undefined): string {
  const lines = String(section ?? '')
    .replace(/\r/g, '')
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line && !isErrorLine(line));
  return lines.length ? lines[lines.length - 1] : '';
}

/** 判断分节读取出错（git 报 fatal/error）。 */
function sectionHasError(section: string | undefined): boolean {
  return String(section ?? '')
    .split('\n')
    .some((line) => isErrorLine(line));
}

// ============================ 安全校验 ============================

/**
 * 校验 git 参数只使用白名单子命令，并拒绝 force push。
 * 在构造命令时调用（throw 表示拒绝）。
 */
export function assertSafeGitArgs(gitArgs: string): void {
  const tokens = String(gitArgs ?? '').match(/'[^']*'|"[^"]*"|\S+/g) ?? [];
  let subcommand = '';
  for (const token of tokens) {
    if (token.startsWith('-')) continue;
    subcommand = token.replace(/^['"]|['"]$/g, '');
    break;
  }
  if (!subcommand) throw new Error('拒绝执行：缺少 git 子命令');
  if (!ALLOWED_GIT_SUBCOMMANDS.has(subcommand)) {
    throw new Error(`拒绝执行：git ${subcommand} 不在允许的命令白名单内（只允许只读命令与 add/commit/push）`);
  }
  if (subcommand === 'push' && /(?:^|\s)(?:--force(?:-with-lease)?|-f)(?:\s|$)/.test(gitArgs)) {
    throw new Error('拒绝执行：不允许强推（--force / -f）');
  }
  const rest = String(gitArgs ?? '').trim().slice(subcommand.length).trim();
  if (subcommand === 'switch' && !/^-c\s+\S+$/.test(rest)) {
    throw new Error('拒绝执行：git switch 仅允许 `git switch -c <新分支>`（禁止切换已有分支/丢弃改动）');
  }
  if (subcommand === 'check-ref-format' && !/^--branch\s+\S+$/.test(rest)) {
    throw new Error('拒绝执行：git check-ref-format 仅允许 `--branch <分支名>`');
  }
  if (subcommand === 'remote' && rest !== '-v') {
    throw new Error('拒绝执行：git remote 仅允许 `-v` 查询');
  }
}

/** 防御性命令扫描：拒绝任何破坏性命令骨架（README 约定 + 双保险）。 */
export function assertSafeCommand(command: string): void {
  const skeleton = commandSkeleton(command);
  for (const pattern of FORBIDDEN_COMMAND_PATTERNS) {
    if (pattern.test(skeleton)) {
      throw new Error('拒绝执行：该命令包含破坏性操作（reset/checkout/clean 等），Git 模式不允许');
    }
  }
}

// ============================ 命令构造 ============================

const ENV_EXPORTS = 'GIT_TERMINAL_PROMPT=0 LC_ALL=C GIT_PAGER=cat GIT_ASKPASS=true GCM_INTERACTIVE=never';

/** 构造 git 命令的公共骨架：cd 仓库 → 设置防挂起环境 → 执行 → 输出退出码标记。 */
function wrapGitCommand(repoShellPath: string, body: string): string {
  return [
    `printf '${PF_GIT_BEGIN}\\n'`,
    `if cd ${shellQuote(repoShellPath)} 2>/dev/null; then`,
    `  export ${ENV_EXPORTS}`,
    `  ${body}`,
    `  printf '\\n${PF_GIT_END}:%s\\n' "$?"`,
    'else',
    `  printf '\\n${PF_GIT_END}:97\\n'`,
    'fi',
  ].join('\n');
}

/** 允许的 gh 命令形态（分支+PR 工作流最小权限）。 */
const GH_ARGS_PATTERN =
  /^(?:--version|auth status|repo view [\w.-]+\/[\w.-]+ --json defaultBranchRef|pr create(?: --(?:fill|draft)| --base [\w./-]+| --head [\w./:-]+| --repo [\w.-]+\/[\w.-]+)+)$/;

/** 校验 gh 参数：只允许版本/登录状态/默认分支查询/PR 创建。 */
export function assertSafeGhArgs(ghArgs: unknown): void {
  const text = String(ghArgs ?? '').trim();
  if (!text) throw new Error('拒绝执行：缺少 gh 参数');
  if (/[;|&$`()<>\n]/.test(text)) {
    throw new Error('拒绝执行：gh 参数包含非法字符');
  }
  if (!GH_ARGS_PATTERN.test(text)) {
    throw new Error('拒绝执行：gh 命令不在允许范围内（仅支持 --version / auth status / repo view / pr create）');
  }
}

/** 构造 gh 命令（与 git 同款 BEGIN/END 包装，防 PTY 回显污染）。 */
export function buildGhCommand(repoShellPath: string, ghArgs: string): string {
  const shellPath = toShellPath(repoShellPath);
  if (!shellPath) throw new Error('缺少 Git 仓库路径');
  assertSafeGhArgs(ghArgs);
  return wrapGitCommand(shellPath, `gh ${ghArgs} 2>&1`);
}

/** 构造单条 git 命令（供 diff / add / commit / push 使用）。 */
export function buildGitCommand(repoShellPath: string, gitArgs: string): string {
  const shellPath = toShellPath(repoShellPath);
  if (!shellPath) throw new Error('缺少 Git 仓库路径');
  assertSafeGitArgs(gitArgs);
  return wrapGitCommand(shellPath, `git ${gitArgs} 2>&1`);
}

/** 构造只读快照命令（一次 PTY 往返拿到 仓库根/分支/上游/ahead-behind/status/log）。 */
export function buildSnapshotCommand(repoShellPath: string): string {
  const shellPath = toShellPath(repoShellPath);
  if (!shellPath) throw new Error('缺少 Git 仓库路径');
  const sections: { name: SnapshotSection; command: string }[] = [
    { name: 'TOPLEVEL', command: 'git rev-parse --show-toplevel' },
    { name: 'BRANCH', command: 'git rev-parse --abbrev-ref HEAD' },
    { name: 'SYMREF', command: 'git symbolic-ref --short -q HEAD' },
    { name: 'UPSTREAM', command: "git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}'" },
    { name: 'AB', command: "git rev-list --left-right --count '@{upstream}...HEAD'" },
    { name: 'STATUS', command: 'git status --porcelain=v1 -z' },
    { name: 'LOG', command: 'git --no-pager log --oneline -20' },
    { name: 'REMOTE', command: 'git remote -v' },
  ];
  return [
    `printf '${PF_GIT_BEGIN}\\n'`,
    `if cd ${shellQuote(shellPath)} 2>/dev/null; then`,
    `  export ${ENV_EXPORTS}`,
    ...sections.map(
      (section) => `  printf '\\n${PF_GIT_SECTION}:${section.name}\\n'\n  ${section.command} 2>&1`,
    ),
    `  printf '\\n${PF_GIT_END}:0\\n'`,
    'else',
    `  printf '\\n${PF_GIT_END}:97\\n'`,
    'fi',
  ].join('\n');
}

/** 构造批量工作区探测命令：对每个候选路径执行 `git -C <posix路径> rev-parse --is-inside-work-tree`。 */
export function buildWorkspaceProbeCommand(shellPaths: readonly string[]): string {
  const paths = (shellPaths ?? []).map((item) => toShellPath(item)).filter(Boolean);
  if (!paths.length) return ':';
  return [
    `printf '${PF_GIT_BEGIN}\\n'`,
    'i=0',
    `for p in ${paths.map((item) => shellQuote(item)).join(' ')}; do`,
    `  if GIT_TERMINAL_PROMPT=0 LC_ALL=C git -C "$p" rev-parse --is-inside-work-tree >/dev/null 2>&1; then printf '${PF_GIT_REPO}%s\\n' "$i"; fi`,
    '  i=$((i+1))',
    'done',
    `printf '\\n${PF_GIT_END}:0\\n'`,
  ].join('\n');
}

/** 构造单个文件的 diff 命令（按分组选择 git 参数）。 */
export function buildDiffCommand(repoShellPath: string, group: PfGitGroup, path: string): string {
  const shellPath = toShellPath(repoShellPath);
  if (!shellPath) throw new Error('缺少 Git 仓库路径');
  if (!path) throw new Error('缺少文件路径');
  if (group === 'untracked') {
    // --no-index 在「有差异」时退出码为 1，解析层把 0/1 都视为成功
    return wrapGitCommand(
      shellPath,
      `git --no-pager diff --no-color --no-index -- /dev/null ${shellQuote(path)} 2>&1`,
    );
  }
  const cached = group === 'staged' ? ' --cached' : '';
  return wrapGitCommand(shellPath, `git --no-pager diff --no-color${cached} -- ${shellQuote(path)} 2>&1`);
}

// ============================ GitHub 识别 / 分支名 / PR 辅助 ============================

/** GitHub 仓库引用。 */
export interface PfGithubRepoRef {
  host: string;
  owner: string;
  repo: string;
}

/** 当前仓库的 GitHub 关系：origin 必填；检测到 upstream 时视为 fork。 */
export interface PfGithubContext {
  origin: PfGithubRepoRef;
  upstream: PfGithubRepoRef | null;
  isFork: boolean;
  /** PR 要开到的仓库：fork 时为 upstream，否则 origin。 */
  baseRepo: PfGithubRepoRef;
}

/** PR 创建结果（用于 UI 提示与降级链接）。 */
export interface PfGitPrResult {
  ok: boolean;
  url?: string;
  message: string;
}

const GITHUB_HOST = 'github.com';

/** 解析 GitHub 远端 URL（https / ssh / scp 形态）；非 GitHub 返回 null。 */
export function parseGithubRemoteUrl(raw: unknown): PfGithubRepoRef | null {
  const text = String(raw ?? '').trim();
  if (!text) return null;
  let host = '';
  let owner = '';
  let repo = '';
  const urlMatch = /^(?:https?|ssh):\/\/(?:[^@/]+@)?([^/:]+)(?::\d+)?\/([^/]+)\/([^/]+?)(?:\.git)?\/?$/.exec(text);
  const scpMatch = /^(?:[^@/]+@)?([^:/]+):\/?([^/]+)\/([^/]+?)(?:\.git)?\/?$/.exec(text);
  if (urlMatch) {
    host = urlMatch[1];
    owner = urlMatch[2];
    repo = urlMatch[3];
  } else if (scpMatch) {
    host = scpMatch[1];
    owner = scpMatch[2];
    repo = scpMatch[3];
  } else {
    return null;
  }
  if (host.toLowerCase() !== GITHUB_HOST) return null;
  if (!owner || !repo) return null;
  return { host: GITHUB_HOST, owner, repo };
}

/** 从 `git remote -v` 输出解析 remote 名 → URL（同名取第一条）。 */
export function parseRemoteUrls(remotesOutput: unknown): Record<string, string> {
  const result: Record<string, string> = {};
  for (const rawLine of String(remotesOutput ?? '').split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line) continue;
    const match = /^(\S+)\s+(\S+)/.exec(line);
    if (!match) continue;
    const name = match[1];
    const url = match[2];
    if (!(name in result)) result[name] = url;
  }
  return result;
}

/** 综合 origin/upstream 判定 GitHub 与 fork 上下文；非 GitHub 返回 null。 */
export function detectGithubContext(remotesOutput: unknown): PfGithubContext | null {
  const remotes = parseRemoteUrls(remotesOutput);
  const origin = parseGithubRemoteUrl(remotes.origin);
  if (!origin) return null;
  const upstream = parseGithubRemoteUrl(remotes.upstream);
  const isFork = Boolean(upstream) && (upstream!.owner !== origin.owner || upstream!.repo !== origin.repo);
  return { origin, upstream: isFork ? upstream : null, isFork, baseRepo: isFork ? upstream! : origin };
}

/** 本地校验 git 分支名（与 git check-ref-format 主要规则一致，最终仍以服务端命令为准）。 */
export function isValidGitBranchName(name: unknown): boolean {
  const text = String(name ?? '').trim();
  if (!text || text.length > 120) return false;
  if (text === '@') return false;
  if (/^[-.]/.test(text)) return false;
  if (/\.\./.test(text)) return false;
  if (text.includes('@{')) return false;
  if (text.endsWith('.lock') || text.endsWith('.') || text.endsWith('/')) return false;
  if (text.startsWith('/') || text.includes('//')) return false;
  // ASCII 控制字符 / 空格 / ~ ^ : ? * [ \ / 结尾等
  if (/[\x00-\x20\x7f~^:?*[\\]/.test(text)) return false;
  return true;
}

/** 默认修改分支名：vcp/YYYYMMDD-HHMMSS。 */
export function defaultNewBranchName(date: Date = new Date()): string {
  const pad = (value: number): string => String(value).padStart(2, '0');
  const stamp = `${date.getFullYear()}${pad(date.getMonth() + 1)}${pad(date.getDate())}-${pad(date.getHours())}${pad(date.getMinutes())}${pad(date.getSeconds())}`;
  return `vcp/${stamp}`;
}

/** 生成 GitHub compare（手动开 PR）链接，作为 gh 不可用时的降级方案。 */
export function formatGithubCompareUrl(
  context: PfGithubContext,
  baseBranch: string,
  headBranch: string,
): string {
  const base = `${context.baseRepo.owner}/${context.baseRepo.repo}`;
  const head = context.isFork
    ? `${context.origin.owner}:${encodeURIComponent(headBranch)}`
    : encodeURIComponent(headBranch);
  return `https://github.com/${base}/compare/${encodeURIComponent(baseBranch)}...${head}?expand=1`;
}

/** 从 gh 输出提取 PR 链接。 */
export function extractPullRequestUrl(text: unknown): string | null {
  const match = /https:\/\/github\.com\/[^\s"')\]]+\/pull\/\d+/i.exec(String(text ?? ''));
  return match ? match[0] : null;
}

/** 解析 gh 输出里的 JSON（gh --json 输出）。 */
export function parseGhJsonOutput(text: unknown): unknown {
  const raw = String(text ?? '').trim();
  if (!raw) return null;
  const start = raw.search(/[[{]/);
  if (start < 0) return null;
  const candidate = raw.slice(start);
  try {
    return JSON.parse(candidate);
  } catch {
    return null;
  }
}

// ============================ 输出解析 ============================

/** 解析包装命令的输出：去 ANSI/CRLF，取 BEGIN/END 之间的正文与退出码。 */
export function parseGitCommandOutput(raw: unknown): PfGitCommandOutput {
  const cleaned = stripAnsi(String(raw ?? ''))
    .replace(/\r\n/g, '\n')
    .replace(/\r/g, '\n');
  const endPattern = new RegExp(`${PF_GIT_END}:(\\d+)`, 'g');
  let endCode: number | null = null;
  let endIndex = -1;
  let match: RegExpExecArray | null;
  while ((match = endPattern.exec(cleaned))) {
    endCode = Number(match[1]);
    endIndex = match.index;
  }
  const beginIndex = cleaned.lastIndexOf(PF_GIT_BEGIN);
  if (beginIndex === -1 || endIndex <= beginIndex) {
    return { text: cleaned.trim(), exitCode: null, hasMarkers: false };
  }
  let body = cleaned.slice(beginIndex + PF_GIT_BEGIN.length, endIndex);
  body = body.replace(/^\n/, '').replace(/\n+$/, '');
  return { text: body, exitCode: endCode, hasMarkers: true };
}

/**
 * 把快照输出拆成 { 分节名: 内容 }。
 * 优先按「整行即标记」的严格形态解析（PTY 若回显命令，标记文本会粘在命令里，严格模式可避免误判）；
 * 若一条都没匹配到，再退化为宽松子串匹配（兼容 prompt 粘在标记行前的实现）。
 */
export function splitSnapshotSections(text: string): Record<string, string> {
  const body = String(text ?? '');
  const collect = (pattern: RegExp): { name: string; index: number; length: number }[] => {
    const matches: { name: string; index: number; length: number }[] = [];
    let match: RegExpExecArray | null;
    while ((match = pattern.exec(body))) {
      matches.push({ name: match[1], index: match.index ?? 0, length: match[0].length });
    }
    return matches;
  };
  let matches = collect(new RegExp(`^${PF_GIT_SECTION}:([A-Z_]+)`, 'gm'));
  if (!matches.length) matches = collect(new RegExp(`${PF_GIT_SECTION}:([A-Z_]+)`, 'g'));
  const result: Record<string, string> = {};
  matches.forEach((item, index) => {
    const start = item.index + item.length;
    const end = index + 1 < matches.length ? matches[index + 1].index : body.length;
    result[item.name] = body
      .slice(start, end)
      .replace(/^\n/, '')
      .replace(/\n+$/, '');
  });
  return result;
}

/** 解析工作区探测结果，返回「是 git 仓库」的候选下标。 */
export function parseWorkspaceProbe(text: string): number[] {
  const indices: number[] = [];
  const body = String(text ?? '');
  const inner = body.includes(PF_GIT_BEGIN) ? parseGitCommandOutput(body).text : body;
  const pattern = new RegExp(`${PF_GIT_REPO}\\s*(\\d+)`, 'g');
  let match: RegExpExecArray | null;
  while ((match = pattern.exec(inner))) indices.push(Number(match[1]));
  return indices;
}

/** 去掉 porcelain（非 -z 模式）对路径的 JSON 引号。 */
function unquotePorcelainPath(value: string): string {
  const text = String(value ?? '').trim();
  if (text.length >= 2 && text.startsWith('"') && text.endsWith('"')) {
    try {
      const parsed = JSON.parse(text);
      if (typeof parsed === 'string') return parsed;
    } catch {
      return text.slice(1, -1);
    }
  }
  return text;
}

/** 创建空分组（每次返回新对象）。 */
export function createEmptyGroups(): PfGitGroups {
  return { conflicts: [], staged: [], changes: [], untracked: [] };
}

function makeEntry(group: PfGitGroup, status: string, path: string, origPath?: string): PfGitFileEntry {
  return {
    path,
    displayPath: path,
    origPath,
    origDisplayPath: origPath,
    status,
    label: STATUS_LABELS[status] || status,
    group,
  };
}

/**
 * 解析 `git status --porcelain=v1 -z`（也容错非 -z 的按行格式）。
 * 分组规则：
 *   - 冲突状态（DD/AU/UD/UA/DU/AA/UU）→ conflicts；
 *   - `??` → untracked；
 *   - 暂存区状态字母非空 → staged；工作区状态字母非空 → changes（例如 MM 同时进两组）；
 *   - `!!`（ignored）跳过。
 * -z 模式下重命名/复制是两条 NUL 记录：`R  <新路径>\0<旧路径>\0`。
 */
export function parsePorcelainV1Z(raw: unknown): PfGitGroups {
  const groups = createEmptyGroups();
  const text = String(raw ?? '');
  if (!text.trim() && !text.includes('\u0000')) return groups;

  const nulDelimited = text.includes('\u0000');
  const records = nulDelimited ? text.split('\u0000') : text.split('\n');

  for (let index = 0; index < records.length; index += 1) {
    let record = records[index];
    if (!record) continue;
    if (!nulDelimited) record = record.replace(/\r$/, '');
    if (record.length < 3 || record[2] !== ' ') continue;
    const indexStatus = record[0];
    const worktreeStatus = record[1];
    if (indexStatus === '!' || worktreeStatus === '!') continue;

    let path = record.slice(3);
    let origPath: string | undefined;
    if (indexStatus === 'R' || indexStatus === 'C') {
      if (nulDelimited) {
        const next = records[index + 1];
        if (next) {
          origPath = next;
          index += 1;
        }
      } else {
        const arrow = path.lastIndexOf(' -> ');
        if (arrow >= 0) {
          origPath = unquotePorcelainPath(path.slice(0, arrow));
          path = unquotePorcelainPath(path.slice(arrow + 4));
        } else {
          path = unquotePorcelainPath(path);
        }
      }
    } else if (!nulDelimited) {
      path = unquotePorcelainPath(path);
    }

    if (CONFLICT_PAIRS.has(`${indexStatus}${worktreeStatus}`)) {
      groups.conflicts.push(makeEntry('conflicts', '!', path, origPath));
      continue;
    }
    if (indexStatus === '?' && worktreeStatus === '?') {
      groups.untracked.push(makeEntry('untracked', '?', path, origPath));
      continue;
    }
    if (indexStatus !== ' ' && indexStatus !== '?') {
      groups.staged.push(makeEntry('staged', indexStatus, path, origPath));
    }
    if (worktreeStatus !== ' ' && worktreeStatus !== '?') {
      groups.changes.push(makeEntry('changes', worktreeStatus, path, origPath));
    }
  }
  return groups;
}

/** 用工作区前缀裁剪展示路径（子目录工作区时把 `sub/xxx` 展示为 `xxx`）。 */
export function applyDisplayPrefix(groups: PfGitGroups, prefix: string): PfGitGroups {
  const cleanPrefix = String(prefix ?? '').replace(/^\/+|\/+$/g, '');
  if (!cleanPrefix) return groups;
  const lowerPrefix = `${cleanPrefix}/`.toLowerCase();
  const strip = (value: string): string => {
    const normalized = String(value ?? '').replace(/\\/g, '/');
    if (normalized.toLowerCase().startsWith(lowerPrefix)) return normalized.slice(lowerPrefix.length);
    return normalized;
  };
  for (const group of PF_GIT_GROUP_ORDER) {
    for (const entry of groups[group]) {
      entry.displayPath = strip(entry.path);
      entry.origDisplayPath = entry.origPath ? strip(entry.origPath) : undefined;
    }
  }
  return groups;
}

/** 空分支信息。 */
export function createEmptyBranch(): PfGitBranchInfo {
  return { head: '', upstream: null, detached: false, initial: false, ahead: 0, behind: 0 };
}

/** 非仓库快照。 */
export function createNotRepoSnapshot(message: string): PfGitRepoSnapshot {
  return {
    isRepo: false,
    topLevel: '',
    prefix: '',
    groups: createEmptyGroups(),
    branch: createEmptyBranch(),
    log: [],
    total: 0,
    message,
  };
}

/**
 * 解析只读快照命令输出（buildSnapshotCommand 的正文部分）。
 * @param text parseGitCommandOutput(...).text
 * @param workspaceShellPath 当前工作区 POSIX 路径（用于裁剪子目录前缀）
 */
export function parseRepoSnapshot(text: string, workspaceShellPath = ''): PfGitRepoSnapshot {
  const sections = splitSnapshotSections(text);
  const topSection = sections.TOPLEVEL ?? '';
  if (sectionHasError(topSection) || !topSection.trim()) {
    const message = firstLineOf(topSection) || '当前工作区不是 Git 仓库';
    return createNotRepoSnapshot(message);
  }
  const topLevel = firstCleanLine(topSection) || workspaceShellPath;
  const prefix = relativePrefix(topLevel, workspaceShellPath || topLevel);

  const branchRaw = firstCleanLine(sections.BRANCH);
  const symref = firstCleanLine(sections.SYMREF);
  let head = '';
  let detached = false;
  if (symref) {
    head = symref;
  } else if (branchRaw && branchRaw !== 'HEAD') {
    head = branchRaw;
  } else if (branchRaw === 'HEAD') {
    detached = true;
  }

  const upstreamRaw = firstCleanLine(sections.UPSTREAM);
  const upstream = upstreamRaw && !sectionHasError(sections.UPSTREAM) ? upstreamRaw : null;

  let ahead = 0;
  let behind = 0;
  const aheadBehind = /(\d+)\s+(\d+)/.exec(String(sections.AB ?? ''));
  if (aheadBehind) {
    behind = Number(aheadBehind[1]);
    ahead = Number(aheadBehind[2]);
  }

  const groups = applyDisplayPrefix(parsePorcelainV1Z(sections.STATUS ?? ''), prefix);
  const log: PfGitLogEntry[] = [];
  for (const line of String(sections.LOG ?? '').split('\n')) {
    const match = /^([0-9a-f]{7,40})\s+(.*)$/.exec(line.trim());
    if (match) log.push({ hash: match[1], subject: match[2] });
  }
  const initial =
    !log.length && /does not have any commits yet|no commits yet/i.test(String(sections.LOG ?? ''));

  const total = groups.conflicts.length + groups.staged.length + groups.changes.length + groups.untracked.length;
  return {
    isRepo: true,
    topLevel,
    prefix,
    groups,
    branch: { head, upstream, detached, initial, ahead, behind },
    log,
    total,
    remotes: sections.REMOTE ?? '',
  };
}

// ============================ 运行时（组合式函数） ============================

interface PfGitSelection {
  conflicts: Set<string>;
  staged: Set<string>;
  changes: Set<string>;
  untracked: Set<string>;
}

function readSavedPath(): string {
  try {
    const raw = window.localStorage.getItem(PF_GIT_STORAGE_KEY);
    if (!raw) return '';
    try {
      const parsed = JSON.parse(raw);
      if (parsed && typeof parsed === 'object' && typeof (parsed as { path?: unknown }).path === 'string') {
        return String((parsed as { path: string }).path);
      }
    } catch {
      // 旧格式：直接存字符串
    }
    return raw;
  } catch {
    return '';
  }
}

function writeSavedPath(path: string): void {
  try {
    if (!path) window.localStorage.removeItem(PF_GIT_STORAGE_KEY);
    else window.localStorage.setItem(PF_GIT_STORAGE_KEY, JSON.stringify({ path, savedAt: Date.now() }));
  } catch {
    // localStorage 不可用时忽略（仅影响记忆上次工作区）
  }
}

/**
 * 创建 Git 模式运行时。返回响应式状态与操作，供 `PfGitPanel.vue` 使用；
 * 单测可直接传入假的 `exec` / `listWorkspaces` / `confirm`。
 */
export function createProjectForgeGit(deps: PfGitRuntimeDeps) {
  if (!deps || typeof deps.exec !== 'function') {
    throw new Error('createProjectForgeGit 需要注入 exec 执行器');
  }
  const confirmAction: PfGitConfirm = deps.confirm ?? (async () => false);

  // —— 工作区 ——
  const workspaces = ref<PfGitWorkspace[]>([]);
  const workspaceScanning = ref(false);
  const workspacesError = ref('');
  const pathValidating = ref(false);
  const currentShellPath = ref('');

  // —— 仓库状态 ——
  const status = ref<PfGitRepoSnapshot | null>(null);
  const statusLoading = ref(false);
  const statusError = ref('');
  const lastLoadedAt = ref(0);

  // —— 提示与操作输出 ——
  const notice = ref<PfGitNotice | null>(null);
  const opOutput = ref<PfGitOpOutput | null>(null);
  const busy = ref(false);
  const busyOp = ref<'commit' | 'push' | null>(null);

  // —— 提交与差异 ——
  const commitMessage = ref('');

  // —— 分支 + PR（2026-11-07 新增） ——
  /** 是否把本次提交放到新分支（默认关闭，开启时默认生成 vcp/时间戳）。 */
  const newBranchMode = ref(false);
  const newBranchName = ref('');
  /** 推送成功后是否自动创建 PR（GitHub 远端 + gh 可用时）。 */
  const autoCreatePr = ref(true);
  const prBusy = ref(false);
  const prResult = ref<PfGitPrResult | null>(null);
  const ghStatus = ref<'unknown' | 'available' | 'missing'>('unknown');
  const ghAccount = ref('');
  const activeDiff = ref<PfGitActiveDiff | null>(null);
  const diffLoading = ref(false);
  const diffError = ref('');
  const diffText = ref('');
  const collapsedGroups = ref<Set<PfGitGroup>>(new Set());

  const selection = reactive<PfGitSelection>({
    conflicts: new Set<string>(),
    staged: new Set<string>(),
    changes: new Set<string>(),
    untracked: new Set<string>(),
  });

  let statusSeq = 0;
  let diffSeq = 0;

  const currentWorkspace = computed(
    () => workspaces.value.find((item) => item.shellPath === currentShellPath.value) || null,
  );
  const currentAlias = computed(() => currentWorkspace.value?.alias || '');
  const currentDisplayPath = computed(() => currentWorkspace.value?.path || '');
  const isRepo = computed(() => Boolean(status.value?.isRepo));
  const effectiveRepoPath = computed(() =>
    status.value?.isRepo && status.value.topLevel ? status.value.topLevel : currentShellPath.value,
  );
  const repoGroups = computed<PfGitGroups>(() => status.value?.groups ?? createEmptyGroups());
  const repoLog = computed<PfGitLogEntry[]>(() => status.value?.log ?? []);
  const totalChanges = computed(() => status.value?.total ?? 0);

  const selectedEntries = computed<PfGitFileEntry[]>(() => {
    const result: PfGitFileEntry[] = [];
    const seen = new Set<string>();
    for (const group of PF_GIT_GROUP_ORDER) {
      for (const entry of status.value?.groups[group] ?? []) {
        if (!selection[group].has(entry.path)) continue;
        if (seen.has(entry.path)) continue;
        seen.add(entry.path);
        result.push(entry);
      }
    }
    return result;
  });
  const selectedPaths = computed<string[]>(() => selectedEntries.value.map((entry) => entry.path));
  const selectedCount = computed(() => selectedEntries.value.length);

  const canCommit = computed(
    () =>
      isRepo.value &&
      selectedCount.value > 0 &&
      commitMessage.value.trim().length > 0 &&
      !busy.value &&
      Boolean(effectiveRepoPath.value),
  );
  const canPush = computed(() => {
    const branch = status.value?.branch;
    return (
      isRepo.value &&
      Boolean(branch) &&
      !branch?.detached &&
      !branch?.initial &&
      !busy.value &&
      Boolean(effectiveRepoPath.value)
    );
  });
  const githubContext = computed<PfGithubContext | null>(() => detectGithubContext(status.value?.remotes));
  const canCreatePr = computed(
    () =>
      isRepo.value &&
      Boolean(githubContext.value) &&
      Boolean(status.value?.branch.head) &&
      !status.value?.branch.detached &&
      !busy.value &&
      !prBusy.value,
  );

  // —— 分支 + PR 操作（2026-11-07） ——
  function enableNewBranchMode(enabled: boolean): void {
    newBranchMode.value = Boolean(enabled);
    if (newBranchMode.value && !newBranchName.value.trim()) newBranchName.value = defaultNewBranchName();
    prResult.value = null;
  }

  function setNewBranchName(name: unknown): void {
    newBranchName.value = String(name ?? '').slice(0, 120);
  }

  function regenerateNewBranchName(): void {
    newBranchName.value = defaultNewBranchName();
  }

  function setAutoCreatePr(enabled: boolean): void {
    autoCreatePr.value = Boolean(enabled);
  }

  function resolveNewBranchName(): string {
    return newBranchName.value.trim() || defaultNewBranchName();
  }

  async function runGhCommand(repo: string, ghArgs: string, timeoutMs = 90_000): Promise<ExecOutcome> {
    let command: string;
    try {
      assertSafeGhArgs(ghArgs);
      command = buildGhCommand(repo, ghArgs);
    } catch (error) {
      return { text: errorMessageOf(error), exitCode: null, hasMarkers: false, ok: false };
    }
    let rawResult: PfGitExecResult;
    try {
      rawResult = await deps.exec(command, { timeoutMs });
    } catch (error) {
      return {
        text: `命令执行失败：${errorMessageOf(error)}`,
        exitCode: null,
        hasMarkers: false,
        ok: false,
      };
    }
    const parsed = parseGitCommandOutput(rawResult?.text ?? '');
    const ok = parsed.exitCode === null ? rawResult?.ok !== false : parsed.exitCode === 0;
    return { ...parsed, ok };
  }

  /** 检测 gh CLI 与登录状态（惰性，一次）。 */
  async function ensureGhAvailable(): Promise<boolean> {
    if (ghStatus.value === 'available') return true;
    if (ghStatus.value === 'missing') return false;
    const repo = effectiveRepoPath.value;
    if (!repo) return false;
    const version = await runGhCommand(repo, '--version', 15_000);
    if (!version.ok) {
      ghStatus.value = 'missing';
      return false;
    }
    const auth = await runGhCommand(repo, 'auth status', 20_000);
    if (!auth.ok) {
      ghStatus.value = 'missing';
      return false;
    }
    ghStatus.value = 'available';
    const account = /account\s+(\S+)/i.exec(auth.text);
    if (account) ghAccount.value = account[1];
    return true;
  }

  /** PR 的目标默认分支（fork 时为 upstream 的默认分支）。 */
  async function resolvePullRequestBase(context: PfGithubContext): Promise<string> {
    const repo = effectiveRepoPath.value;
    if (!repo) return 'main';
    const repoArg = `${context.baseRepo.owner}/${context.baseRepo.repo}`;
    const view = await runGhCommand(repo, `repo view ${repoArg} --json defaultBranchRef`, 25_000);
    const parsed = parseGhJsonOutput(view.text) as { defaultBranchRef?: { name?: unknown } } | null;
    const name = typeof parsed?.defaultBranchRef?.name === 'string' ? parsed.defaultBranchRef.name : '';
    return name || 'main';
  }

  function failPr(message: string): PfGitPrResult {
    const result: PfGitPrResult = { ok: false, message };
    prResult.value = result;
    setNotice('warning', message);
    return result;
  }

  /** 创建 PR：gh 可用时自动创建；否则降级为 GitHub compare 链接。 */
  async function openPullRequest(): Promise<PfGitPrResult> {
    const repo = effectiveRepoPath.value;
    const snapshot = status.value;
    const context = githubContext.value;
    if (!repo || !snapshot?.isRepo) return failPr('请先选择 Git 工作区');
    if (!context) return failPr('当前仓库不是 GitHub 远端，无法创建 PR');
    if (prBusy.value) return failPr('正在创建 PR，请稍候');
    const branch = snapshot.branch;
    if (!branch.head || branch.detached) return failPr('当前分支不可用（分离 HEAD），无法创建 PR');
    if (!branch.upstream) return failPr(`分支「${branch.head}」尚未推送，请先点「推送」再创建 PR`);
    prBusy.value = true;
    clearOpOutput();
    try {
      if (!(await ensureGhAvailable())) {
        const url = formatGithubCompareUrl(context, 'main', branch.head);
        prResult.value = { ok: false, url, message: `未检测到可用的 gh 登录，请在浏览器打开：${url}` };
        setNotice('warning', '未检测到 gh 登录，已生成 GitHub 对比链接');
        setOpOutput('PR 需手动创建', false, url, 'gh pr create（不可用）');
        return prResult.value;
      }
      const base = await resolvePullRequestBase(context);
      if (!context.isFork && branch.head === base) {
        return failPr(`当前在默认分支「${base}」上，无法向自身开 PR；请先勾选「新建修改分支」再提交推送`);
      }
      const headArg = context.isFork
        ? `--head ${context.origin.owner}:${branch.head}`
        : `--head ${branch.head}`;
      const ghArgs = `pr create --fill --base ${base} ${headArg} --repo ${context.baseRepo.owner}/${context.baseRepo.repo}`;
      const result = await runGhCommand(repo, ghArgs, 120_000);
      const url = extractPullRequestUrl(result.text);
      if (result.ok && url) {
        prResult.value = { ok: true, url, message: `PR 已创建：${url}` };
        setOpOutput('PR 已创建', true, result.text, `gh ${ghArgs}`);
        setNotice('success', 'PR 已创建');
        return prResult.value;
      }
      const fallbackUrl = formatGithubCompareUrl(context, base, branch.head);
      prResult.value = {
        ok: false,
        url: fallbackUrl,
        message: `PR 创建失败：${firstLineOf(result.text) || '未知错误'}（可在浏览器打开：${fallbackUrl}）`,
      };
      setOpOutput('PR 创建失败', false, `${result.text}\n${fallbackUrl}`, `gh ${ghArgs}`);
      setNotice('error', 'PR 创建失败');
      return prResult.value;
    } finally {
      prBusy.value = false;
    }
  }

  const hasWorkspaces = computed(() => workspaces.value.length > 0);

  // —— 提示 ——
  function setNotice(type: PfGitNotice['type'], text: string): void {
    notice.value = { type, text };
  }

  function clearNotice(): void {
    notice.value = null;
  }

  function clearOpOutput(): void {
    opOutput.value = null;
  }

  function setOpOutput(title: string, ok: boolean, text: string, command: string): void {
    opOutput.value = {
      title,
      ok,
      text: String(text ?? '').trim() || '（无输出）',
      command,
      finishedAt: Date.now(),
    };
  }

  // —— 执行 ——
  interface ExecOutcome extends PfGitCommandOutput {
    ok: boolean;
  }

  async function runCommand(
    command: string,
    options: { timeoutMs?: number; allowExitCodes?: number[] } = {},
  ): Promise<ExecOutcome> {
    try {
      assertSafeCommand(command);
    } catch (error) {
      return { text: errorMessageOf(error), exitCode: null, hasMarkers: false, ok: false };
    }
    let rawResult: PfGitExecResult;
    try {
      rawResult = await deps.exec(
        command,
        options.timeoutMs ? { timeoutMs: options.timeoutMs } : undefined,
      );
    } catch (error) {
      return {
        text: `命令执行失败：${errorMessageOf(error)}`,
        exitCode: null,
        hasMarkers: false,
        ok: false,
      };
    }
    const parsed = parseGitCommandOutput(rawResult?.text ?? '');
    const allowed =
      options.allowExitCodes && options.allowExitCodes.length ? options.allowExitCodes : [0];
    const ok = parsed.exitCode === null ? rawResult?.ok !== false : allowed.includes(parsed.exitCode);
    return { ...parsed, ok };
  }

  function clearSelection(): void {
    for (const group of PF_GIT_GROUP_ORDER) selection[group].clear();
  }

  function pruneSelection(snapshot: PfGitRepoSnapshot): void {
    for (const group of PF_GIT_GROUP_ORDER) {
      const valid = new Set((snapshot.groups[group] ?? []).map((entry) => entry.path));
      for (const path of [...selection[group]]) {
        if (!valid.has(path)) selection[group].delete(path);
      }
    }
  }

  function isSelected(group: PfGitGroup, path: string): boolean {
    return selection[group].has(path);
  }

  function toggleFile(group: PfGitGroup, path: string, next?: boolean): void {
    const set = selection[group];
    const target = next ?? !set.has(path);
    if (target) set.add(path);
    else set.delete(path);
  }

  function setGroupSelected(group: PfGitGroup, selected: boolean): void {
    const set = selection[group];
    const entries = status.value?.groups[group] ?? [];
    if (selected) entries.forEach((entry) => set.add(entry.path));
    else set.clear();
  }

  function groupSelection(group: PfGitGroup): { all: boolean; some: boolean; count: number } {
    const entries = status.value?.groups[group] ?? [];
    const set = selection[group];
    let count = 0;
    for (const entry of entries) if (set.has(entry.path)) count += 1;
    return { all: entries.length > 0 && count === entries.length, some: count > 0, count };
  }

  function toggleGroupCollapsed(group: PfGitGroup): void {
    const next = new Set(collapsedGroups.value);
    if (next.has(group)) next.delete(group);
    else next.add(group);
    collapsedGroups.value = next;
  }

  function isGroupCollapsed(group: PfGitGroup): boolean {
    return collapsedGroups.value.has(group);
  }

  // —— 工作区加载与选择 ——
  function selectWorkspace(shellPath: string): void {
    const workspace = workspaces.value.find((item) => item.shellPath === shellPath);
    if (!workspace) return;
    currentShellPath.value = workspace.shellPath;
    writeSavedPath(workspace.path);
    status.value = null;
    statusError.value = '';
    lastLoadedAt.value = 0;
    clearSelection();
    closeDiff();
    opOutput.value = null;
    notice.value = null;
    void refreshStatus();
  }

  async function loadWorkspaces(preferredPath = ''): Promise<void> {
    if (workspaceScanning.value) return;
    if (!deps.listWorkspaces) {
      workspacesError.value = '未配置工作区数据源，请在下方手动输入 Git 仓库路径。';
      return;
    }
    workspaceScanning.value = true;
    workspacesError.value = '';
    try {
      const rawList = await deps.listWorkspaces();
      const candidates: { id: string; alias: string; path: string; shellPath: string }[] = [];
      const seen = new Set<string>();
      (Array.isArray(rawList) ? rawList : []).forEach((item, index) => {
        const rawPath = String(item?.path ?? '').trim();
        if (!rawPath) return;
        const shellPath = toShellPath(rawPath);
        if (!shellPath || seen.has(shellPath)) return;
        seen.add(shellPath);
        candidates.push({
          id: String(item?.id ?? `workspace-${index}`),
          alias: String(item?.alias ?? '').trim() || lastSegmentOfPath(rawPath) || rawPath,
          path: rawPath,
          shellPath,
        });
      });

      let repoIndices: number[] = [];
      if (candidates.length) {
        const probe = await runCommand(
          buildWorkspaceProbeCommand(candidates.map((item) => item.shellPath)),
          { timeoutMs: PROBE_TIMEOUT_MS },
        );
        if (!probe.hasMarkers) {
          workspacesError.value = `检测 Git 工作区失败：${
            firstLineOf(probe.text) || 'PTYShellExecutor 无输出（桥接可能离线）'
          }`;
          return;
        }
        if (!probe.ok) {
          workspacesError.value = `检测 Git 工作区失败：${firstLineOf(probe.text) || '命令执行异常'}`;
          return;
        }
        repoIndices = parseWorkspaceProbe(probe.text);
      }

      const verified: PfGitWorkspace[] = repoIndices
        .map((index) => candidates[index])
        .filter((item): item is NonNullable<typeof item> => Boolean(item))
        .map((item) => ({ ...item, manual: false }));
      // 保留手动添加、且不在 ListWorkspaces 里的条目
      const manualOnly = workspaces.value.filter(
        (item) => item.manual && !verified.some((candidate) => candidate.shellPath === item.shellPath),
      );
      const nextList = [...verified, ...manualOnly];
      workspaces.value = nextList;
      if (verified.length < candidates.length) {
        setNotice(
          'info',
          `共 ${candidates.length} 个登记工作区，其中 ${verified.length} 个是 Git 仓库`,
        );
      }

      const preferredShell = toShellPath(preferredPath);
      const savedShell = toShellPath(readSavedPath());
      const desired =
        [preferredShell, currentShellPath.value, savedShell].find(
          (candidate) => candidate && nextList.some((item) => item.shellPath === candidate),
        ) || nextList[0]?.shellPath;
      if (desired) {
        if (desired === currentShellPath.value) {
          if (!status.value) void refreshStatus();
        } else {
          selectWorkspace(desired);
        }
      } else {
        currentShellPath.value = '';
        status.value = null;
      }
    } catch (error) {
      workspacesError.value = `加载工作区失败：${errorMessageOf(error)}`;
    } finally {
      workspaceScanning.value = false;
    }
  }

  async function ensureWorkspaces(
    options: { force?: boolean; preferredPath?: string } = {},
  ): Promise<void> {
    const preferred = String(options.preferredPath ?? '').trim();
    if (options.force || !workspaces.value.length) {
      await loadWorkspaces(preferred);
      return;
    }
    const preferredShell = toShellPath(preferred);
    if (preferredShell && workspaces.value.some((item) => item.shellPath === preferredShell)) {
      if (preferredShell !== currentShellPath.value) selectWorkspace(preferredShell);
      else if (!status.value) await refreshStatus();
      return;
    }
    if (!currentShellPath.value && workspaces.value.length) {
      selectWorkspace(workspaces.value[0].shellPath);
      return;
    }
    if (currentShellPath.value && !status.value && !statusError.value) await refreshStatus();
  }

  /** 手动输入路径兜底：校验是否为 Git 仓库后加入工作区列表并选中。 */
  async function addWorkspaceByPath(rawPath: string): Promise<{ ok: boolean; message: string }> {
    const raw = String(rawPath ?? '').trim();
    const shellPath = toShellPath(raw);
    if (!shellPath) return { ok: false, message: '请输入 Git 仓库路径，例如 D:\\Workspace\\Project' };
    const existing = workspaces.value.find(
      (item) => item.shellPath === shellPath || item.path.trim() === raw,
    );
    if (existing) {
      selectWorkspace(existing.shellPath);
      return { ok: true, message: `已切换到「${existing.alias}」` };
    }
    pathValidating.value = true;
    try {
      const probe = await runCommand(buildWorkspaceProbeCommand([shellPath]), {
        timeoutMs: PROBE_TIMEOUT_MS,
      });
      if (!probe.hasMarkers) {
        return {
          ok: false,
          message: `路径检测失败：${firstLineOf(probe.text) || 'PTYShellExecutor 无输出'}`,
        };
      }
      const indices = parseWorkspaceProbe(probe.text);
      if (!probe.ok || !indices.includes(0)) {
        return { ok: false, message: `不是 Git 仓库（或路径不存在）：${raw}` };
      }
      const workspace: PfGitWorkspace = {
        id: `manual-${shellPath}`,
        alias: lastSegmentOfPath(raw) || raw,
        path: raw,
        shellPath,
        manual: true,
      };
      workspaces.value = [workspace, ...workspaces.value];
      workspacesError.value = '';
      selectWorkspace(shellPath);
      return { ok: true, message: `已添加 Git 工作区「${workspace.alias}」` };
    } catch (error) {
      return { ok: false, message: `路径检测失败：${errorMessageOf(error)}` };
    } finally {
      pathValidating.value = false;
    }
  }

  // —— 仓库状态 ——
  async function refreshStatus(options: { quiet?: boolean } = {}): Promise<boolean> {
    const workspacePath = currentShellPath.value;
    if (!workspacePath) return false;
    if (statusLoading.value) return false;
    const seq = (statusSeq += 1);
    statusLoading.value = true;
    statusError.value = '';
    try {
      const repoPath = effectiveRepoPath.value || workspacePath;
      const result = await runCommand(buildSnapshotCommand(repoPath), { timeoutMs: READ_TIMEOUT_MS });
      if (seq !== statusSeq) return false;
      if (result.exitCode === 97) {
        statusError.value = `仓库路径不可访问：${
          currentDisplayPath.value || repoPath
        }（桌面端不存在该目录？）`;
        status.value = null;
        if (!options.quiet) setNotice('error', statusError.value);
        return false;
      }
      if (!result.hasMarkers) {
        statusError.value = `读取 Git 状态失败：${
          firstLineOf(result.text) || '命令执行器无输出（桥接可能离线）'
        }`;
        if (!options.quiet) setNotice('error', statusError.value);
        return false;
      }
      const snapshot = parseRepoSnapshot(result.text, workspacePath);
      status.value = snapshot;
      lastLoadedAt.value = Date.now();
      if (!snapshot.isRepo) {
        statusError.value = snapshot.message || '当前工作区不是 Git 仓库';
        if (!options.quiet) setNotice('warning', statusError.value);
      } else {
        statusError.value = '';
        pruneSelection(snapshot);
      }
      // 当前正在查看的文件若仍有变更则刷新 diff，否则关闭
      if (activeDiff.value) {
        const entry = findEntry(activeDiff.value.group, activeDiff.value.path);
        if (entry) void openDiff(activeDiff.value.group, activeDiff.value.path);
        else closeDiff();
      }
      return snapshot.isRepo;
    } catch (error) {
      if (seq !== statusSeq) return false;
      statusError.value = `读取 Git 状态失败：${errorMessageOf(error)}`;
      if (!options.quiet) setNotice('error', statusError.value);
      return false;
    } finally {
      if (seq === statusSeq) statusLoading.value = false;
    }
  }

  function findEntry(group: PfGitGroup, path: string): PfGitFileEntry | null {
    return (status.value?.groups[group] ?? []).find((entry) => entry.path === path) || null;
  }

  // —— 差异 ——
  function closeDiff(): void {
    diffSeq += 1;
    activeDiff.value = null;
    diffLoading.value = false;
    diffError.value = '';
    diffText.value = '';
  }

  async function openDiff(group: PfGitGroup, path: string): Promise<void> {
    const entry = findEntry(group, path);
    if (!entry) return;
    const repo = effectiveRepoPath.value;
    if (!repo) {
      diffError.value = '缺少 Git 仓库路径，无法读取差异';
      return;
    }
    activeDiff.value = {
      group,
      path: entry.path,
      displayPath: entry.displayPath,
      status: entry.status,
      label: entry.label,
    };
    // 未跟踪目录（porcelain 聚合显示为 `dir/`）无法做文件级 diff，直接交给空态提示
    if (group === 'untracked' && entry.path.endsWith('/')) {
      diffSeq += 1;
      diffLoading.value = false;
      diffError.value = '';
      diffText.value = '';
      return;
    }
    const seq = (diffSeq += 1);
    diffLoading.value = true;
    diffError.value = '';
    diffText.value = '';
    try {
      const result = await runCommand(buildDiffCommand(repo, group, entry.path), {
        timeoutMs: READ_TIMEOUT_MS,
        allowExitCodes: group === 'untracked' ? [0, 1] : [0],
      });
      if (seq !== diffSeq) return;
      if (!result.ok) {
        diffError.value = `读取差异失败：${firstLineOf(result.text) || '未知错误'}`;
        diffText.value = result.text;
        return;
      }
      diffText.value = result.text;
    } catch (error) {
      if (seq !== diffSeq) return;
      diffError.value = `读取差异失败：${errorMessageOf(error)}`;
    } finally {
      if (seq === diffSeq) diffLoading.value = false;
    }
  }

  // —— 写操作（均要求调用方先二次确认） ——

  /** 暂存勾选文件并提交（不做确认，确认由 confirmAndCommit 负责）。 */
  async function stageAndCommit(paths: readonly string[], message: string): Promise<boolean> {
    const repo = effectiveRepoPath.value;
    const unique = [...new Set((paths ?? []).map((item) => String(item ?? '').trim()).filter(Boolean))];
    const text = String(message ?? '').trim();
    if (!repo) {
      setNotice('error', '请先选择 Git 工作区');
      return false;
    }
    if (!unique.length) {
      setNotice('warning', '请先勾选要提交的文件');
      return false;
    }
    if (!text) {
      setNotice('warning', '请填写提交信息');
      return false;
    }
    if (busy.value) {
      setNotice('warning', '正在执行其它 Git 操作，请稍候');
      return false;
    }

    busy.value = true;
    busyOp.value = 'commit';
    clearOpOutput();
    try {
      // 可选：先把本次修改放到一个新分支（默认 vcp/时间戳），避免直接提交/推送默认分支。
      let createdBranch = '';
      if (newBranchMode.value) {
        const targetName = resolveNewBranchName();
        if (!isValidGitBranchName(targetName)) {
          setNotice('error', `分支名不合法：${targetName}（不能含空格 / .. / ~^:?*[\\ 等字符）`);
          return false;
        }
        const exists = await runCommand(
          buildGitCommand(repo, `rev-parse --verify --quiet refs/heads/${shellQuote(targetName)}`),
          { allowExitCodes: [0, 1], timeoutMs: READ_TIMEOUT_MS },
        );
        if (exists.exitCode === 0) {
          setNotice('error', `分支已存在：${targetName}，请换一个名字或点「生成」`);
          return false;
        }
        const switchResult = await runCommand(buildGitCommand(repo, `switch -c ${shellQuote(targetName)}`), {
          timeoutMs: COMMIT_TIMEOUT_MS,
        });
        if (!switchResult.ok) {
          setOpOutput('创建分支失败', false, switchResult.text, `git switch -c ${targetName}`);
          setNotice('error', `创建分支失败：${firstLineOf(switchResult.text) || '未知错误'}`);
          return false;
        }
        createdBranch = targetName;
      }

      const addArgs = `add -- ${unique.map((item) => shellQuote(item)).join(' ')}`;
      const addResult = await runCommand(buildGitCommand(repo, addArgs), {
        timeoutMs: COMMIT_TIMEOUT_MS,
      });
      if (!addResult.ok) {
        setOpOutput('暂存失败', false, addResult.text, `git add -- ${unique.join(' ')}`);
        setNotice('error', `暂存失败：${firstLineOf(addResult.text) || '未知错误'}`);
        return false;
      }

      const commitResult = await runCommand(buildGitCommand(repo, `commit -m ${shellQuote(text)}`), {
        timeoutMs: COMMIT_TIMEOUT_MS,
      });
      const outputText = `${addResult.text ? `${addResult.text}\n` : ''}${commitResult.text}`.trim();
      const commandText = `${createdBranch ? `git switch -c ${createdBranch} && ` : ''}git add -- ${unique.join(' ')} && git commit -m ${shellQuote(text)}`;
      setOpOutput(
        commitResult.ok ? (createdBranch ? `已提交到新分支 ${createdBranch}` : '提交完成') : '提交失败',
        commitResult.ok,
        outputText,
        commandText,
      );
      if (!commitResult.ok) {
        setNotice('error', `提交失败：${firstLineOf(commitResult.text) || '未知错误'}`);
        return false;
      }
      commitMessage.value = '';
      clearSelection();
      setNotice('success', createdBranch ? `已在新分支「${createdBranch}」提交成功` : '提交成功');
      await refreshStatus({ quiet: true });
      return true;
    } finally {
      busy.value = false;
      busyOp.value = null;
    }
  }

  /** 提交前二次确认：展示勾选文件清单与提交信息。 */
  async function confirmAndCommit(): Promise<boolean> {
    if (busy.value) return false;
    if (!isRepo.value) {
      setNotice('error', '请先选择 Git 工作区');
      return false;
    }
    const entries = selectedEntries.value;
    const message = commitMessage.value.trim();
    if (!entries.length) {
      setNotice('warning', '请先勾选要提交的文件');
      return false;
    }
    if (!message) {
      setNotice('warning', '请填写提交信息');
      return false;
    }
    if (message.length > 20000) {
      setNotice('warning', '提交信息过长（最多 20000 字符）');
      return false;
    }
    const preview = entries
      .slice(0, 12)
      .map((entry) => `• ${entry.displayPath}`)
      .join('\n');
    const more = entries.length > 12 ? `\n…以及另外 ${entries.length - 12} 个文件` : '';
    const confirmed = await confirmAction({
      title: '确认提交',
      message: `将暂存并提交 ${entries.length} 个文件到「${
        currentAlias.value || currentDisplayPath.value
      }」：\n${preview}${more}\n\n提交信息：\n${message}`,
      isDanger: false,
    });
    if (!confirmed) {
      setNotice('info', '已取消提交');
      return false;
    }
    return stageAndCommit(
      entries.map((entry) => entry.path),
      message,
    );
  }

  /** 推送（不做确认，确认由 confirmAndPush 负责）。无上游时通过 -u origin <branch> 建立上游。 */
  async function push(): Promise<boolean> {
    const repo = effectiveRepoPath.value;
    const snapshot = status.value;
    if (!repo || !snapshot?.isRepo) {
      setNotice('error', '请先选择 Git 工作区');
      return false;
    }
    if (busy.value) {
      setNotice('warning', '正在执行其它 Git 操作，请稍候');
      return false;
    }
    const branch = snapshot.branch;
    if (branch.detached) {
      setNotice('warning', '当前处于分离 HEAD 状态，无法推送，请先切回分支');
      return false;
    }
    if (branch.initial) {
      setNotice('warning', '当前分支尚无任何提交，无法推送');
      return false;
    }
    const hasUpstream = Boolean(branch.upstream);
    if (!hasUpstream && !branch.head) {
      setNotice('warning', '无法确定当前分支，请先在桌面端设置上游分支');
      return false;
    }

    busy.value = true;
    busyOp.value = 'push';
    clearOpOutput();
    try {
      const gitArgs = hasUpstream ? 'push' : `push --set-upstream origin ${shellQuote(branch.head)}`;
      const humanCommand = hasUpstream ? 'git push' : `git push --set-upstream origin ${branch.head}`;
      const result = await runCommand(buildGitCommand(repo, gitArgs), { timeoutMs: PUSH_TIMEOUT_MS });
      setOpOutput(result.ok ? '推送完成' : '推送失败', result.ok, result.text, humanCommand);
      if (!result.ok) {
        setNotice('error', `推送失败：${firstLineOf(result.text) || '未知错误'}`);
        return false;
      }
      setNotice('success', '推送成功');
      await refreshStatus({ quiet: true });
      // 推送成功后按需创建 PR（仅 GitHub 远端 + 已开启自动 PR）
      if (newBranchMode.value && autoCreatePr.value && githubContext.value) {
        const pr = await openPullRequest();
        setNotice(pr.ok ? 'success' : 'warning', `推送成功 · ${pr.message}`);
      }
      return true;
    } finally {
      busy.value = false;
      busyOp.value = null;
    }
  }

  /** 推送前二次确认（无上游时说明会设置 origin 为上游）。 */
  async function confirmAndPush(): Promise<boolean> {
    if (busy.value) return false;
    const snapshot = status.value;
    if (!snapshot?.isRepo) {
      setNotice('error', '请先选择 Git 工作区');
      return false;
    }
    const branch = snapshot.branch;
    if (branch.detached) {
      setNotice('warning', '当前处于分离 HEAD 状态，无法推送，请先切回分支');
      return false;
    }
    if (branch.initial) {
      setNotice('warning', '当前分支尚无任何提交，无法推送');
      return false;
    }
    const hasUpstream = Boolean(branch.upstream);
    const confirmed = await confirmAction({
      title: hasUpstream ? '确认推送' : '推送到新上游',
      message: hasUpstream
        ? `将把「${branch.head}」推送到 ${branch.upstream}。\n当前领先远程 ${branch.ahead} 个提交、落后 ${branch.behind} 个提交。`
        : `分支「${branch.head}」尚未关联上游分支。\n将执行 git push --set-upstream origin ${branch.head}，把 origin 设为上游并推送。`,
      isDanger: false,
    });
    if (!confirmed) {
      setNotice('info', '已取消推送');
      return false;
    }
    return push();
  }

  return {
    // state
    workspaces,
    workspaceScanning,
    workspacesError,
    pathValidating,
    currentShellPath,
    currentWorkspace,
    currentAlias,
    currentDisplayPath,
    status,
    statusLoading,
    statusError,
    lastLoadedAt,
    notice,
    opOutput,
    busy,
    busyOp,
    commitMessage,
    activeDiff,
    diffLoading,
    diffError,
    diffText,
    collapsedGroups,
    hasWorkspaces,
    // computed
    isRepo,
    effectiveRepoPath,
    groups: repoGroups,
    log: repoLog,
    totalChanges,
    selectedEntries,
    selectedPaths,
    selectedCount,
    canCommit,
    canPush,
    // workspace actions
    loadWorkspaces,
    ensureWorkspaces,
    addWorkspaceByPath,
    selectWorkspace,
    // status actions
    refreshStatus,
    // selection actions
    isSelected,
    toggleFile,
    setGroupSelected,
    groupSelection,
    clearSelection,
    toggleGroupCollapsed,
    isGroupCollapsed,
    // diff actions
    openDiff,
    closeDiff,
    // notice / output
    setNotice,
    clearNotice,
    clearOpOutput,
    // branch + PR（2026-11-07）
    newBranchMode,
    newBranchName,
    autoCreatePr,
    prBusy,
    prResult,
    ghStatus,
    ghAccount,
    githubContext,
    canCreatePr,
    enableNewBranchMode,
    setNewBranchName,
    regenerateNewBranchName,
    setAutoCreatePr,
    openPullRequest,
    // write actions
    confirmAndCommit,
    stageAndCommit,
    confirmAndPush,
    push,
  };
}

/** Git 运行时类型。 */
export type ProjectForgeGit = ReturnType<typeof createProjectForgeGit>;
