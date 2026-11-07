/**
 * V工程（桌面 VCPChat ProjectForge 工程施工台）手机端数据契约。
 *
 * 数据来源：MobilePluginBridge 的 `POST {base}/invoke`。
 * 成功返回：`{ ok:true, result:{ content:[{type:'text',text}], details:{...} } }`；
 * 失败返回：HTTP 4xx/5xx + `{ ok:false, error:{ code, message } }`。
 *
 * 契约要点（2026-11，依据桌面插件 ProjectForgeService.js 实测）：
 * - `ListWorkspaces.details = { command, workspaces:[{id,alias,path,enabled,status}] }`
 * - `ListProjects.details = { command, count, limit, projects:[PfProject] }`；
 *   工程列表的 todo 进度只出现在 content[0].text 的 Markdown 表格里（store 解析兜底）。
 * - `GetProject.details = { command, project, todos, stats }`；**不含 timeline/files/contributors**，
 *   详情页的这三块由 store 用 `SearchHistory(projectId, limit≈300)` 聚合。
 * - `SearchHistory.details = { command, count, nodes:[PfHistoryRow] }`（每个文件一条节点）。
 * - `ReadCode` 正文在 `content[1..].text`（带 `N | ` 行号），details 只有 `read/skipped/failed/symbols`。
 * - `GetNodeDiff` 只有 content 文本 + `details={count}`。
 * - `Rollback/EditCode/CreateFile/...` 的 details 带 `status`（ok/ambiguous/error/unchanged/conflict/noop/...）。
 */

// ============================ 基础枚举 ============================

/** 错误分级：网络不可达 / 鉴权失败 / 桌面端命令失败 / 协议不符 / 本地参数校验失败。 */
export type PfErrorKind = 'network' | 'auth' | 'bridge' | 'protocol' | 'validation';

/** 待办状态：pending / doing / done / blocked（服务端可能扩展）。 */
export type PfTodoStatus = 'pending' | 'doing' | 'done' | 'blocked' | (string & {});

/** 工程状态：active（施工中）/ review（待验收）/ accepted（已验收）。 */
export type PfProjectStatus = 'active' | 'review' | 'accepted' | (string & {});

/** 节点操作类型：create / edit / remove / move / rollback / external。 */
export type PfOpKind = 'create' | 'edit' | 'remove' | 'move' | 'rollback' | 'external' | (string & {});

/** 验收报告状态：review（待验收）/ accepted（已验收）/ active（退回继续施工）。 */
export type PfReportStatus = 'review' | 'accepted' | 'active' | (string & {});

/** 桥接连接状态。 */
export type PfConnectionStatus = 'unknown' | 'connecting' | 'ok' | 'fail';

// ============================ 工作区 ============================

export interface PfWorkspace {
  id: string;
  alias: string;
  path: string;
  /** 停用工作区只读；工程只能建在已启用工作区内。 */
  enabled: boolean;
  /** 桌面端额外状态（如 ready / disabled）。 */
  status?: string;
  /** 是否为桌面当前激活工作区（ListWorkspaces 文本里有，结构化数据可能没有）。 */
  current?: boolean;
}

// ============================ 工程 ============================

export interface PfRootInfo {
  root?: string;
  workspaceAlias?: string;
  /** false = 只读（工作区停用/移除，或目录不在白名单内），禁止回退与写入。 */
  writable?: boolean;
  blockedReason?: string;
}

export interface PfProgress {
  done: number;
  total: number;
  /** 形如 `3/8`；没有 todo 时桌面端给 `无`。 */
  text: string;
}

/** 工程改动统计（GetProject.details.stats）。 */
export interface PfStats {
  nodeCount: number;
  fileCount: number;
  added: number;
  removed: number;
  lastNodeId?: number | null;
  lastAt?: string | null;
}

export interface PfProject {
  id: string;
  name: string;
  workspace_id?: string;
  workspace_alias?: string;
  /** 根目录绝对路径（桌面端 POSIX 形式，如 `/home/user/...`）。 */
  root: string;
  subpath?: string;
  status: PfProjectStatus;
  /** 验收报告 Markdown（如有）。 */
  report?: string | null;
  created_by?: string | null;
  report_by?: string | null;
  created_at?: string;
  updated_at?: string;
  deleted_at?: string | null;
  deleted_by?: string | null;
  /** 工程根目录与可写状态（详情页 GetProject 可能带，列表页可能没有）。 */
  rootInfo?: PfRootInfo;
  /** 列表接口/详情接口可能带的 todo 进度。 */
  progress?: PfProgress;
  /** 详情/列表可能带的改动统计。 */
  stats?: PfStats;
  /** 是否已提交验收报告。 */
  hasReport?: boolean;
  /** 手机端派生字段：todo 进度（列表 Markdown 表格解析或 progress 字段）。 */
  todoDone?: number;
  todoTotal?: number;
  /** 参与过施工的署名列表（桌面 GUI 门面会带）。 */
  maids?: string[];
}

