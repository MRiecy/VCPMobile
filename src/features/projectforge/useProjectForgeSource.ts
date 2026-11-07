/**
 * V工程「源码」模式数据层（对齐桌面 VCPChat / ProjectForge 源码侧栏）。
 *
 * 通道说明（手机端没有桌面 Electron IPC，全部走 MobilePluginBridge）：
 * - 工作区 / 工程：ProjectForge `ListWorkspaces` + `ListProjects` + `GetProject`（取工程根目录）；
 * - 文件树 / 文件名搜索：桌面插件 `FileOperator`（`ListDirectory` / `SearchFiles`），经 `pluginInvoke` 调用；
 *   若 FileOperator 不可用（未启用 / 返回结构无法识别 / 报错），自动降级为 `PTYShellExecutor`：
 *   目录用 `find -maxdepth 1`，搜索用 `find -type f -iname`；
 * - 读文件：ProjectForge `ReadCode`（正文在 content[1..]，这里显式用 `showLine:false` 取精确全文，
 *   行号由手机端自己渲染；同时保留 content 解析兜底）；
 * - 写文件：`EditCode`（整文件替换：target=原文 / replace=新全文；失败时退化为 start/end 行区间替换）、
 *   `CreateFile`、`RemoveFile`、`MoveFile`；全部要求 reason，并在写入后用 `ReadCode` 回读校验；
 * - 符号：`Outline` / `FindSymbol` / `Trace`。
 *
 * 本模块是模块级单例（一个页面只有一份源码面板），组件通过 `useProjectForgeSource()` 读取同一份状态；
 * 单测用 `__resetProjectForgeSourceForTests()` 重置。
 */
import { computed, reactive, ref } from 'vue';
import {
  bridgeContentParts,
  bridgeContentText,
  pfInvoke,
  pfInvokeEnvelope,
  pluginInvoke,
  ptyExec,
} from './projectForgeApi';
import type { PfBridgeContentPart, PfBridgeEnvelope } from './projectForgeApi';
import type {
  PfFileEntry,
  PfFindSymbolResult,
  PfOutlineResult,
  PfOutlineSymbol,
  PfProject,
  PfReadCodeResult,
  PfReadFileInfo,
  PfTraceResult,
  PfWorkspace,
} from './projectForgeTypes';

// ============================ 常量 / 对外类型 ============================

export const SOURCE_WORKSPACE_STORAGE_KEY = 'vcp.projectforge.source.workspace';
export const SOURCE_PROJECT_STORAGE_KEY = 'vcp.projectforge.source.project';
const SOURCE_EXPANDED_PREFIX = 'vcp.projectforge.source.expanded.';

/** 搜索结果上限（与桌面侧栏 MAX_RENDER=400 同量级，手机端放宽到 500）。 */
export const SOURCE_SEARCH_LIMIT = 500;
/** 单个目录最多渲染的条目数（防御 node_modules 这类超大目录）。 */
export const SOURCE_DIR_ENTRY_LIMIT = 2000;
/** 只读查看最多渲染的行数（超出提示编辑查看）。 */
export const SOURCE_RENDER_LINE_LIMIT = 5000;
const PTY_TIMEOUT_MS = 20_000;
const TREE_ROOT = '';
const collator = new Intl.Collator('zh-CN', { numeric: true, sensitivity: 'base' });

/** 文件树传输通道：fileoperator = 桌面文件插件；pty = 降级 shell。 */
export type PfSourceTransport = 'unknown' | 'fileoperator' | 'pty';

/** 目录节点状态（懒加载：只有展开过的目录才会加载）。 */
export interface PfSourceDirState {
  loaded: boolean;
  loading: boolean;
  error: string;
  entries: PfFileEntry[];
}

/** 文件树扁平化后的一行。 */
export interface PfSourceTreeRow extends PfFileEntry {
  depth: number;
  open: boolean;
  loading: boolean;
}

/** 当前打开的源码文件。 */
export interface PfSourceFile {
  /** 工程内相对路径（POSIX）。 */
  path: string;
  /** 磁盘正文（已把 CRLF 归一为 LF，供展示/编辑）。 */
  text: string;
  /** 磁盘原文（保留原始 EOL，用于 EditCode target 精确替换）。 */
  rawText: string;
  lines: string[];
  lineCount: number;
  eol: 'LF' | 'CRLF';
  size?: number;
  /** 只读查看时是否因行数过多会被截断渲染。 */
  renderTruncated: boolean;
}

/** 符号大纲一项。 */
export interface PfSourceOutlineItem {
  name: string;
  kind?: string;
  start: number;
  end?: number;
  depth: number;
}

/** FindSymbol 一条命中。 */
export interface PfSourceSymbolHit {
  path: string;
  name: string;
  kind?: string;
  start?: number;
  end?: number;
  signature?: string;
  raw: string;
}

/** Trace 查询结果。 */
export interface PfSourceTrace {
  target: string;
  text: string;
}

/** 写操作结果（供组件展示回读校验信息）。 */
export interface PfSourceWriteResult {
  path: string;
  warnings: string[];
}

// ============================ 纯工具函数（导出便于单测） ============================

