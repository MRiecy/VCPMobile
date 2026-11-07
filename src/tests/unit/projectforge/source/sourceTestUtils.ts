/**
 * T3 源码模式单测公共工具：bridge envelope 构造 + FileOperator/PTY mock 装置。
 *
 * 说明：`vi.mock('.../projectForgeApi')` 必须在各测试文件里声明（vi.mock 会被提升到文件顶部），
 * 本文件只提供 envelope 构造函数与可复用的 mock 装配，避免每个测试重复写 bridge 协议细节。
 */
import { vi } from 'vitest';
import { flushPromises } from '@vue/test-utils';

type AnyMock = ReturnType<typeof vi.fn>;

export const PROJECT_ROOT = '/home/user/proj/demo';
export const WORKSPACE = { id: 'ws1', alias: '工程区', path: '/home/user/proj', enabled: true };
export const PROJECT = {
  id: 'p1',
  name: '测试工程',
  workspace_alias: '工程区',
  root: PROJECT_ROOT,
  status: 'active',
};

export interface BridgeContentPart {
  type: string;
  text: string;
}

export interface MockEnvelope {
  ok: boolean;
  result: { content: BridgeContentPart[]; details: Record<string, unknown> };
}

/** 构造 bridge 成功 envelope（content 多段 + details）。 */
export function textEnvelope(parts: Array<string | { type?: string; text: string }>, details: Record<string, unknown> = {}): MockEnvelope {
  return {
    ok: true,
    result: {
      content: parts.map((part) =>
        typeof part === 'string'
          ? { type: 'text', text: part }
          : { type: part.type ?? 'text', text: part.text },
      ),
      details,
    },
  };
}