export interface PfTodo {
  id?: number;
  project_id?: string;
  /** 序号，从 1 开始；UpdateTodos 的 done/doing/pending/blocked/remove 都按序号操作。 */
  seq: number;
  title: string;
  status: PfTodoStatus;
  note?: string | null;
  updated_by?: string | null;
  updated_at?: string | null;
}

// ============================ 历史 / 聚合 ============================

/** SearchHistory 的一条记录（每个文件每批次一条节点）。 */
export interface PfHistoryRow {
  id: number;
  batch_id: number;
  batch_kind?: string;
  op: PfOpKind;
  project_id?: string;
  file_path: string;
  maid?: string | null;
  /** 桌面端合并后的原因（effective_reason 优先，其次 reason / summary）。 */
  effective_reason?: string;
  reason?: string;
  summary?: string;
  added: number;
  removed: number;
  created_at: string;
  todo_id?: number | null;
  /** 仅 content 过滤时服务端内部使用，正常不返回。 */
  after_hash?: string | null;
}

/**
 * 开发脉络的一个批次（由 PfHistoryRow 按 batch_id 聚合）。
 * 契约字段来自 spec §5；`batchId/batchKey/at` 是给旧版页面/组件保留的兼容别名。
 */
export interface PfBatchSummary {
  /** 批次主键（形状 `b{id}` 中的数字部分，与数据库 batch.id 一致）。 */
  id: number;
  kind: PfOpKind;
  maid?: string | null;
  reason: string;
  created_at: string;
  node_count: number;
  added: number;
  removed: number;
  /** 涉及文件路径（去重，保持首次出现顺序）。 */
  files: string[];
  /** @deprecated 兼容旧版 `PfTimelineItem`：等于 id。 */
  batchId?: number;
  /** @deprecated 兼容旧版 `PfTimelineItem`：形如 `b21`。 */
  batchKey?: string;
  /** @deprecated 兼容旧版 `PfTimelineItem`：等于 created_at。 */
  at?: string;
}

/** 改动文件聚合（按 file_path 聚合 PfHistoryRow）。 */
export interface PfFileChange {
  file_path: string;
  /** 改动节点数（= 编辑次数）。 */
  edits: number;
  added: number;
  removed: number;
  /** 最近一次改动的节点 id（`n{last_node}`）。 */
  last_node: number;
}

/** 施工角色聚合（按 maid 聚合 PfHistoryRow；maid 为 null 表示外部/未署名修改）。 */
export interface PfContributor {
  maid: string | null;
  /** 该署名参与的批次数。 */
  batches: number;
  added: number;
  removed: number;
  /** 最近活动时间 ISO 字符串。 */
  last_at: string;
}

/** 工程详情：GetProject(project/todos/stats) + SearchHistory 聚合出 timeline/files/contributors。 */
export interface PfProjectDetail {
  project: PfProject;
  todos: PfTodo[];
  stats: PfStats;
  timeline: PfBatchSummary[];
  files: PfFileChange[];
  contributors: PfContributor[];
  /** 本地加载完成时间戳（毫秒）。 */
  loadedAt: number;
}

/** diff 结果（GetNodeDiff / 旧 getBatchDiff 统一返回）。 */
export interface PfDiffResult {
  title: string;
  /** Markdown 文本，通常含 ```diff 围栏。 */
  text: string;
}

/** SearchHistory 过滤条件（均可选；未传 projectId 时搜索全部工程）。 */
export interface PfHistoryFilters {
  projectId?: string;
  /** 匹配 reason、摘要与路径。 */
  keyword?: string;
  /** 匹配改动后的文件内容（服务端逐节点读快照，较慢）。 */
  content?: string;
  /** 路径通配，如 `src/*.js`。 */
  file?: string;
  /** 按操作者署名过滤。 */
  byMaid?: string;
  op?: PfOpKind;
  /** 关联 todo 序号（需 projectId）。 */
  todo?: number | string;
  /** 返回条数上限（服务端默认 30）。 */
  limit?: number;
  /** 按批次过滤：`b21` 或数字 21（历史页展开批次节点用）。 */
  batch?: number | string;
  since?: string;
  until?: string;
}

// ============================ ReadCode ============================

export interface PfReadFileInfo {
  path?: string;
  kind?: string;
  size?: number;
  lines?: number;
  truncated?: boolean;
  [key: string]: unknown;
}

export interface PfSymbolRef {
  path?: string;
  name?: string;
  qualified?: string;
  kind?: string;
  start?: number;
  end?: number;
  fullStart?: number;
  signature?: string;
  lang?: string;
  readStart?: number;
  readEnd?: number;
  [key: string]: unknown;
}