export function errorMessage(error: unknown): string {
  if (error instanceof Error && error.message) return error.message;
  const maybe = error as { message?: unknown } | null;
  if (maybe && typeof maybe.message === 'string' && maybe.message) return maybe.message;
  if (typeof error === 'string') return error;
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

/** 错误分级（沿用 T1 的 ProjectForgeError.kind；非桥接错误归为 bridge）。 */
export function errorKind(error: unknown): string {
  const maybe = error as { kind?: unknown } | null;
  return maybe && typeof maybe.kind === 'string' ? maybe.kind : 'bridge';
}

export function basename(path: string): string {
  const clean = String(path ?? '').replace(/\\/g, '/').replace(/\/+$/, '');
  const index = clean.lastIndexOf('/');
  return index >= 0 ? clean.slice(index + 1) : clean;
}

export function dirname(path: string): string {
  const clean = String(path ?? '').replace(/\\/g, '/').replace(/\/+$/, '');
  const index = clean.lastIndexOf('/');
  return index > 0 ? clean.slice(0, index) : '';
}

/** 把用户输入规整成工程内相对路径（POSIX、无前导 /、无 ./）。 */
export function normalizeRelativePath(input: unknown): string {
  let raw = String(input ?? '').trim().replace(/\\/g, '/');
  raw = raw.replace(/^\.\/+/, '').replace(/^\/+/, '');
  const parts = raw.split('/').filter((part) => part !== '' && part !== '.');
  const out: string[] = [];
  for (const part of parts) {
    if (part === '..') return '';
    out.push(part);
  }
  return out.join('/');
}

/** 校验并返回工程内相对路径；非法时抛出中文错误。 */
export function ensureRelativePath(input: unknown, label = '文件路径'): string {
  const raw = String(input ?? '').trim().replace(/\\/g, '/');
  if (!raw) throw new Error(`请填写${label}`);
  if (/^[A-Za-z]:/.test(raw)) throw new Error(`${label}必须是工程内相对路径，不能是 Windows 绝对路径`);
  if (raw.startsWith('/')) throw new Error(`${label}必须是工程内相对路径，不能以 / 开头`);
  const parts = raw.split('/').filter((part) => part !== '' && part !== '.');
  if (!parts.length) throw new Error(`${label}无效`);
  if (parts.some((part) => part === '..')) throw new Error(`${label}不能包含 ..`);
  return parts.join('/');
}

/** 拼接工程根目录与相对路径（保持根目录原有分隔符风格）。 */
export function joinProjectPath(root: string, rel: string): string {
  const base = String(root ?? '').trim().replace(/[\\/]+$/, '');
  const clean = normalizeRelativePath(rel);
  if (!base) return clean;
  if (!clean) return base;
  const sep = base.includes('\\') && !base.includes('/') ? '\\' : '/';
  return `${base}${sep}${clean.split('/').join(sep)}`;
}

/** Windows 路径 → Git Bash POSIX 路径（`D:\a\b` → `/d/a/b`）；POSIX 路径原样（斜杠归一）。 */
export function toShellPath(input: string): string {
  const raw = String(input ?? '').trim();
  if (!raw) return '.';
  const drive = /^([A-Za-z]):[\\/]?/.exec(raw);
  if (drive) {
    const rest = raw.slice(drive[0].length).replace(/\\/g, '/').replace(/^\/+/, '');
    return rest ? `/${drive[1].toLowerCase()}/${rest}` : `/${drive[1].toLowerCase()}`;
  }
  return raw.replace(/\\/g, '/');
}

/** 单引号 shell 转义。 */
export function shellQuote(value: string): string {
  return `'${String(value ?? '').replace(/'/g, `'\\''`)}'`;
}

export function splitSearchTokens(query: unknown): string[] {
  return String(query ?? '')
    .trim()
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean);
}

/** 末尾换行不敏感的文本比较（用于写后回读校验）。 */
export function normalizeTextForCompare(text: unknown): string {
  return String(text ?? '')
    .replace(/\r\n/g, '\n')
    .replace(/\r/g, '\n')
    .replace(/\n+$/, '');
}

export function sameFileText(a: unknown, b: unknown): boolean {
  return normalizeTextForCompare(a) === normalizeTextForCompare(b);
}

/** 磁盘原文的行数（不把末尾换行产生的空串算作一行）。 */
export function countContentLines(raw: unknown): number {
  const text = String(raw ?? '');
  if (!text) return 0;
  const parts = text.replace(/\r\n/g, '\n').replace(/\r/g, '\n').split('\n');
  const phantom = text.endsWith('\n') || text.endsWith('\r') ? 1 : 0;
  return Math.max(0, parts.length - phantom);
}

export function formatSourceSize(size?: number): string {
  if (typeof size !== 'number' || !Number.isFinite(size) || size < 0) return '—';
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  return `${(size / 1024 / 1024).toFixed(2)} MB`;
}

/** 由扩展名推断语言标签（只读查看的元信息展示）。 */
export function languageLabel(path: string): string {
  const ext = (basename(path).split('.').pop() || '').toLowerCase();
  const map: Record<string, string> = {
    ts: 'TypeScript',
    tsx: 'TypeScript',
    js: 'JavaScript',
    jsx: 'JavaScript',
    mjs: 'JavaScript',
    cjs: 'JavaScript',
    vue: 'Vue',
    json: 'JSON',
    jsonc: 'JSONC',
    md: 'Markdown',
    markdown: 'Markdown',
    css: 'CSS',
    scss: 'SCSS',
    less: 'Less',
    html: 'HTML',
    htm: 'HTML',
    py: 'Python',
    rs: 'Rust',
    go: 'Go',
    java: 'Java',
    kt: 'Kotlin',
    c: 'C',
    h: 'C',
    cc: 'C++',
    cpp: 'C++',
    hpp: 'C++',
    cs: 'C#',
    php: 'PHP',
    rb: 'Ruby',
    sh: 'Shell',
    bash: 'Shell',
    zsh: 'Shell',
    yml: 'YAML',
    yaml: 'YAML',
    toml: 'TOML',
    ini: 'INI',
    conf: '配置',
    sql: 'SQL',
    lua: 'Lua',
    xml: 'XML',
    svg: 'SVG',
    txt: '纯文本',
  };
  return map[ext] || (ext ? ext.toUpperCase() : '纯文本');
}

function tryParseJsonText(value: string): unknown {
  const text = value.trim();
  if (!text) return value;
  const first = text[0];
  if (first !== '{' && first !== '[') return value;
  try {
    return JSON.parse(text);
  } catch {
    return value;
  }
}

function isEmptyContainer(value: unknown): boolean {
  if (value === null || value === undefined || value === '') return true;
  if (Array.isArray(value)) return value.length === 0;
  if (typeof value === 'object') return Object.keys(value as Record<string, unknown>).length === 0;
  return false;
}

const ENTRY_ARRAY_KEYS: Array<{ key: string; isDir?: boolean }> = [
  { key: 'entries' },
  { key: 'items' },
  { key: 'children' },
  { key: 'files', isDir: false },
  { key: 'dirs', isDir: true },
  { key: 'directories', isDir: true },
  { key: 'folders', isDir: true },
  { key: 'list' },
  { key: 'results' },
  { key: 'matches' },
  { key: 'hits' },
];

function hasEntryArrayKeys(obj: Record<string, unknown>): boolean {
  return ENTRY_ARRAY_KEYS.some((item) => Array.isArray(obj[item.key]));
}

function contentToPayload(content: unknown): unknown {
  if (!Array.isArray(content)) return content;
  const texts = content
    .map((item) => {
      if (typeof item === 'string') return item;
      if (item && typeof item === 'object') {
        const text = (item as { text?: unknown }).text;
        if (typeof text === 'string') return text;
      }
      return '';
    })
    .filter((text) => text.trim() !== '');
  if (!texts.length) return content;
  const joined = texts.join('\n');
  const parsed = tryParseJsonText(joined);
  if (parsed !== joined) return parsed;
  return texts.length === 1 ? texts[0] : texts;
}

/** 兼容 插件直返 / bridge envelope / {details} / {data} 等多种返回形态。 */
export function unwrapFilePayload(raw: unknown): unknown {
  let value = typeof raw === 'string' ? tryParseJsonText(raw) : raw;
  for (let depth = 0; depth < 3; depth += 1) {
    if (!value || typeof value !== 'object' || Array.isArray(value)) return value;
    const obj = value as Record<string, unknown>;
    const result = obj.result;
    if (result && typeof result === 'object' && !Array.isArray(result)) {
      const resultObj = result as Record<string, unknown>;
      if (!isEmptyContainer(resultObj.details)) {
        value = resultObj.details;
        continue;
      }
      if (Array.isArray(resultObj.content) && !hasEntryArrayKeys(resultObj)) {
        value = contentToPayload(resultObj.content);
        continue;
      }
      value = resultObj;
      continue;
    }
    if (!isEmptyContainer(obj.details)) {
      value = obj.details;
      continue;
    }
    if (Array.isArray(obj.content) && !hasEntryArrayKeys(obj)) {
      value = contentToPayload(obj.content);
      continue;
    }
    if (obj.data !== undefined && !hasEntryArrayKeys(obj)) {
      value = obj.data;
      continue;
    }
    return value;
  }
  return value;
}

function entryName(raw: unknown): string {
  if (typeof raw === 'string') return basename(raw);
  if (!raw || typeof raw !== 'object') return '';
  const obj = raw as Record<string, unknown>;
  for (const key of ['name', 'fileName', 'filename', 'label', 'title', 'entryName']) {
    const value = obj[key];
    if (typeof value === 'string' && value.trim()) return basename(value.trim());
  }
  for (const key of ['path', 'filePath', 'fullPath', 'absolutePath', 'relPath', 'relativePath', 'pathRelative']) {
    const value = obj[key];
    if (typeof value === 'string' && value.trim()) return basename(value.trim());
  }
  return '';
}

function entryIsDir(raw: unknown, hint: boolean | undefined): boolean {
  if (!raw || typeof raw !== 'object') return hint === true;
  const obj = raw as Record<string, unknown>;
  for (const key of ['isDirectory', 'isDir', 'directory', 'isFolder']) {
    const value = obj[key];
    if (typeof value === 'boolean') return value;
  }
  const type = String(obj.type ?? obj.kind ?? obj.entryType ?? '').toLowerCase();
  if (type === 'directory' || type === 'dir' || type === 'folder' || type === 'tree') return true;
  if (type === 'file' || type === 'fileentry' || type === 'blob') return false;
  if (Array.isArray(obj.children) || Array.isArray(obj.entries)) return true;
  return hint === true;
}

function entrySize(raw: unknown): number | undefined {
  if (!raw || typeof raw !== 'object') return undefined;
  const obj = raw as Record<string, unknown>;
  for (const key of ['size', 'bytes', 'fileSize']) {
    const value = obj[key];
    if (typeof value === 'number' && Number.isFinite(value) && value >= 0) return value;
  }
  return undefined;
}

function entryRawPath(raw: unknown): string {
  if (typeof raw === 'string') return raw;
  if (!raw || typeof raw !== 'object') return '';
  const obj = raw as Record<string, unknown>;
  for (const key of ['path', 'filePath', 'fullPath', 'absolutePath', 'relPath', 'relativePath', 'pathRelative']) {
    const value = obj[key];
    if (typeof value === 'string' && value.trim()) return value;
  }
  return '';
}

/** 把插件返回的路径归一为工程内相对路径。 */
function entryRelativePath(raw: unknown, name: string, relDir: string, projectRootPath: string): string {
  let candidate = entryRawPath(raw).replace(/\\/g, '/').trim();
  const rootNorm = String(projectRootPath ?? '').replace(/\\/g, '/').replace(/\/+$/, '');
  if (candidate && rootNorm) {
    const lowerCandidate = candidate.toLowerCase();
    const lowerRoot = rootNorm.toLowerCase();
    if (lowerCandidate === lowerRoot) candidate = '';
    else if (lowerCandidate.startsWith(`${lowerRoot}/`)) {
      const stripped = candidate.slice(rootNorm.length).replace(/^\/+/, '');
      if (stripped) return stripped.replace(/\/+$/, '');
    }
  }
  candidate = candidate.replace(/^\.\//, '').replace(/^\/+/, '').replace(/\/+$/, '');
  if (candidate && relDir) {
    const prefix = `${relDir.toLowerCase()}/`;
    if (candidate.toLowerCase().startsWith(prefix)) return candidate;
  }
  if (candidate && candidate.includes('/') && !relDir) return candidate;
  return relDir ? `${relDir}/${name}` : name;
}

function sortEntries(entries: PfFileEntry[]): PfFileEntry[] {
  return [...entries].sort((a, b) => {
    if (a.type === 'dir' && b.type !== 'dir') return -1;
    if (a.type !== 'dir' && b.type === 'dir') return 1;
    return collator.compare(a.name, b.name);
  });
}

function dedupeEntries(entries: PfFileEntry[]): PfFileEntry[] {
  const seen = new Set<string>();
  const out: PfFileEntry[] = [];
  for (const entry of entries) {
    const key = entry.path.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(entry);
  }
  return out;
}

/** 过滤隐藏文件（与桌面侧栏默认 showHidden=false 一致）。 */
function filterHidden(entries: PfFileEntry[]): PfFileEntry[] {
  return entries.filter((entry) => !entry.name.startsWith('.'));
}

function buildEntries(
  raws: unknown[],
  relDir: string,
  projectRootPath: string,
  dirHint: boolean | undefined,
): PfFileEntry[] {
  const entries: PfFileEntry[] = [];
  for (const raw of raws) {
    if (raw === null || raw === undefined) continue;
    if (Array.isArray(raw)) {
      entries.push(...buildEntries(raw, relDir, projectRootPath, dirHint));
      continue;
    }
    const name = entryName(raw);
    if (!name || name === '.' || name === '..') continue;
    const isDir = entryIsDir(raw, dirHint);
    const path = entryRelativePath(raw, name, relDir, projectRootPath);
    const size = entrySize(raw);
    const entry: PfFileEntry = { name, path, type: isDir ? 'dir' : 'file' };
    if (size !== undefined) entry.size = size;
    entries.push(entry);
  }
  return entries;
}

function textEntry(
  namePath: string,
  isDir: boolean,
  relDir: string,
  projectRootPath: string,
  size?: number,
): PfFileEntry | null {
  const clean = String(namePath ?? '').replace(/\\/g, '/').replace(/\/+$/, '').trim();
  if (!clean || clean === '.' || clean === '..') return null;
  const name = basename(clean);
  if (!name) return null;
  const entry: PfFileEntry = {
    name,
    path: entryRelativePath(clean, name, relDir, projectRootPath),
    type: isDir ? 'dir' : 'file',
  };
  if (size !== undefined) entry.size = size;
  return entry;
}

const ERROR_LINE_RE =
  /错误|失败|不存在|无法|拒绝|无权|not\s+found|no\s+such|permission\s+denied|denied|error|invalid/i;
const LS_LINE_RE = /^([-dlbcps])([rwxsStT-]{9})\s+\d+\s+\S+\s+\S+\s+(\d+)\s+\S+\s+\d+\s+[\d:]+\s+(.+)$/;

/** 解析 FileOperator 可能返回的纯文本目录列表（JSON 之外的人可读文本）。 */
export function parsePlainDirectoryText(
  text: string,
  relDir: string,
  projectRootPath: string,
): PfFileEntry[] | null {
  const rawLines = String(text ?? '')
    .split(/\r?\n/)
    .map((line) => line.replace(/\r/g, ''))
    .filter((line) => line.trim() !== '');
  if (!rawLines.length) return [];
  const entries: PfFileEntry[] = [];
  let parsedLines = 0;
  for (const rawLine of rawLines) {
    const line = rawLine.trim();
    const lsMatch = LS_LINE_RE.exec(line);
    if (lsMatch) {
      let name = lsMatch[4].trim();
      if (name === '.' || name === '..') continue;
      const arrowIndex = name.indexOf(' -> ');
      if (arrowIndex > 0) name = name.slice(0, arrowIndex);
      const size = Number(lsMatch[3]);
      const entry = textEntry(
        name,
        lsMatch[1] === 'd',
        relDir,
        projectRootPath,
        Number.isFinite(size) ? size : undefined,
      );
      if (entry) {
        entries.push(entry);
        parsedLines += 1;
      }
      continue;
    }
    if (/^(total|Directory of|find:|ls:|drwxr)/i.test(line)) continue;
    if (ERROR_LINE_RE.test(line)) return null;
    const folderHint = /^[📁📂🗀🗁]/u.test(line);
    let namePart = line.replace(/^[📁📄📂🗀🗁\s>*+-]+/u, '').trim();
    let isDir = folderHint;
    if (/[\\/]$/.test(line)) isDir = true;
    if (/[（(](?:directory|dir|folder|文件夹|目录)[)）]$/i.test(namePart)) {
      isDir = true;
      namePart = namePart.replace(/[（(](?:directory|dir|folder|文件夹|目录)[)）]$/i, '').trim();
    } else if (/[（(](?:file|文件)[)）]$/i.test(namePart)) {
      namePart = namePart.replace(/[（(](?:file|文件)[)）]$/i, '').trim();
    }
    let size: number | undefined;
    const sizeMatch = /[（(]?\s*(\d+(?:\.\d+)?)\s*(B|KB|MB|GB)\s*[)）]?$/i.exec(namePart);
    if (sizeMatch) {
      const units: Record<string, number> = { B: 1, KB: 1024, MB: 1024 * 1024, GB: 1024 * 1024 * 1024 };
      size = Math.round(Number(sizeMatch[1]) * (units[sizeMatch[2].toUpperCase()] || 1));
      namePart = namePart.slice(0, sizeMatch.index).trim();
    }
    const entry = textEntry(namePart, isDir, relDir, projectRootPath, size);
    if (entry) {
      entries.push(entry);
      parsedLines += 1;
    }
  }
  if (!parsedLines && rawLines.length) return null;
  return dedupeEntries(entries);
}

/**
 * 归一化 FileOperator.ListDirectory 的返回：支持数组 / {entries|items|files|dirs|children} /
 * 纯文本（JSON 字符串或 `ls` 风格）。无法识别时返回 null（调用方据此降级 PTY）。
 */
export function normalizeDirectoryPayload(
  payload: unknown,
  relDir: string,
  projectRootPath: string,
): PfFileEntry[] | null {
  const unwrapped = unwrapFilePayload(payload);
  if (Array.isArray(unwrapped)) return sortEntries(dedupeEntries(buildEntries(unwrapped, relDir, projectRootPath, undefined)));
  if (typeof unwrapped === 'string') {
    const plain = parsePlainDirectoryText(unwrapped, relDir, projectRootPath);
    return plain === null ? null : sortEntries(plain);
  }
  if (unwrapped && typeof unwrapped === 'object') {
    const obj = unwrapped as Record<string, unknown>;
    const groups = ENTRY_ARRAY_KEYS.filter((item) => Array.isArray(obj[item.key]));
    if (!groups.length) return null;
    const entries: PfFileEntry[] = [];
    for (const group of groups) {
      entries.push(...buildEntries(obj[group.key] as unknown[], relDir, projectRootPath, group.isDir));
    }
    return sortEntries(dedupeEntries(entries));
  }
  return null;
}

/** 归一化 FileOperator.SearchFiles 的返回；无法识别时返回 null。 */
export function normalizeSearchPayload(payload: unknown, projectRootPath: string): PfFileEntry[] | null {
  const unwrapped = unwrapFilePayload(payload);
  if (Array.isArray(unwrapped)) {
    return dedupeEntries(buildEntries(unwrapped, '', projectRootPath, false).filter((entry) => entry.type !== 'dir'));
  }
  if (typeof unwrapped === 'string') {
    const lines = unwrapped
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean);
    if (!lines.length) return [];
    const entries: PfFileEntry[] = [];
    for (const line of lines) {
      if (ERROR_LINE_RE.test(line)) return null;
      const cleaned = line
        .replace(/^[📄📁\s>*+-]+/u, '')
        .replace(/^\.\//, '')
        .trim();
      if (!cleaned) continue;
      const name = basename(cleaned);
      if (!name) continue;
      entries.push({ name, path: entryRelativePath(cleaned, name, '', projectRootPath), type: 'file' });
    }
    return dedupeEntries(entries);
  }
  if (unwrapped && typeof unwrapped === 'object') {
    const obj = unwrapped as Record<string, unknown>;
    const keys = ['files', 'results', 'matches', 'hits', 'entries', 'items', 'list'].filter((key) =>
      Array.isArray(obj[key]),
    );
    if (!keys.length) return null;
    const entries: PfFileEntry[] = [];
    for (const key of keys) {
      entries.push(...buildEntries(obj[key] as unknown[], '', projectRootPath, false));
    }
    return dedupeEntries(entries.filter((entry) => entry.type !== 'dir'));
  }
  return null;
}

/** 解析 PTY `find -printf '%y\t%f\t%s\n'` 输出。 */
export function parsePtyFindOutput(text: string, relDir: string): PfFileEntry[] {
  const entries: PfFileEntry[] = [];
  for (const rawLine of String(text ?? '').split(/\r?\n/)) {
    const line = rawLine.replace(/\r/g, '');
    if (!line.trim()) continue;
    if (/^(find:|ls:|cd:|bash:|\/bin\/sh:)/i.test(line.trim())) continue;
    const fields = line.split('\t');
    if (fields.length < 2) continue;
    const type = fields[0].trim();
    const name = fields[1];
    if (!name || name === '.' || name === '..') continue;
    const size = Number(fields[2]);
    const entry: PfFileEntry = {
      name,
      path: relDir ? `${relDir}/${name}` : name,
      type: type.startsWith('d') ? 'dir' : 'file',
    };
    if (Number.isFinite(size)) entry.size = size;
    entries.push(entry);
  }
  return sortEntries(dedupeEntries(entries));
}

export interface ParsedReadCode {
  /** 归一化 EOL 后的正文（展示 / 编辑用）。 */
  text: string;
  /** 桥接返回的原始正文（保留 EOL，EditCode target 精确替换用）。 */
  rawText: string;
  eol: 'LF' | 'CRLF';
  size?: number;
  multimodal: boolean;
  failed: string[];
  skipped: string[];
}

/** 把对象/字符串列表归一成可读字符串列表（details.failed / details.skipped）。 */
export function normalizeStringList(value: unknown): string[] {
  if (value === null || value === undefined) return [];
  if (typeof value === 'string') return value.trim() ? [value.trim()] : [];
  if (Array.isArray(value)) {
    return value
      .map((item) => {
        if (typeof item === 'string') return item;
        if (item && typeof item === 'object') {
          const obj = item as Record<string, unknown>;
          const path = obj.path ?? obj.file ?? obj.filePath;
          const reason = obj.reason ?? obj.error ?? obj.message;
          const parts = [path, reason].filter((part): part is string => typeof part === 'string' && part !== '');
          if (parts.length) return parts.join('：');
          try {
            return JSON.stringify(item);
          } catch {
            return String(item);
          }
        }
        return String(item ?? '');
      })
      .filter(Boolean);
  }
  if (typeof value === 'object') {
    try {
      return [JSON.stringify(value)];
    } catch {
      return [String(value)];
    }
  }
  return [String(value)];
}

/**
 * 解析 ReadCode 的 bridge envelope。
 * 桌面 ReadCode 恒为 content[0]=摘要 + content[1..]=正文（默认带行号，showLine:false 时不带）。
 */
/**
 * 桌面 ReadCode 的正文 part 形如：
 * ```
 * #### path (386 Bytes)
 * - 总行数：9
 *
 * ```markdown
 * <文件正文>
 * ```
 * ```
 * 这里剥掉文件头（`#### path (size)` 与 `- ...` 元信息行）和自适应长度的 md 围栏，
 * 只返回文件真实正文（保留原始 EOL）。无法识别时原样返回。
 */
export function extractReadCodeBody(part: unknown): string {
  const text = String(part ?? '');
  // 围栏长度自适应（正文含 ``` 时桌面端会加长围栏），用反向引用匹配同一长度闭合围栏。
  const match =
    /^#### [^\r\n]*\r?\n(?:- [^\r\n]*\r?\n)*\r?\n(`{3,})[A-Za-z0-9_+.-]*[ \t]*\r?\n([\s\S]*?)\r?\n\1$/.exec(
      text,
    );
  return match ? match[2] : text;
}

export function parseReadCodeEnvelope(envelope: PfBridgeEnvelope): ParsedReadCode {
  const parts: PfBridgeContentPart[] = bridgeContentParts(envelope);
  const details = (envelope.result?.details ?? {}) as Partial<PfReadCodeResult> & Record<string, unknown>;
  const failed = normalizeStringList(details.failed);
  const skipped = normalizeStringList(details.skipped);
  const files = Array.isArray(details.files) ? (details.files as PfReadFileInfo[]) : [];
  const readInfos = Array.isArray((details as { read?: unknown }).read)
    ? ((details as { read?: PfReadFileInfo[] }).read as PfReadFileInfo[])
    : [];
  const info = files[0] || readInfos[0] || ({} as PfReadFileInfo);
  const infoEol = typeof info.eol === 'string' ? String(info.eol).toUpperCase() : '';
  const multimodal = parts.some((part) => /image|audio|video/i.test(String(part.type ?? '')));
  const textParts = parts.map((part) => String(part.text ?? ''));
  const bodyParts = textParts.length > 1 ? textParts.slice(1).filter((text) => text !== '') : textParts.slice(0, 1);
  const rawText = bodyParts.map((text) => extractReadCodeBody(text)).join('\n\n');
  const eol: 'LF' | 'CRLF' = rawText.includes('\r\n') || infoEol.includes('CRLF') ? 'CRLF' : 'LF';
  const size = typeof info.size === 'number' ? info.size : undefined;
  const text = rawText.replace(/\r\n/g, '\n').replace(/\r/g, '\n');
  return { text, rawText, eol, size, multimodal, failed, skipped };
}

/** 解析符号大纲（details.files[].symbols 优先，退回 content 文本）。 */
export function parseOutlineEnvelope(envelope: PfBridgeEnvelope): PfSourceOutlineItem[] {
  const details = (envelope.result?.details ?? null) as Partial<PfOutlineResult> | null;
  const items: PfSourceOutlineItem[] = [];
  if (details && Array.isArray(details.files)) {
    for (const file of details.files) {
      const symbols = Array.isArray(file.symbols) ? (file.symbols as PfOutlineSymbol[]) : [];
      for (const symbol of symbols) {
        const name = String(symbol.qualified ?? symbol.name ?? '').trim();
        const start = Number(symbol.start ?? symbol.fullStart ?? 0);
        if (!name || !Number.isFinite(start) || start <= 0) continue;
        const endValue = Number(symbol.end ?? 0);
        const depthValue = Number(symbol.depth ?? 0);
        const item: PfSourceOutlineItem = {
          name,
          start,
          depth: Number.isFinite(depthValue) && depthValue > 0 ? Math.round(depthValue) : 0,
        };
        if (symbol.kind) item.kind = String(symbol.kind);
        if (Number.isFinite(endValue) && endValue >= start) item.end = endValue;
        items.push(item);
      }
    }
  }
  if (!items.length) items.push(...parseOutlineText(bridgeContentText(envelope)));
  return items;
}

/** 解析大纲纯文本（如 `- foo (function) L10-20`）。 */
export function parseOutlineText(text: string): PfSourceOutlineItem[] {
  const items: PfSourceOutlineItem[] = [];
  for (const rawLine of String(text ?? '').split(/\r?\n/)) {
    if (!rawLine.trim()) continue;
    const indent = /^\s*/.exec(rawLine)?.[0].length ?? 0;
    const line = rawLine.trim().replace(/^[-*+]\s+/, '').replace(/^#+\s*/, '');
    const rangeMatch = /\bL(\d+)(?:\s*-\s*(\d+))?/.exec(line);
    if (!rangeMatch) continue;
    const headRaw = line
      .slice(0, rangeMatch.index)
      .trim()
      .replace(/[·|,]+$/, '')
      .trim();
    const tail = line.slice(rangeMatch.index + rangeMatch[0].length).replace(/^[·\s]+/, '').trim();
    if (!headRaw) continue;
    let head = headRaw;
    let kind: string | undefined;
    const kindMatch = /[（(]([A-Za-z_#]+)[)）]\s*$/.exec(head);
    if (kindMatch) {
      kind = kindMatch[1];
      head = head.replace(kindMatch[0], '').trim();
    }
    if (!kind && tail) {
      const first = tail.split(/[·\s]+/).filter(Boolean)[0];
      if (first && /^[A-Za-z_][A-Za-z0-9_]{1,}$/.test(first)) kind = first;
    }
    let name = head.trim();
    const hashIndex = name.indexOf('#');
    if (hashIndex > 0) name = name.slice(hashIndex + 1).trim();
    if (!name) continue;
    const start = Number(rangeMatch[1]);
    const end = rangeMatch[2] ? Number(rangeMatch[2]) : undefined;
    const item: PfSourceOutlineItem = { name, start, depth: Math.round(indent / 2) };
    if (kind) item.kind = kind;
    if (end !== undefined) item.end = end;
    items.push(item);
  }
  return items;
}

export interface ParsedFindSymbol {
  hits: PfSourceSymbolHit[];
  total: number;
  note: string;
}

/** 解析 FindSymbol（details.hits 优先，退回 content 文本）。 */
export function parseFindSymbolEnvelope(envelope: PfBridgeEnvelope): ParsedFindSymbol {
  const details = (envelope.result?.details ?? null) as Partial<PfFindSymbolResult> | null;
  const hits: PfSourceSymbolHit[] = [];
  let total = 0;
  if (details) {
    total = Number(details.total ?? 0) || 0;
    const rawHits = Array.isArray(details.hits) ? details.hits : [];
    for (const hit of rawHits) {
      const path = String(hit.path ?? '').trim();
      if (!path) continue;
      const name = String(hit.qualified ?? hit.name ?? '').trim() || path;
      const startValue = Number(hit.start ?? hit.fullStart ?? 0);
      const endValue = Number(hit.end ?? 0);
      const item: PfSourceSymbolHit = { path, name, raw: `${path}#${name}` };
      if (hit.kind) item.kind = String(hit.kind);
      if (Number.isFinite(startValue) && startValue > 0) item.start = startValue;
      if (Number.isFinite(endValue) && endValue > 0) item.end = endValue;
      if (hit.signature) item.signature = String(hit.signature);
      hits.push(item);
    }
  }
  if (!hits.length) hits.push(...parseFindSymbolText(bridgeContentText(envelope)));
  if (!total) total = hits.length;
  const parts: string[] = [];
  if (typeof details?.scanned === 'number') parts.push(`扫描 ${details.scanned} 个文件`);
  if (details?.truncated === true) parts.push('结果已截断');
  const note = parts.join(' · ');
  return { hits, total, note };
}

/** 解析 FindSymbol 纯文本行（如 `src/a.js#Store.put · L10-20 · method · 签名`）。 */
export function parseFindSymbolText(text: string): PfSourceSymbolHit[] {
  const hits: PfSourceSymbolHit[] = [];
  for (const rawLine of String(text ?? '').split(/\r?\n/)) {
    const trimmed = rawLine.trim().replace(/^[-*+]\s+/, '');
    if (!trimmed) continue;
    const hashIndex = trimmed.indexOf('#');
    const rangeMatch = /\bL(\d+)(?:\s*-\s*(\d+))?/.exec(trimmed);
    if (hashIndex <= 0 || !rangeMatch || rangeMatch.index < hashIndex) continue;
    const path = trimmed.slice(0, hashIndex).trim().replace(/\s+/g, '');
    if (!path) continue;
    const name = trimmed
      .slice(hashIndex + 1, rangeMatch.index)
      .replace(/[·\s]+$/, '')
      .trim();
    if (!name) continue;
    const after = trimmed
      .slice(rangeMatch.index + rangeMatch[0].length)
      .replace(/^[·\s]+/, '')
      .trim();
    const segments = after.split('·').map((segment) => segment.trim()).filter(Boolean);
    let kind: string | undefined;
    let signature: string | undefined;
    if (segments.length >= 2) {
      kind = segments[0];
      signature = segments.slice(1).join(' · ');
    } else if (segments.length === 1) {
      if (/^[A-Za-z_][A-Za-z0-9_]*$/.test(segments[0])) kind = segments[0];
      else signature = segments[0];
    }
    const hit: PfSourceSymbolHit = { path, name, raw: trimmed };
    if (kind) hit.kind = kind;
    if (signature) hit.signature = signature;
    const start = Number(rangeMatch[1]);
    if (Number.isFinite(start)) hit.start = start;
    if (rangeMatch[2]) hit.end = Number(rangeMatch[2]);
    hits.push(hit);
  }
  return hits;
}

/** 解析 Trace（content 文本优先，退回 details.text / JSON）。 */
export function parseTraceEnvelope(envelope: PfBridgeEnvelope, target: string): PfSourceTrace {
  const details = (envelope.result?.details ?? null) as Partial<PfTraceResult> | null;
  let text = bridgeContentText(envelope);
  if (!text && details && typeof details.text === 'string') text = details.text;
  if (!text && details) {
    try {
      text = JSON.stringify(details, null, 2);
    } catch {
      text = String(details);
    }
  }
  return { target, text: text || '（没有可显示的链路信息）' };
}

/** 从桥接结果里尽量抽取「代码审查」告警文案。 */
export function extractReviewWarnings(response: unknown): string[] {
  const payload = unwrapFilePayload(response);
  if (!payload || typeof payload !== 'object') return [];
  const warnings: string[] = [];
  const visit = (value: unknown, depth: number): void => {
    if (depth > 3 || !value) return;
    if (typeof value === 'string') return;
    if (Array.isArray(value)) {
      for (const item of value) {
        if (typeof item === 'string') warnings.push(item);
        else if (item && typeof item === 'object') {
          const obj = item as Record<string, unknown>;
          const message = obj.message ?? obj.issue ?? obj.text ?? obj.reason;
          if (typeof message === 'string' && message.trim()) warnings.push(message.trim());
          else visit(item, depth + 1);
        }
      }
      return;
    }
    if (typeof value === 'object') {
      const obj = value as Record<string, unknown>;
      for (const key of ['issues', 'warnings', 'errors', 'review', 'lint']) {
        if (obj[key] !== undefined) visit(obj[key], depth + 1);
      }
    }
  };
  const obj = payload as Record<string, unknown>;
  for (const key of ['review', 'issues', 'warnings', 'errors']) {
    if (obj[key] !== undefined) visit(obj[key], 0);
  }
  return warnings.filter((item, index) => warnings.indexOf(item) === index).slice(0, 5);
}

/** 目录树扁平化后的可见行（组件与测试共用）。 */
export function flattenTreeRows(
  dirs: Record<string, PfSourceDirState>,
  expanded: readonly string[],
): PfSourceTreeRow[] {
  const expandedSet = new Set(expanded);
  const rows: PfSourceTreeRow[] = [];
  const walk = (dirPath: string, depth: number): void => {
    const state = dirs[dirPath];
    if (!state || !state.loaded) return;
    for (const entry of state.entries) {
      const open = entry.type === 'dir' && expandedSet.has(entry.path);
      rows.push({ ...entry, depth, open, loading: entry.type === 'dir' ? Boolean(dirs[entry.path]?.loading) : false });
      if (entry.type === 'dir' && open) walk(entry.path, depth + 1);
    }
  };
  walk(TREE_ROOT, 0);
  return rows;
}

// ============================ 模块级状态（单例） ============================

const workspaces = ref<PfWorkspace[]>([]);
const workspacesLoading = ref(false);
const workspacesError = ref('');
const selectedWorkspaceAlias = ref('');

const projects = ref<PfProject[]>([]);
const projectsLoading = ref(false);
const projectsError = ref('');
const selectedProjectId = ref('');
const selectedProject = ref<PfProject | null>(null);
const projectRoot = ref('');
const projectLoading = ref(false);
const projectError = ref('');

const transport = ref<PfSourceTransport>('unknown');
const transportNote = ref('');

// FileOperator 只允许访问桌面插件配置里的 ALLOWED_DIRECTORIES 白名单；ProjectForge 工程根目录
// 往往不在白名单内（如 /home/user/...），这属于能力边界而非错误：
// 先在本地判断，命中白名单才尝试 FileOperator，否则静默走 PTY 兼容模式，避免界面误报“错误”。
const allowedRoots = ref<string[] | null>(null);
const ALLOWED_ROOTS_TTL_MS = 60_000;
let allowedRootsFetchedAt = 0;
let allowedRootsPromise: Promise<string[] | null> | null = null;

const dirs = reactive<Record<string, PfSourceDirState>>({});
const expanded = ref<string[]>([]);
const treeError = ref('');

const searchQuery = ref('');
const searchResults = ref<PfFileEntry[]>([]);
const searchTotal = ref(0);
const searchLoading = ref(false);
const searchError = ref('');
const searchTransport = ref<'fileoperator' | 'pty' | ''>('');
const searchNote = ref('');

const currentFile = ref<PfSourceFile | null>(null);
const fileLoading = ref(false);
const fileError = ref('');

const outline = ref<PfSourceOutlineItem[]>([]);
const outlineLoading = ref(false);
const outlineError = ref('');
const outlinePath = ref('');

const findSymbolQuery = ref('');
const findSymbolHits = ref<PfSourceSymbolHit[]>([]);
const findSymbolNote = ref('');
const findSymbolLoading = ref(false);
const findSymbolError = ref('');

const traceTarget = ref('');
const traceResult = ref<PfSourceTrace | null>(null);
const traceLoading = ref(false);
const traceError = ref('');

const saving = ref(false);
const actionBusy = ref(false);
const actionError = ref('');
const lastWriteNotice = ref('');
const initialized = ref(false);

// ============================ 持久化小工具 ============================

function readStorage(key: string): string {
  try {
    return String(globalThis.localStorage?.getItem(key) ?? '');
  } catch {
    return '';
  }
}

function writeStorage(key: string, value: string): void {
  try {
    if (value) globalThis.localStorage?.setItem(key, value);
    else globalThis.localStorage?.removeItem(key);
  } catch {
    // localStorage 不可用时忽略（隐私模式 / 测试环境）
  }
}

function expandedStorageKey(projectId: string): string {
  return `${SOURCE_EXPANDED_PREFIX}${projectId}`;
}

function readExpanded(projectId: string): string[] {
  if (!projectId) return [];
  try {
    const parsed = JSON.parse(readStorage(expandedStorageKey(projectId)) || '[]');
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((item): item is string => typeof item === 'string' && item !== '');
  } catch {
    return [];
  }
}

function saveExpanded(): void {
  const projectId = selectedProjectId.value;
  if (!projectId) return;
  writeStorage(expandedStorageKey(projectId), JSON.stringify(expanded.value.slice(-1000)));
}

function isExpanded(path: string): boolean {
  return expanded.value.includes(path);
}

function setExpanded(path: string, open: boolean): void {
  if (!path) return;
  const has = expanded.value.includes(path);
  if (open === has) return;
  expanded.value = open ? [...expanded.value, path] : expanded.value.filter((item) => item !== path);
  saveExpanded();
}

// ============================ 传输层：文件树 / 搜索 ============================

const PTY_BEGIN = '__PF_SOURCE_BEGIN__';
const PTY_END = '__PF_SOURCE_END__';

/** 从 PTY 回显里裁出 BEGIN/END 标记之间的真实输出。 */
export function extractMarkedOutput(text: string, begin = PTY_BEGIN, end = PTY_END): string {
  const lines = String(text ?? '').split(/\r?\n/);
  const start = lines.findIndex((line) => line.trim() === begin);
  if (start < 0) return String(text ?? '');
  for (let index = start + 1; index < lines.length; index += 1) {
    if (lines[index].trim() === end) return lines.slice(start + 1, index).join('\n');
  }
  return lines.slice(start + 1).join('\n');
}

async function runPty(command: string): Promise<string> {
  const result = await ptyExec(command, { timeoutMs: PTY_TIMEOUT_MS });
  const text = String(result?.text ?? '');
  if (!result?.ok) throw new Error(text.trim() || 'PTY 命令执行失败（无输出）');
  return text;
}

function markedPtyCommand(inner: string): string {
  return `echo ${PTY_BEGIN}; ${inner}; echo ${PTY_END}`;
}

function ptyDirFor(relDir: string): string {
  const root = projectRoot.value;
  if (!root) throw new Error('请先选择工程');
  const abs = relDir ? joinProjectPath(root, relDir) : root;
  return shellQuote(toShellPath(abs));
}

async function ptyListDirectory(relDir: string): Promise<PfFileEntry[]> {
  const dir = ptyDirFor(relDir);
  let text: string;
  try {
    text = await runPty(
      markedPtyCommand(`cd ${dir} && LC_ALL=C find . -maxdepth 1 -mindepth 1 -printf '%y\\t%f\\t%s\\n'`),
    );
  } catch {
    text = await runPty(markedPtyCommand(`cd ${dir} && LC_ALL=C ls -la`));
  }
  const body = extractMarkedOutput(text);
  if (/no such file or directory|not a directory|permission denied|无法|不存在/i.test(body)) {
    throw new Error(`PTY 无法读取目录：${body.trim().split(/\r?\n/)[0] || '未知错误'}`);
  }
  const entries = parsePtyFindOutput(body, relDir);
  if (entries.length) return filterHidden(entries);
  // find -printf 不可用的环境（极老 Git Bash / BusyBox）退化为 ls -la 文本
  const plain = parsePlainDirectoryText(body, relDir, projectRoot.value);
  if (plain) return filterHidden(plain);
  return [];
}

async function ptySearchFiles(tokens: string[]): Promise<PfFileEntry[]> {
  const root = projectRoot.value;
  if (!root) throw new Error('请先选择工程');
  const shellRoot = shellQuote(toShellPath(root));
  const pattern = `*${tokens[0]}*`;
  const find = [
    `cd ${shellRoot}`,
    `LC_ALL=C find . -type f -iname ${shellQuote(pattern)} -not -path './node_modules/*' -not -path './.git/*' | head -n ${SOURCE_SEARCH_LIMIT}`,
  ].join(' && ');
  const text = await runPty(markedPtyCommand(find));
  const body = extractMarkedOutput(text);
  const entries: PfFileEntry[] = [];
  for (const rawLine of body.split(/\r?\n/)) {
    const line = rawLine.trim().replace(/^\.\//, '');
    if (!line) continue;
    if (/^(find:|ls:|cd:)/i.test(line)) continue;
    const name = basename(line);
    if (!name) continue;
    entries.push({ name, path: line, type: 'file' });
  }
  return filterByTokens(dedupeEntries(entries), tokens);
}

function filterByTokens(entries: PfFileEntry[], tokens: string[]): PfFileEntry[] {
  if (!tokens.length) return entries;
  return entries.filter((entry) => {
    const haystack = entry.path.toLowerCase();
    return tokens.every((token) => haystack.includes(token));
  });
}

/** 归一化白名单目录：统一分隔符、去尾斜杠、小写（Linux/Windows 都按大小写不敏感比较，宽松匹配）。 */
function normalizeRootPath(input: unknown): string {
  return String(input ?? '')
    .trim()
    .replace(/\\/g, '/')
    .replace(/\/+$/, '')
    .toLowerCase();
}

/** 判断绝对路径是否位于白名单目录之下。 */
function isPathUnderRoots(absPath: string, roots: string[]): boolean {
  const target = normalizeRootPath(absPath);
  if (!target) return false;
  return roots.some((root) => {
    const normalized = normalizeRootPath(root);
    if (!normalized) return false;
    return target === normalized || target.startsWith(`${normalized}/`);
  });
}

/** 读取 FileOperator 的允许目录白名单（缓存 60s；获取失败返回 null=未知，按可尝试处理）。 */
async function ensureAllowedRoots(): Promise<string[] | null> {
  const now = Date.now();
  if (allowedRoots.value !== null && now - allowedRootsFetchedAt < ALLOWED_ROOTS_TTL_MS) {
    return allowedRoots.value;
  }
  if (allowedRootsPromise) return allowedRootsPromise;
  allowedRootsPromise = (async () => {
    try {
      const payload = await pluginInvoke<unknown>('FileOperator', { command: 'ListAllowedDirectories' });
      const raw = (payload as { allowedRoots?: unknown })?.allowedRoots;
      const roots = Array.isArray(raw)
        ? raw.map((item) => String(item ?? '')).filter(Boolean)
        : null;
      allowedRoots.value = roots;
      allowedRootsFetchedAt = Date.now();
      return roots;
    } catch {
      // 拿不到白名单（插件未启用等）时返回 null：沿用旧行为尝试 FileOperator，失败再降级。
      return null;
    } finally {
      allowedRootsPromise = null;
    }
  })();
  return allowedRootsPromise;
}

/** 该绝对路径是否值得尝试 FileOperator。 */
async function canUseFileOperator(absPath: string): Promise<boolean> {
  if (!absPath) return true;
  const roots = await ensureAllowedRoots();
  if (!roots || !roots.length) return true;
  return isPathUnderRoots(absPath, roots);
}

async function fileOperatorListDirectory(relDir: string): Promise<PfFileEntry[]> {
  const root = projectRoot.value;
  if (!root) throw new Error('请先选择工程');
  const payload = await pluginInvoke<unknown>('FileOperator', {
    command: 'ListDirectory',
    directoryPath: joinProjectPath(root, relDir),
    showHidden: false,
  });
  const entries = normalizeDirectoryPayload(payload, relDir, root);
  if (entries === null) throw new Error('FileOperator.ListDirectory 返回结构无法识别');
  return filterHidden(entries).slice(0, SOURCE_DIR_ENTRY_LIMIT);
}

async function fileOperatorSearchFiles(tokens: string[]): Promise<PfFileEntry[]> {
  const root = projectRoot.value;
  if (!root) throw new Error('请先选择工程');
  const keyword = tokens[0];
  // SearchFiles 的 pattern 语义桌面清单未写明：先按通配（*kw*）试，再按原文试；都为空才算没命中。
  const patterns = [`*${keyword}*`, keyword];
  let lastResult: PfFileEntry[] = [];
  let lastError: unknown = null;
  let anySuccess = false;
  for (const pattern of patterns) {
    try {
      const payload = await pluginInvoke<unknown>('FileOperator', {
        command: 'SearchFiles',
        searchPath: root,
        pattern,
      });
      const entries = normalizeSearchPayload(payload, root);
      if (entries === null) throw new Error('FileOperator.SearchFiles 返回结构无法识别');
      anySuccess = true;
      lastResult = filterByTokens(entries, tokens);
      if (lastResult.length) return lastResult.slice(0, SOURCE_SEARCH_LIMIT);
    } catch (error) {
      lastError = error;
    }
  }
  if (!anySuccess && lastError) throw lastError;
  return lastResult.slice(0, SOURCE_SEARCH_LIMIT);
}

async function listDirectory(relDir: string): Promise<PfFileEntry[]> {
  const root = projectRoot.value;
  const absPath = root ? joinProjectPath(root, relDir) : '';
  // 白名单外的工程根目录直接走 PTY 兼容模式：不做无效调用，也不向用户展示“错误”。
  if (root && !(await canUseFileOperator(absPath))) {
    transport.value = 'pty';
    transportNote.value = '';
    return ptyListDirectory(relDir);
  }
  if (transport.value !== 'pty') {
    try {
      const entries = await fileOperatorListDirectory(relDir);
      if (transport.value !== 'fileoperator') {
        transport.value = 'fileoperator';
        transportNote.value = '';
      }
      return entries;
    } catch (error) {
      console.warn('[V工程] FileOperator 列目录失败，改用 PTY 兼容模式：', errorMessage(error));
      transport.value = 'pty';
      transportNote.value = '桌面文件插件暂不可用，已自动使用兼容模式读取（不影响文件浏览）';
    }
  }
  return ptyListDirectory(relDir);
}

async function searchByTransport(
  tokens: string[],
): Promise<{ entries: PfFileEntry[]; via: 'fileoperator' | 'pty' }> {
  const root = projectRoot.value;
  const allowed = root ? await canUseFileOperator(root) : true;
  if (allowed && transport.value !== 'pty') {
    try {
      const entries = await fileOperatorSearchFiles(tokens);
      searchNote.value = '';
      return { entries, via: 'fileoperator' };
    } catch (error) {
      console.warn('[V工程] FileOperator 搜索失败，改用 PTY 兼容模式：', errorMessage(error));
      searchNote.value = '桌面文件插件搜索暂不可用，已自动使用兼容模式搜索';
    }
  } else {
    searchNote.value = '';
  }
  return { entries: await ptySearchFiles(tokens), via: 'pty' };
}

// ============================ 目录树状态 ============================

function getDirState(path: string): PfSourceDirState {
  // 注意：必须返回 dirs 里的响应式代理（而不是新建的裸对象），否则后续对 loading/entries 的
  // 修改不会触发 computed 更新，文件树会卡在「正在读取文件树…」。
  if (!dirs[path]) dirs[path] = { loaded: false, loading: false, error: '', entries: [] };
  return dirs[path] as PfSourceDirState;
}

async function loadDir(relDir: string, options: { force?: boolean } = {}): Promise<void> {
  const state = getDirState(relDir);
  if (!options.force && (state.loading || state.loaded)) return;
  state.loading = true;
  state.error = '';
  try {
    const entries = await listDirectory(relDir);
    state.entries = entries;
    state.loaded = true;
    if (relDir === TREE_ROOT) treeError.value = '';
  } catch (error) {
    state.loaded = false;
    state.entries = [];
    state.error = errorMessage(error);
    if (relDir === TREE_ROOT) treeError.value = state.error;
    throw error;
  } finally {
    state.loading = false;
  }
}

/** 恢复持久化的展开目录（按层级从浅到深加载）。 */
async function restoreExpandedDirs(): Promise<void> {
  const paths = [...expanded.value]
    .filter((path) => normalizeRelativePath(path) !== '')
    .sort((a, b) => a.split('/').length - b.split('/').length);
  for (const path of paths) {
    if (!isExpanded(path)) continue;
    const state = dirs[path] as PfSourceDirState | undefined;
    if (state?.loaded || state?.loading) continue;
    try {
      await loadDir(path);
    } catch {
      // 单个目录加载失败不影响整棵树；错误会挂在节点上
    }
  }
}

// ============================ 工作区 / 工程 ============================

function normalizeWorkspace(raw: unknown): PfWorkspace {
  const obj = (raw && typeof raw === 'object' ? raw : {}) as Record<string, unknown>;
  return {
    id: String(obj.id ?? obj.workspaceId ?? obj.workspace_id ?? ''),
    alias: String(obj.alias ?? obj.workspaceAlias ?? obj.name ?? ''),
    path: String(obj.path ?? obj.root ?? obj.directory ?? ''),
    enabled: obj.enabled !== false,
    ...(obj.status === undefined ? {} : { status: String(obj.status) }),
    ...(obj.current === true ? { current: true } : {}),
  };
}

function normalizeProject(raw: unknown, base?: PfProject): PfProject {
  const obj = (raw && typeof raw === 'object' ? raw : {}) as Record<string, unknown>;
  const rootInfo = (obj.rootInfo ?? {}) as Record<string, unknown>;
  const root = obj.root ?? rootInfo.root ?? base?.root ?? '';
  const project: PfProject = {
    id: String(obj.id ?? base?.id ?? ''),
    name: String(obj.name ?? base?.name ?? ''),
    root: String(root),
    status: String(obj.status ?? base?.status ?? 'active'),
  };
  const workspaceAlias = obj.workspace_alias ?? obj.workspaceAlias ?? obj.workspace ?? base?.workspace_alias;
  if (workspaceAlias !== undefined && workspaceAlias !== null) project.workspace_alias = String(workspaceAlias);
  const subpath = obj.subpath ?? base?.subpath;
  if (subpath !== undefined && subpath !== null) project.subpath = String(subpath);
  const updatedAt = obj.updated_at ?? obj.updatedAt ?? base?.updated_at;
  if (updatedAt !== undefined && updatedAt !== null) project.updated_at = String(updatedAt);
  const createdBy = obj.created_by ?? base?.created_by;
  if (createdBy !== undefined) project.created_by = createdBy as string | null;
  if (rootInfo.writable === false) project.rootInfo = { writable: false, root: String(root) };
  return project;
}

function extractArray(payload: unknown, keys: string[]): unknown[] | null {
  if (Array.isArray(payload)) return payload;
  if (!payload || typeof payload !== 'object') return null;
  const obj = payload as Record<string, unknown>;
  for (const key of keys) {
    if (Array.isArray(obj[key])) return obj[key] as unknown[];
  }
  return null;
}

function resolveProjectRoot(project: PfProject | null): string {
  const direct = String(project?.root ?? '').trim();
  if (direct) return direct.replace(/[\\/]+$/, '');
  const alias = String(project?.workspace_alias ?? '');
  const workspace = workspaces.value.find((item) => item.alias === alias || item.id === alias);
  if (workspace?.path) {
    const subpath = normalizeRelativePath(project?.subpath ?? '');
    return subpath ? joinProjectPath(workspace.path, subpath) : workspace.path.replace(/[\\/]+$/, '');
  }
  return '';
}

function resetTreeState(projectId: string): void {
  for (const key of Object.keys(dirs)) delete dirs[key];
  treeError.value = '';
  expanded.value = projectId ? readExpanded(projectId) : [];
}

async function loadWorkspaces(): Promise<void> {
  workspacesLoading.value = true;
  workspacesError.value = '';
  try {
    const payload = await pfInvoke<unknown>('ListWorkspaces');
    const raw = extractArray(payload, ['workspaces', 'items', 'list']) ?? [];
    workspaces.value = raw.map(normalizeWorkspace).filter((item) => item.alias || item.id);
    const saved = readStorage(SOURCE_WORKSPACE_STORAGE_KEY);
    const valid = (alias: string): boolean => workspaces.value.some((item) => item.alias === alias);
    if (!valid(selectedWorkspaceAlias.value)) {
      const fallback = workspaces.value.find((item) => item.enabled) ?? workspaces.value[0];
      selectedWorkspaceAlias.value = saved && valid(saved) ? saved : fallback?.alias ?? '';
    }
    if (selectedWorkspaceAlias.value) writeStorage(SOURCE_WORKSPACE_STORAGE_KEY, selectedWorkspaceAlias.value);
  } catch (error) {
    workspacesError.value = errorMessage(error);
    throw error;
  } finally {
    workspacesLoading.value = false;
  }
}

async function selectProject(projectId: string): Promise<void> {
  const id = String(projectId ?? '').trim();
  if (!id) return;
  const listed = projects.value.find((item) => item.id === id) ?? null;
  selectedProjectId.value = id;
  selectedProject.value = listed;
  projectError.value = '';
  projectLoading.value = true;
  try {
    let canonical: PfProject = listed ?? { id, name: id, root: '', status: 'active' };
    try {
      const payload = await pfInvoke<unknown>('GetProject', { projectId: id, timeline: 1 });
      const detail = (payload && typeof payload === 'object' ? payload : {}) as Record<string, unknown>;
      const rawProject = (detail.project ?? payload) as unknown;
      canonical = normalizeProject(rawProject, canonical);
      if (!canonical.id) canonical.id = id;
    } catch (error) {
      projectError.value = `读取工程根目录失败：${errorMessage(error)}`;
    }
    selectedProject.value = canonical;
    const root = resolveProjectRoot(canonical);
    if (!root) throw new Error('无法确定工程根目录：GetProject 未返回 root，且工作区路径不可用');
    projectRoot.value = root;
    writeStorage(SOURCE_PROJECT_STORAGE_KEY, id);
    resetTreeState(id);
    await loadDir(TREE_ROOT, { force: true });
    await restoreExpandedDirs();
  } catch (error) {
    projectRoot.value = '';
    treeError.value = errorMessage(error);
    throw error;
  } finally {
    projectLoading.value = false;
  }
}

async function loadProjects(): Promise<void> {
  projectsLoading.value = true;
  projectsError.value = '';
  try {
    const args: Record<string, unknown> = { limit: 100 };
    if (selectedWorkspaceAlias.value) args.workspace = selectedWorkspaceAlias.value;
    const payload = await pfInvoke<unknown>('ListProjects', args);
    const raw = extractArray(payload, ['projects', 'items', 'list']) ?? [];
    projects.value = raw.map((item) => normalizeProject(item)).filter((item) => item.id);
    const saved = readStorage(SOURCE_PROJECT_STORAGE_KEY);
    const valid = (projectId: string): boolean => projects.value.some((item) => item.id === projectId);
    let nextId = selectedProjectId.value;
    if (!nextId || !valid(nextId)) nextId = saved && valid(saved) ? saved : projects.value[0]?.id ?? '';
    if (!nextId) {
      selectedProjectId.value = '';
      selectedProject.value = null;
      projectRoot.value = '';
      resetTreeState('');
      projectsError.value = '该工作区下还没有工程，请先在桌面端创建工程';
      return;
    }
    await selectProject(nextId);
  } catch (error) {
    projectsError.value = errorMessage(error);
    throw error;
  } finally {
    projectsLoading.value = false;
  }
}

async function selectWorkspace(alias: string): Promise<void> {
  const next = String(alias ?? '').trim();
  if (next === selectedWorkspaceAlias.value) return;
  selectedWorkspaceAlias.value = next;
  writeStorage(SOURCE_WORKSPACE_STORAGE_KEY, next);
  selectedProjectId.value = '';
  selectedProject.value = null;
  projectRoot.value = '';
  projects.value = [];
  projectError.value = '';
  resetTreeState('');
  await loadProjects();
}

async function initSource(): Promise<void> {
  if (initialized.value) return;
  initialized.value = true;
  try {
    await loadWorkspaces();
    if (!workspaces.value.length) {
      workspacesError.value = '桌面端没有登记任何工作区，请先在桌面 V工程里添加工作区';
      return;
    }
    if (!selectedWorkspaceAlias.value) selectedWorkspaceAlias.value = workspaces.value[0]?.alias ?? '';
    await loadProjects();
  } catch {
    // 错误已写入 workspacesError / projectsError，页面展示即可
  }
}

// ============================ 目录树动作 ============================

async function toggleDir(path: string): Promise<void> {
  const target = normalizeRelativePath(path);
  if (!target) return;
  if (isExpanded(target)) {
    setExpanded(target, false);
    return;
  }
  setExpanded(target, true);
  try {
    await loadDir(target);
  } catch {
    // 节点自身显示错误，这里不再抛给页面
  }
}

async function refreshTree(options: { retryTransport?: boolean } = {}): Promise<void> {
  if (!projectRoot.value) return;
  if (options.retryTransport) {
    transport.value = 'unknown';
    transportNote.value = '';
  }
  for (const key of Object.keys(dirs)) delete dirs[key];
  treeError.value = '';
  try {
    await loadDir(TREE_ROOT, { force: true });
    await restoreExpandedDirs();
  } catch {
    // treeError 已写入
  }
}

async function revealPath(filePath: string): Promise<void> {
  const clean = normalizeRelativePath(filePath);
  if (!clean) return;
  const parts = clean.split('/');
  let current = '';
  for (let index = 0; index < parts.length - 1; index += 1) {
    current = current ? `${current}/${parts[index]}` : parts[index];
    setExpanded(current, true);
    if (!(dirs[current] as PfSourceDirState | undefined)?.loaded) {
      try {
        await loadDir(current);
      } catch {
        // 定位失败不阻断打开文件
      }
    }
  }
}

// ============================ 搜索 ============================

async function runSearch(query?: string): Promise<void> {
  const raw = String(query ?? searchQuery.value ?? '');
  searchQuery.value = raw;
  const tokens = splitSearchTokens(raw);
  if (!tokens.length) {
    clearSearch();
    return;
  }
  if (!projectRoot.value) {
    searchError.value = '请先选择工程';
    return;
  }
  searchLoading.value = true;
  searchError.value = '';
  searchNote.value = '';
  searchTransport.value = '';
  try {
    const { entries, via } = await searchByTransport(tokens);
    searchResults.value = entries;
    searchTotal.value = filterByTokens(entries, tokens).length;
    searchTransport.value = via;
  } catch (error) {
    searchResults.value = [];
    searchTotal.value = 0;
    searchError.value = errorMessage(error);
  } finally {
    searchLoading.value = false;
  }
}

function clearSearch(): void {
  searchQuery.value = '';
  searchResults.value = [];
  searchTotal.value = 0;
  searchError.value = '';
  searchNote.value = '';
  searchTransport.value = '';
  searchLoading.value = false;
}

// ============================ 读文件 ============================

async function readCodeRaw(path: string): Promise<ParsedReadCode> {
  const projectId = selectedProjectId.value;
  if (!projectId) throw new Error('请先选择工程');
  const envelope = await pfInvokeEnvelope('ReadCode', {
    projectId,
    path,
    showLine: false,
  });
  const parsed = parseReadCodeEnvelope(envelope);
  if (parsed.failed.length) throw new Error(`ReadCode 读取失败：${parsed.failed.join('；')}`);
  if (parsed.multimodal) throw new Error('该文件是图片 / 音视频等二进制内容，暂不支持在源码模式中查看');
  if (!parsed.rawText && !parsed.text) {
    const reason = parsed.skipped.length ? `：${parsed.skipped.join('；')}` : '';
    throw new Error(`ReadCode 没有返回文件正文${reason}`);
  }
  return parsed;
}

function buildFileState(path: string, parsed: ParsedReadCode): PfSourceFile {
  const lines = parsed.text.split('\n');
  const state: PfSourceFile = {
    path,
    text: parsed.text,
    rawText: parsed.rawText,
    lines,
    lineCount: lines.length,
    eol: parsed.eol,
    renderTruncated: lines.length > SOURCE_RENDER_LINE_LIMIT,
  };
  if (parsed.size !== undefined) state.size = parsed.size;
  return state;
}

async function openFile(path: string): Promise<PfSourceFile> {
  const clean = ensureRelativePath(path);
  fileLoading.value = true;
  fileError.value = '';
  try {
    const parsed = await readCodeRaw(clean);
    const file = buildFileState(clean, parsed);
    currentFile.value = file;
    outline.value = [];
    outlinePath.value = '';
    outlineError.value = '';
    await revealPath(clean);
    return file;
  } catch (error) {
    fileError.value = errorMessage(error);
    throw error;
  } finally {
    fileLoading.value = false;
  }
}

async function reloadCurrentFile(): Promise<void> {
  const file = currentFile.value;
  if (!file) return;
  try {
    const parsed = await readCodeRaw(file.path);
    currentFile.value = buildFileState(file.path, parsed);
  } catch (error) {
    fileError.value = errorMessage(error);
  }
}

function closeFile(): void {
  currentFile.value = null;
  fileError.value = '';
  outline.value = [];
  outlinePath.value = '';
  outlineError.value = '';
  clearFindSymbol();
  clearTrace();
}

// ============================ 写操作（强确认由组件负责，这里做参数校验 + 回读校验） ============================

function ensureWriteReason(reason: unknown): string {
  const text = String(reason ?? '').trim();
  if (!text) throw new Error('请填写本次改动原因（reason 必填，会写入桌面施工记录）');
  if (text.length > 500) throw new Error('改动原因不能超过 500 个字符');
  return text;
}

async function refreshDirAfterWrite(relDir: string): Promise<void> {
  try {
    await loadDir(relDir, { force: true });
  } catch {
    // 目录刷新失败不影响写后回读校验（校验以 ReadCode 为准）
  }
}

/** 保存当前打开文件（EditCode 整文件替换 + 回读校验）。 */
async function saveFile(draft: string, reason: string): Promise<PfSourceWriteResult> {
  const file = currentFile.value;
  if (!file) throw new Error('当前没有打开的文件');
  const projectId = selectedProjectId.value;
  if (!projectId) throw new Error('请先选择工程');
  const reasonText = ensureWriteReason(reason);
  const draftText = String(draft ?? '').replace(/\r\n/g, '\n');
  const newRaw = file.eol === 'CRLF' ? draftText.replace(/\n/g, '\r\n') : draftText;
  saving.value = true;
  actionError.value = '';
  lastWriteNotice.value = '';
  try {
    let response: unknown;
    try {
      response = await editWholeFile(projectId, file, newRaw, reasonText);
    } catch (primaryError) {
      const primaryMessage = errorMessage(primaryError);
      // target 精确替换失败（行号偏移 / target 歧义 / 多义票据）时退回行区间整文件替换
      try {
        response = await editByLineRange(projectId, file, newRaw, reasonText);
      } catch (fallbackError) {
        throw new Error(`保存失败：${primaryMessage}；行区间替换也失败：${errorMessage(fallbackError)}`);
      }
    }
    const after = await readCodeRaw(file.path);
    if (!sameFileText(after.text, newRaw)) {
      currentFile.value = buildFileState(file.path, after);
      throw new Error('回读校验失败：桌面端保存后的内容与编辑器不一致，已重新载入磁盘内容');
    }
    currentFile.value = buildFileState(file.path, after);
    const warnings = extractReviewWarnings(response);
    lastWriteNotice.value = warnings.length
      ? `已保存 ${file.path}（代码审查提示 ${warnings.length} 条）`
      : `已保存 ${file.path}`;
    await refreshDirAfterWrite(dirname(file.path));
    return { path: file.path, warnings };
  } catch (error) {
    actionError.value = errorMessage(error);
    throw error;
  } finally {
    saving.value = false;
  }
}

async function editWholeFile(
  projectId: string,
  file: PfSourceFile,
  content: string,
  reason: string,
): Promise<unknown> {
  if (!file.rawText) {
    return pfInvoke('EditCode', { projectId, path: file.path, reason, op: 'insert', after: 0, content });
  }
  return pfInvoke('EditCode', {
    projectId,
    path: file.path,
    reason,
    target: file.rawText,
    replace: content,
  });
}

async function editByLineRange(
  projectId: string,
  file: PfSourceFile,
  content: string,
  reason: string,
): Promise<unknown> {
  const lines = countContentLines(file.rawText);
  if (lines <= 0) {
    return pfInvoke('EditCode', { projectId, path: file.path, reason, op: 'insert', after: 0, content });
  }
  return pfInvoke('EditCode', { projectId, path: file.path, reason, start: 1, end: lines, content });
}

/** 新建文件（CreateFile + 回读校验）。 */
async function createFile(input: { path: string; content: string; reason: string }): Promise<PfSourceFile> {
  const projectId = selectedProjectId.value;
  if (!projectId) throw new Error('请先选择工程');
  const path = ensureRelativePath(input.path, '新文件路径');
  const reason = ensureWriteReason(input.reason);
  const content = String(input.content ?? '');
  actionBusy.value = true;
  actionError.value = '';
  lastWriteNotice.value = '';
  try {
    await pfInvoke('CreateFile', { projectId, path, content, reason });
    const after = await readCodeRaw(path);
    if (!sameFileText(after.text, content)) {
      throw new Error('回读校验失败：新建文件的内容与提交内容不一致');
    }
    const file = buildFileState(path, after);
    currentFile.value = file;
    await refreshDirAfterWrite(dirname(path));
    await revealPath(path);
    lastWriteNotice.value = `已新建 ${path}`;
    return file;
  } catch (error) {
    actionError.value = errorMessage(error);
    throw error;
  } finally {
    actionBusy.value = false;
  }
}

/** 删除文件（RemoveFile + 目录列表回读校验）。 */
async function removeFile(path: string, reason: string): Promise<void> {
  const projectId = selectedProjectId.value;
  if (!projectId) throw new Error('请先选择工程');
  const target = ensureRelativePath(path);
  const reasonText = ensureWriteReason(reason);
  actionBusy.value = true;
  actionError.value = '';
  lastWriteNotice.value = '';
  try {
    await pfInvoke('RemoveFile', { projectId, path: target, reason: reasonText });
    await refreshDirAfterWrite(dirname(target));
    const parentEntries = (dirs[dirname(target)] as PfSourceDirState | undefined)?.entries ?? [];
    const stillListed = parentEntries.some((entry) => entry.path === target);
    if (stillListed) throw new Error('回读校验失败：删除后文件仍出现在目录列表中');
    // 目录列表不可用时退回 ReadCode 校验（此时应当读取失败）
    if (!parentEntries.length) {
      let stillReadable = true;
      try {
        await readCodeRaw(target);
      } catch {
        stillReadable = false;
      }
      if (stillReadable) throw new Error('回读校验失败：删除后文件仍可读取');
    }
    if (currentFile.value?.path === target) closeFile();
    lastWriteNotice.value = `已删除 ${target}`;
  } catch (error) {
    actionError.value = errorMessage(error);
    throw error;
  } finally {
    actionBusy.value = false;
  }
}

/** 移动/重命名文件（MoveFile + 回读校验）。 */
async function moveFile(input: { from: string; to: string; reason: string }): Promise<PfSourceFile> {
  const projectId = selectedProjectId.value;
  if (!projectId) throw new Error('请先选择工程');
  const from = ensureRelativePath(input.from, '原文件路径');
  const to = ensureRelativePath(input.to, '目标文件路径');
  if (from === to) throw new Error('目标路径与原路径相同，无需移动');
  const reason = ensureWriteReason(input.reason);
  let expectedText = '';
  if (currentFile.value?.path === from) expectedText = currentFile.value.text;
  actionBusy.value = true;
  actionError.value = '';
  lastWriteNotice.value = '';
  try {
    await pfInvoke('MoveFile', { projectId, from, to, reason });
    const after = await readCodeRaw(to);
    if (expectedText && !sameFileText(after.text, expectedText)) {
      throw new Error('回读校验失败：移动后的文件内容与移动前不一致');
    }
    const file = buildFileState(to, after);
    if (currentFile.value?.path === from) currentFile.value = file;
    await refreshDirAfterWrite(dirname(from));
    if (dirname(to) !== dirname(from)) await refreshDirAfterWrite(dirname(to));
    await revealPath(to);
    lastWriteNotice.value = `已移动到 ${to}`;
    return file;
  } catch (error) {
    actionError.value = errorMessage(error);
    throw error;
  } finally {
    actionBusy.value = false;
  }
}

// ============================ Outline / FindSymbol / Trace ============================

function requireProjectId(): string {
  const projectId = selectedProjectId.value;
  if (!projectId) throw new Error('请先选择工程');
  return projectId;
}

async function loadOutline(path?: string): Promise<void> {
  const target = String(path ?? currentFile.value?.path ?? '').trim();
  if (!target) throw new Error('请先打开一个文件');
  outlineLoading.value = true;
  outlineError.value = '';
  try {
    const envelope = await pfInvokeEnvelope('Outline', {
      projectId: requireProjectId(),
      path: target,
      depth: 3,
    });
    outline.value = parseOutlineEnvelope(envelope);
    outlinePath.value = target;
  } catch (error) {
    outlineError.value = errorMessage(error);
    outline.value = [];
    throw error;
  } finally {
    outlineLoading.value = false;
  }
}

function clearOutline(): void {
  outline.value = [];
  outlinePath.value = '';
  outlineError.value = '';
}

async function findSymbol(query?: string): Promise<PfSourceSymbolHit[]> {
  const name = String(query ?? findSymbolQuery.value ?? '').trim();
  findSymbolQuery.value = name;
  if (!name) throw new Error('请输入要查找的符号名称');
  findSymbolLoading.value = true;
  findSymbolError.value = '';
  findSymbolNote.value = '';
  try {
    const envelope = await pfInvokeEnvelope('FindSymbol', {
      projectId: requireProjectId(),
      name,
      limit: 50,
    });
    const parsed = parseFindSymbolEnvelope(envelope);
    findSymbolHits.value = parsed.hits;
    findSymbolNote.value = parsed.note;
    return parsed.hits;
  } catch (error) {
    findSymbolError.value = errorMessage(error);
    findSymbolHits.value = [];
    throw error;
  } finally {
    findSymbolLoading.value = false;
  }
}

function clearFindSymbol(): void {
  findSymbolQuery.value = '';
  findSymbolHits.value = [];
  findSymbolNote.value = '';
  findSymbolError.value = '';
}

async function trace(input?: string): Promise<PfSourceTrace> {
  const target = String(input ?? traceTarget.value ?? '').trim();
  traceTarget.value = target;
  if (!target) {
    throw new Error('请输入要查询的链路目标（如 ipc:通道名 / global:名称 / page:xxx.html / file:相对路径）');
  }
  traceLoading.value = true;
  traceError.value = '';
  try {
    const envelope = await pfInvokeEnvelope('Trace', {
      projectId: requireProjectId(),
      target,
    });
    const result = parseTraceEnvelope(envelope, target);
    traceResult.value = result;
    return result;
  } catch (error) {
    traceError.value = errorMessage(error);
    traceResult.value = null;
    throw error;
  } finally {
    traceLoading.value = false;
  }
}

function clearTrace(): void {
  traceTarget.value = '';
  traceResult.value = null;
  traceError.value = '';
}

// ============================ 刷新 / 重置 ============================

async function refreshAll(): Promise<void> {
  clearSearch();
  actionError.value = '';
  try {
    await loadWorkspaces();
    if (workspaces.value.length) {
      await loadProjects();
      if (selectedProjectId.value) await refreshTree({ retryTransport: true });
    }
    if (currentFile.value?.path) await reloadCurrentFile();
    lastWriteNotice.value = '已刷新';
  } catch {
    // 错误已记录在对应状态里
  }
}

function clearActionError(): void {
  actionError.value = '';
}

/** 仅供单元测试重置模块级单例状态。 */
export function __resetProjectForgeSourceForTests(): void {
  workspaces.value = [];
  workspacesLoading.value = false;
  workspacesError.value = '';
  selectedWorkspaceAlias.value = '';
  projects.value = [];
  projectsLoading.value = false;
  projectsError.value = '';
  selectedProjectId.value = '';
  selectedProject.value = null;
  projectRoot.value = '';
  projectLoading.value = false;
  projectError.value = '';
  transport.value = 'unknown';
  transportNote.value = '';
  allowedRoots.value = null;
  allowedRootsFetchedAt = 0;
  allowedRootsPromise = null;
  for (const key of Object.keys(dirs)) delete dirs[key];
  expanded.value = [];
  treeError.value = '';
  clearSearch();
  currentFile.value = null;
  fileLoading.value = false;
  fileError.value = '';
  clearOutline();
  clearFindSymbol();
  clearTrace();
  saving.value = false;
  actionBusy.value = false;
  actionError.value = '';
  lastWriteNotice.value = '';
  initialized.value = false;
}

// ============================ 派生状态 ============================

const activeWorkspace = computed(
  () => workspaces.value.find((item) => item.alias === selectedWorkspaceAlias.value) ?? null,
);

const treeRows = computed(() => flattenTreeRows(dirs, expanded.value));
const treeLoading = computed(() => Boolean((dirs[TREE_ROOT] as PfSourceDirState | undefined)?.loading));
const treeLoaded = computed(() => Boolean((dirs[TREE_ROOT] as PfSourceDirState | undefined)?.loaded));
const dirLoading = computed(() => Object.keys(dirs).some((key) => Boolean(dirs[key]?.loading)));

const connectionLabel = computed(() => {
  if (transport.value === 'fileoperator') return '文件树：FileOperator';
  if (transport.value === 'pty') return '文件树：兼容模式（PTY）';
  return '文件树：未连接';
});

export function useProjectForgeSource() {
  return {
    // —— 工作区 / 工程 ——
    workspaces,
    workspacesLoading,
    workspacesError,
    selectedWorkspaceAlias,
    activeWorkspace,
    projects,
    projectsLoading,
    projectsError,
    selectedProjectId,
    selectedProject,
    projectRoot,
    projectLoading,
    projectError,
    loadWorkspaces,
    loadProjects,
    selectWorkspace,
    selectProject,
    initSource,
    refreshAll,
    // —— 文件树 ——
    transport,
    transportNote,
    connectionLabel,
    treeRows,
    treeLoading,
    treeLoaded,
    dirLoading,
    treeError,
    dirs,
    expanded,
    loadRoot: () => loadDir(TREE_ROOT, { force: true }),
    loadDir,
    toggleDir,
    refreshTree,
    revealPath,
    isExpanded,
    // —— 搜索 ——
    searchQuery,
    searchResults,
    searchTotal,
    searchLoading,
    searchError,
    searchTransport,
    searchNote,
    runSearch,
    clearSearch,
    // —— 文件读写 ——
    currentFile,
    fileLoading,
    fileError,
    openFile,
    reloadCurrentFile,
    closeFile,
    saveFile,
    createFile,
    removeFile,
    moveFile,
    // —— 符号 ——
    outline,
    outlineLoading,
    outlineError,
    outlinePath,
    loadOutline,
    clearOutline,
    findSymbolQuery,
    findSymbolHits,
    findSymbolNote,
    findSymbolLoading,
    findSymbolError,
    findSymbol,
    clearFindSymbol,
    traceTarget,
    traceResult,
    traceLoading,
    traceError,
    trace,
    clearTrace,
    // —— 写操作状态 ——
    saving,
    actionBusy,
    actionError,
    lastWriteNotice,
    clearActionError,
  };
}