/** ReadCode 响应：content[0]=摘要，content[1]=桌面端真实格式（文件头 + 自适应 md 围栏 + 正文）。 */
export function readEnvelope(path: string, text: string, details: Record<string, unknown> = {}): MockEnvelope {
  const body = String(text ?? '');
  const longest = Math.max(3, ...[...body.matchAll(/`+/g)].map((match) => match[0].length + 1));
  const fence = '`'.repeat(longest);
  const normalized = body.replace(/\r\n/g, '\n').replace(/\r/g, '\n');
  const phantom = normalized.endsWith('\n') ? 1 : 0;
  const lineCount = normalized ? Math.max(0, normalized.split('\n').length - phantom) : 0;
  const header = `#### ${path} (${body.length} Bytes)\n- 总行数：${lineCount}`;
  const bodyPart = `${header}\n\n${fence}markdown\n${body}\n${fence}`;
  return textEnvelope(['已读取文件', bodyPart], {
    command: 'ReadCode',
    read: [{ path, kind: 'text', size: body.length }],
    skipped: [],
    failed: [],
    ...details,
  });
}

/** Outline 响应（details.files[].symbols）。 */
export function outlineEnvelope(
  path: string,
  symbols: Array<{ name: string; qualified?: string; kind?: string; start: number; end?: number; depth?: number }>,
): MockEnvelope {
  return textEnvelope(['符号大纲'], {
    command: 'Outline',
    files: [
      {
        path,
        symbols: symbols.map((symbol) => ({
          name: symbol.name,
          qualified: symbol.qualified ?? symbol.name,
          kind: symbol.kind,
          start: symbol.start,
          end: symbol.end ?? symbol.start,
          depth: symbol.depth ?? 0,
        })),
      },
    ],
  });
}

/** FindSymbol 响应（details.hits）。 */
export function findSymbolEnvelope(
  hits: Array<{ path: string; name: string; kind?: string; start?: number; end?: number; signature?: string }>,
  extra: Record<string, unknown> = {},
): MockEnvelope {
  return textEnvelope(['符号查找结果'], {
    command: 'FindSymbol',
    total: hits.length,
    scanned: 3,
    hits: hits.map((hit) => ({
      path: hit.path,
      name: hit.name,
      qualified: hit.name,
      kind: hit.kind,
      start: hit.start,
      end: hit.end,
      signature: hit.signature,
      inProject: true,
    })),
    ...extra,
  });
}

/** Trace 响应（正文在 content）。 */
export function traceEnvelope(text: string): MockEnvelope {
  return textEnvelope([text], { command: 'Trace', kind: 'ipc', found: true });
}

export interface SourceEntryJson {
  name: string;
  type: 'file' | 'dir';
  size?: number;
}

export interface SourceBridgeOptions {
  /** FileOperator 全部调用失败（触发 PTY 降级）。 */
  fileOperatorFails?: boolean;
  /** FileOperator 白名单不含工程根目录（模拟真实桌面 ALLOWED_DIRECTORIES 边界，应静默走 PTY）。 */
  fileOperatorNotAllowed?: boolean;
  /** 获取白名单本身失败（模拟插件未启用，未知时仍尝试 FileOperator）。 */
  fileOperatorRootsFail?: boolean;
  /** 使用真实 FileOperator 返回形态（details.items / details.results + details.content 摘要）。 */
  fileOperatorRealShape?: boolean;
  /** EditCode 返回成功但不真正落盘（模拟回读不一致）。 */
  skipApplyEdits?: boolean;
  /** EditCode 的 target 精确替换总是失败（触发行区间兜底）。 */
  failTargetReplace?: boolean;
  /** RemoveFile 后目录列表仍包含该文件（模拟删除回读校验失败）。 */
  keepDeletedListed?: boolean;
}

export interface SourceBridgeState {
  disk: Record<string, string>;
  dirs: Record<string, SourceEntryJson[]>;
  searches: string[];
  editCalls: Array<Record<string, unknown>>;
  createCalls: Array<Record<string, unknown>>;
  removeCalls: Array<Record<string, unknown>>;
  moveCalls: Array<Record<string, unknown>>;
  outlineSymbols: Array<{ name: string; qualified?: string; kind?: string; start: number; end?: number; depth?: number }>;
  findSymbolHits: Array<{ path: string; name: string; kind?: string; start?: number; end?: number; signature?: string }>;
  traceText: string;
  /** FileOperator 调用过的 command 序列（断言白名单外不应调用 ListDirectory）。 */
  fileOperatorCommands: string[];
}

function dirKeyFromAbs(absPath: unknown): string {
  const value = String(absPath ?? '');
  if (!value) return '';
  if (!value.startsWith(PROJECT_ROOT)) return '';
  return value.slice(PROJECT_ROOT.length).replace(/^\/+/, '');
}

function dirKeyOfPath(path: string): string {
  const index = path.lastIndexOf('/');
  return index > 0 ? path.slice(0, index) : '';
}

function addDirEntry(state: SourceBridgeState, path: string, type: 'file' | 'dir', size?: number): void {
  const key = dirKeyOfPath(path);
  const name = path.slice(path.lastIndexOf('/') + 1);
  const list = (state.dirs[key] ??= []);
  if (list.some((entry) => entry.name === name)) return;
  const entry: SourceEntryJson = { name, type };
  if (size !== undefined) entry.size = size;
  list.push(entry);
}

function removeDirEntry(state: SourceBridgeState, path: string): void {
  const key = dirKeyOfPath(path);
  const name = path.slice(path.lastIndexOf('/') + 1);
  const list = state.dirs[key];
  if (!list) return;
  state.dirs[key] = list.filter((entry) => entry.name !== name);
}

/**
 * 把 4 个 projectForgeApi 函数接到 mock bridge 上（ProjectForge + FileOperator + PTYShellExecutor）。
 * 返回可变 state，测试可断言调用记录或改造数据。
 */
export function installSourceBridge(
  api: { pfInvoke: AnyMock; pfInvokeEnvelope: AnyMock; pluginInvoke: AnyMock; ptyExec: AnyMock },
  options: SourceBridgeOptions = {},
): SourceBridgeState {
  const state: SourceBridgeState = {
    disk: {
      'README.md': '# 测试\n',
      'src/main.ts': 'export const a = 1;\n',
    },
    dirs: {
      '': [
        { name: 'src', type: 'dir' },
        { name: 'README.md', type: 'file', size: 6 },
      ],
      src: [{ name: 'main.ts', type: 'file', size: 20 }],
    },
    searches: ['src/main.ts'],
    editCalls: [],
    createCalls: [],
    removeCalls: [],
    moveCalls: [],
    outlineSymbols: [{ name: 'a', qualified: 'a', kind: 'variable', start: 1, end: 1, depth: 0 }],
    findSymbolHits: [
      { path: 'src/main.ts', name: 'a', kind: 'variable', start: 1, end: 1, signature: 'export const a = 1;' },
    ],
    traceText: 'ipc:demo:invoke → 主进程 handler demo:invoke（渲染端 1 处调用）',
    fileOperatorCommands: [],
  };

  api.pfInvoke.mockImplementation(async (command: string, args: Record<string, unknown> = {}) => {
    switch (command) {
      case 'ListWorkspaces':
        return { command, workspaces: [WORKSPACE] };
      case 'ListProjects':
        return { command, projects: [PROJECT] };
      case 'GetProject':
        return {
          command,
          project: { ...PROJECT, rootInfo: { root: PROJECT_ROOT, writable: true } },
          todos: [],
          stats: {},
        };
      case 'EditCode': {
        state.editCalls.push({ ...args });
        if (options.skipApplyEdits) return { status: 'ok', nodeId: 11 };
        const path = String(args.path ?? '');
        if (args.target !== undefined) {
          if (options.failTargetReplace) throw new Error('未找到匹配的 target，请改用行号或符号定位');
          if (state.disk[path] !== args.target) throw new Error('未找到匹配的 target，请改用行号或符号定位');
          state.disk[path] = String(args.replace ?? '');
        } else if (args.content !== undefined) {
          state.disk[path] = String(args.content);
        }
        return { status: 'ok', nodeId: 11, review: { issues: [{ message: '建议为 a 补充单元测试' }] } };
      }
      case 'CreateFile': {
        state.createCalls.push({ ...args });
        const path = String(args.path ?? '');
        state.disk[path] = String(args.content ?? '');
        addDirEntry(state, path, 'file', String(args.content ?? '').length);
        return { status: 'ok', nodeId: 12 };
      }
      case 'RemoveFile': {
        state.removeCalls.push({ ...args });
        const path = String(args.path ?? '');
        delete state.disk[path];
        if (!options.keepDeletedListed) removeDirEntry(state, path);
        return { status: 'ok', nodeId: 13 };
      }
      case 'MoveFile': {
        state.moveCalls.push({ ...args });
        const from = String(args.from ?? '');
        const to = String(args.to ?? '');
        if (state.disk[to] !== undefined) throw new Error('目标文件已存在');
        state.disk[to] = state.disk[from] ?? '';
        delete state.disk[from];
        removeDirEntry(state, from);
        addDirEntry(state, to, 'file', state.disk[to].length);
        return { status: 'ok', nodeId: 14 };
      }
      default:
        throw new Error(`unexpected ProjectForge command: ${command}`);
    }
  });

  api.pfInvokeEnvelope.mockImplementation(async (command: string, args: Record<string, unknown> = {}) => {
    if (command === 'ReadCode') {
      const path = String(args.path ?? '');
      const text = state.disk[path];
      if (text === undefined) throw new Error(`文件不存在：${path}`);
      return readEnvelope(path, text);
    }
    if (command === 'Outline') {
      return outlineEnvelope(String(args.path ?? ''), state.outlineSymbols);
    }
    if (command === 'FindSymbol') {
      return findSymbolEnvelope(state.findSymbolHits);
    }
    if (command === 'Trace') {
      return traceEnvelope(state.traceText);
    }
    throw new Error(`unexpected ProjectForge command: ${command}`);
  });

  api.pluginInvoke.mockImplementation(async (toolName: string, args: Record<string, unknown> = {}) => {
    if (toolName !== 'FileOperator') throw new Error(`unexpected tool: ${toolName}`);
    const command = String(args.command ?? '');
    state.fileOperatorCommands.push(command);
    if (command === 'ListAllowedDirectories') {
      if (options.fileOperatorRootsFail) throw new Error('FileOperator 插件未启用');
      return {
        command: 'ListAllowedDirectories',
        allowedRoots: options.fileOperatorNotAllowed ? ['/home/user/other-root'] : [PROJECT_ROOT],
      };
    }
    if (options.fileOperatorFails) throw new Error('FileOperator 插件未启用');
    if (args.command === 'ListDirectory') {
      const rel = dirKeyFromAbs(args.directoryPath);
      const items = (state.dirs[rel] ?? []).map((entry) => ({
        name: entry.name,
        path: rel ? `${rel}/${entry.name}` : entry.name,
        type: entry.type === 'dir' ? 'directory' : 'file',
        size: entry.size ?? null,
        sizeFormatted: entry.size !== undefined ? `${entry.size} Bytes` : null,
        lastModified: '2026-01-01T00:00:00.000Z',
        permissions: 0,
        isHidden: false,
      }));
      if (options.fileOperatorRealShape) {
        return {
          path: String(args.directoryPath ?? ''),
          items,
          totalItems: items.length,
          truncated: false,
          message: `### 目录内容 (${items.length} 个项目)`,
          content: [
            {
              type: 'text',
              text: `### 目录内容\n\n| 文件名称 | 类型 |\n|---|---|\n${items
                .map((item: { name: string; type: string }) => `| ${item.name} | ${item.type === 'directory' ? '📁 目录' : '📄 文件'} |`)
                .join('\n')}`,
            },
          ],
        };
      }
      return items;
    }
    if (args.command === 'SearchFiles') {
      const hits = state.searches.map((path) => ({
        path,
        name: path.slice(path.lastIndexOf('/') + 1),
        directory: '',
        type: 'file',
        size: 1,
        sizeFormatted: '1 Bytes',
        lastModified: '2026-01-01T00:00:00.000Z',
        relativePath: path,
      }));
      if (options.fileOperatorRealShape) {
        return {
          searchPath: String(args.searchPath ?? ''),
          pattern: String(args.pattern ?? ''),
          results: hits,
          totalResults: hits.length,
          content: [{ type: 'text', text: `### 搜索结果 (${hits.length} 个)` }],
        };
      }
      return state.searches.map((path) => ({ path, type: 'file' }));
    }
    throw new Error(`unexpected FileOperator command: ${String(args.command)}`);
  });

  api.ptyExec.mockImplementation(async (command: string) => {
    const isSearch = /find \. -type f/.test(command);
    if (isSearch) {
      const lines = state.searches.map((path) => `./${path}`).join('\n');
      return { ok: true, text: `__PF_SOURCE_BEGIN__\n${lines}\n__PF_SOURCE_END__`, exitCode: 0 };
    }
    const rel = /demo\/src\b/.test(command) ? 'src' : '';
    const lines = (state.dirs[rel] ?? [])
      .map((entry) => `${entry.type === 'dir' ? 'd' : 'f'}\t${entry.name}\t${entry.size ?? 0}`)
      .join('\n');
    return { ok: true, text: `__PF_SOURCE_BEGIN__\n${lines}\n__PF_SOURCE_END__`, exitCode: 0 };
  });

  return state;
}

/** 多次 flushPromise，等待 onMounted 里的异步链（workspaces → projects → GetProject → 列目录）落定。 */
export async function settle(times = 10): Promise<void> {
  for (let index = 0; index < times; index += 1) {
    await flushPromises();
  }
}