export interface PfReadCodeOptions {
  /** 行范围：head:N / tail:N / M-N / N。 */
  lines?: string;
  /** 多文件（换行或逗号分隔，每项可带 :M-N）。 */
  paths?: string[];
  /** 通配批量读取。 */
  glob?: string;
  /** 按符号读取（path 也可写成 `文件#符号`）。 */
  symbol?: string;
  showLine?: boolean;
  find?: string;
  range?: 'full' | 'body' | (string & {});
  context?: number;
  pick?: number | string;
  line?: number;
  /** 覆盖默认桥接超时（毫秒）。 */
  timeoutMs?: number;
}

export interface PfReadCodeResult extends PfDiffResult {
  /** 成功读取的文件信息（details.read）。 */
  files: PfReadFileInfo[];
  skipped: unknown[];
  failed: unknown[];
  symbols: PfSymbolRef[];
  /** read（普通读取）/ find（文件内查找）。 */
  mode?: string;
}

// ============================ 写操作结果 ============================

/** EditCode / ResolveEdit / CreateFile / RemoveFile / MoveFile / MoveCode / CopyCode 的统一结果。 */
export interface PfEditResult {
  command?: string;
  /** ok / ambiguous / error / unchanged / rejected。 */
  status?: string;
  nodeId?: number;
  batchId?: number;
  ticketId?: string;
  applied?: unknown[];
  skipped?: unknown[];
  errors?: unknown[];
  review?: unknown;
  /** content 的 Markdown 摘要（含 diff），便于直接展示。 */
  text?: string;
  [key: string]: unknown;
}

/** EditCode 参数（大多数字段原样透传给桌面命令，行号/符号语义见命令契约）。 */
export interface PfEditCodeArgs {
  projectId?: string;
  path: string;
  /** 必填：本次变动原因。 */
  reason: string;
  symbol?: string;
  start?: number;
  end?: number;
  lines?: string;
  after?: number | string;
  before?: number | string;
  content?: string;
  target?: string;
  replace?: string;
  expect?: string;
  op?: string;
  mode?: 'atomic' | 'bestEffort' | (string & {});
  todo?: number | string;
  revertOnSyntaxError?: boolean;
  [key: string]: unknown;
}

export interface PfResolveEditArgs {
  projectId?: string;
  /** EditCode 返回的歧义票据。 */
  ticketId: string;
  /** 候选序号：2 / 1,3 / all / 2:1;5:3。 */
  pick: string | number;
}

export interface PfCreateFileArgs {
  projectId?: string;
  path: string;
  content: string;
  reason: string;
  overwrite?: boolean;
  todo?: number | string;
  revertOnSyntaxError?: boolean;
}

export interface PfRemoveFileArgs {
  projectId?: string;
  path?: string;
  paths?: string[];
  reason: string;
  todo?: number | string;
}

export interface PfMoveFileArgs {
  projectId?: string;
  from: string;
  to: string;
  reason: string;
  todo?: number | string;
}

/** MoveCode / CopyCode 参数（from 可带 `:M-N` 或 `#符号`）。 */
export interface PfTransferCodeArgs {
  projectId?: string;
  from: string;
  to?: string;
  after?: number | string;
  before?: number | string;
  lines?: string;
  start?: number;
  end?: number;
  target?: string;
  symbol?: string;
  indent?: string | number;
  reason: string;
  range?: 'full' | 'body' | (string & {});
  todo?: number | string;
  revertOnSyntaxError?: boolean;
  [key: string]: unknown;
}

/** Rollback 参数：batch / toNode 二选一；两者都接受数字或 `bX`/`nX` 字符串。 */
export interface PfRollbackArgs {
  projectId?: string;
  /** 批次：`b21` / 21 / `last`（撤销最近一次）。 */
  batch?: number | string;
  /** 恢复到这个节点时的状态：`n21` / 21。 */
  toNode?: number | string;
  /** toNode 模式下只恢复单个文件。 */
  path?: string;
  /** 冲突时强制覆盖（当前磁盘内容会先存快照）。 */
  force?: boolean;
  /** 只预演不执行。 */
  dryRun?: boolean;
  reason?: string;
}

export interface PfRollbackItem {
  rel: string;
  target?: string | null;
  expected?: string | null;
  later?: number;
  /** 移到回收站 / 恢复内容 / 重建文件 / 无需操作。 */
  action?: string;
  conflicts: string[];
}

export interface PfRollbackResult {
  /** ok / noop / conflict / dryRun。 */
  status: 'ok' | 'noop' | 'conflict' | 'dryRun' | (string & {});
  batchId?: number;
  count?: number;
  items?: PfRollbackItem[];
  /** 执行回退的署名。 */
  maid?: string;
  /** content Markdown 摘要（含逐文件动作表）。 */
  text?: string;
}

export interface PfDeleteResult {
  deleted: string[];
  skipped: string[];
}

export interface PfRestoreResult {
  restored: string[];
}

export interface PfPurgeResult {
  purged: string[];
  freedBlobs: number;
}

/** 验收报告提交参数。 */
export interface PfSubmitReportInput {
  /** 验收结论（必填）。 */
  conclusion: string;
  /** 遗留问题（可选）。 */
  issues?: string;
  /** review（默认，待验收）/ accepted（已验收）/ active（退回继续施工）。 */
  status?: PfReportStatus;
}

// ============================ 源码（Outline / FindSymbol / Trace） ============================

export interface PfOutlineSymbol {
  name?: string;
  qualified?: string;
  kind?: string;
  /** 签名行 / 结束行 / 含前置注释的起点。 */
  start?: number;
  end?: number;
  fullStart?: number;
  depth?: number;
  signature?: string;
  [key: string]: unknown;
}

export interface PfOutlineFile {
  path: string;
  lang?: string;
  hasError?: boolean;
  lineCount?: number;
  symbols?: PfOutlineSymbol[];
  /** 二进制文件大纲（节区/导出符号）。 */
  isBinary?: boolean;
  format?: string;
  architecture?: string;
  sections?: unknown[];
  [key: string]: unknown;
}

export interface PfOutlineOptions {
  depth?: number;
  kind?: string;
}

export interface PfOutlineResult {
  files: PfOutlineFile[];
  text?: string;
}

export interface PfFindSymbolHit {
  path: string;
  inProject: boolean;
  name: string;
  qualified?: string;
  kind?: string;
  fullStart?: number;
  start?: number;
  end?: number;
  signature?: string;
  lang?: string;
  [key: string]: unknown;
}

export interface PfFindSymbolResult {
  total: number;
  scanned?: number;
  parsed?: number;
  truncated?: boolean;
  hits: PfFindSymbolHit[];
  text?: string;
}

export interface PfFindSymbolOptions {
  kind?: string;
  glob?: string;
  exact?: boolean;
  limit?: number;
  scope?: 'project' | 'workspace' | (string & {});
}

export interface PfTraceOptions {
  scope?: 'project' | 'workspace' | (string & {});
  glob?: string;
}

export interface PfTraceResult {
  kind?: string;
  found?: boolean;
  text?: string;
  [key: string]: unknown;
}

// ============================ 源码文件树 / 编辑器（T3 使用） ============================

/** 文件树条目（FileOperator.ListDirectory / 本地归一化）。 */
export interface PfFileEntry {
  name: string;
  /** 工作区/工程内相对路径（POSIX）。 */
  path: string;
  type: 'file' | 'dir' | (string & {});
  size?: number;
  mtime?: number | string;
  children?: PfFileEntry[];
  [key: string]: unknown;
}

export interface PfSearchFileHit {
  path: string;
  name?: string;
  matches?: number;
  [key: string]: unknown;
}

// ============================ Git（T4 使用；数据来自 PTYShellExecutor） ============================

export interface PfGitFileStatus {
  path: string;
  /** 暂存区状态字母（M/A/D/R/U…）。 */
  index?: string;
  /** 工作区状态字母。 */
  worktree?: string;
  staged?: boolean;
  unstaged?: boolean;
  untracked?: boolean;
  deleted?: boolean;
  conflict?: boolean;
  [key: string]: unknown;
}

export interface PfGitStatus {
  branch?: string;
  ahead?: number;
  behind?: number;
  staged: PfGitFileStatus[];
  unstaged: PfGitFileStatus[];
  untracked: PfGitFileStatus[];
  conflicted: PfGitFileStatus[];
  /** 原始 porcelain 文本，便于排查。 */
  raw?: string;
}

export interface PfGitDiffResult extends PfDiffResult {
  /** 原始 diff 文本（无 Markdown 围栏）。 */
  raw?: string;
}

export interface PfGitCommitResult {
  hash?: string;
  summary?: string;
  text?: string;
}

export interface PfGitWorkspace {
  id?: string;
  alias: string;
  path: string;
  enabled?: boolean;
}

// ============================ 设置 / 连接 ============================

export interface PfBridgeSettings {
  baseUrl: string;
  token: string;
}

export type PfBridgeConfigSource = 'saved' | 'legacy' | 'sync' | 'default';

export interface PfConnectionState {
  ok: boolean;
  message: string;
  status: PfConnectionStatus;
}

export interface PfLoadingState {
  workspaces: boolean;
  projects: boolean;
  detail: boolean;
  history: boolean;
  action: boolean;
}

/** 工程内节点/批次引用的选择器（node 与 batch 二选一）。 */
export interface PfDiffSelector {
  node?: number | string;
  batch?: number | string;
  maxLines?: number;
}
