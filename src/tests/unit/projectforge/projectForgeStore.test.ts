import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import {
  SIGNATURE_STORAGE_KEY,
  aggregateContributors,
  aggregateFiles,
  groupTimeline,
  normalizeBatchRef,
  normalizeHistoryRow,
  normalizeNodeRef,
  parseProjectProgress,
  statsFromHistory,
  useProjectForgeStore,
} from '../../../features/projectforge/projectForgeStore';
import { ProjectForgeError } from '../../../features/projectforge/projectForgeApi';
import {
  BRIDGE_STORAGE_KEY,
  DEFAULT_BRIDGE_TOKEN,
  LEGACY_DEFAULT_BASE,
  __resetBridgeConfigForTests,
  applyBridgeConfigFromAppSettings,
} from '../../../core/bridge/bridgeConfig';

// —— 与桌面 MobilePluginBridge 真实返回一致的响应构造 ——

interface FetchInit {
  method?: string;
  headers?: Record<string, string>;
  body?: string;
}

const fetchMock = vi.fn();

function httpResponse(body: unknown, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    text: async () => (typeof body === 'string' ? body : JSON.stringify(body)),
  } as unknown as Response;
}

function okResponse(details: any, text = '') {
  return httpResponse({
    ok: true,
    toolName: 'ProjectForge',
    requestId: 'test',
    elapsedMs: 1,
    result: { content: [{ type: 'text', text }], details },
  });
}

function okContentResponse(content: Array<{ type: string; text: string }>, details: any) {
  return httpResponse({
    ok: true,
    toolName: 'ProjectForge',
    requestId: 'test',
    elapsedMs: 1,
    result: { content, details },
  });
}

function parseBody(init: FetchInit) {
  return JSON.parse(String(init?.body || '{}'));
}

function sentCommands(): string[] {
  return fetchMock.mock.calls.map((call) => parseBody(call[1] as FetchInit).args?.command);
}

type Handler = (args: any) => Response | Promise<Response>;

function installBridge(handlers: Record<string, Handler>) {
  fetchMock.mockImplementation(async (url: string, init: FetchInit) => {
    const body = parseBody(init);
    const toolName = body.toolName;
    const args = body.args || {};
    // FileOperator / PTYShellExecutor 等插件响应
    if (toolName !== 'ProjectForge') {
      const pluginHandler = handlers[`${toolName}:${args.command || ''}`];
      if (pluginHandler) return pluginHandler(args);
      throw new Error(`unexpected plugin ${toolName} with args ${JSON.stringify(args)}`);
    }
    const handler = handlers[args.command];
    if (handler) return handler(args);
    throw new Error(`unexpected command ${args.command} (${url})`);
  });
}

const LIST_TEXT = [
  '## 工程列表（全部工作区，2 个，按最近更新排序，最多 50 个）',
  '| ID | 名称 | 工作区 | 状态 | Todo | 报告 | 更新 |',
  '|---|---|---|---|---|---|---|',
  '| `proj1` | 示例工程 A（demo-workspace） | ai打工人 | review | 3/8 | 有 | 2026-10-29 19:19 |',
  '| `proj2` | 示例工程·自主赚钱·主工程 | deepseek | active | 0/7 | - | 2026-10-29 11:40 |',
].join('\n');

const PROJECTS = [
  {
    id: 'proj1',
    name: '示例工程 A（demo-workspace）',
    workspace_id: 'ws_b4e321dc8933',
    workspace_alias: 'ai打工人',
    root: '/home/user/示例工程/demo-workspace',
    subpath: 'demo-workspace',
    status: 'review',
    report: '# 验收报告\n\n## 结论\n上传通道已修复。',
    created_by: 'DeepSeek',
    created_at: '2026-10-29T04:59:48.321Z',
    updated_at: '2026-10-29T11:19:14.830Z',
    deleted_at: null,
  },
  {
    id: 'proj2',
    name: '示例工程·自主赚钱·主工程',
    workspace_id: 'ws_325cce8ae4f4',
    workspace_alias: 'deepseek',
    root: '/app/VCPToolBox/workspaces/agent/DeepSeek/示例工程主工程',
    subpath: '示例工程主工程',
    status: 'active',
    report: null,
    created_at: '2026-10-29T03:40:47.110Z',
    updated_at: '2026-10-29T03:40:47.110Z',
    deleted_at: null,
  },
];

const BASE_TODOS = [
  { id: 21, project_id: 'proj1', seq: 1, title: '修复上传通道', status: 'done', note: null, updated_by: null, updated_at: '2026-10-29T11:19:14.829Z' },
  { id: 22, project_id: 'proj1', seq: 2, title: '核对 state.json 与浮窗待办', status: 'pending', note: '等待登录态恢复', updated_by: null, updated_at: '2026-10-29T04:59:48.322Z' },
  { id: 23, project_id: 'proj1', seq: 3, title: '实测 --click 通道', status: 'doing', note: null, updated_by: 'DeepSeek', updated_at: '2026-10-29T10:58:08.748Z' },
];

const STATS = {
  nodeCount: 14,
  fileCount: 7,
  added: 846,
  removed: 73,
  lastNodeId: 21,
  lastAt: '2026-10-29T11:18:58.496Z',
};

const HISTORY_NODES = [
  {
    id: 21,
    batch_id: 21,
    file_path: 'tools/video.py',
    op: 'edit',
    batch_kind: 'edit',
    effective_reason: '非登录 shell 补充说明',
    added: 7,
    removed: 0,
    created_at: '2026-10-29T11:18:58.496Z',
    maid: null,
  },
  {
    id: 20,
    batch_id: 20,
    file_path: 'tools/compose.py',
    op: 'edit',
    batch_kind: 'edit',
    effective_reason: '补充说明',
    added: 7,
    removed: 0,
    created_at: '2026-10-29T11:18:55.312Z',
    maid: null,
  },
  {
    id: 19,
    batch_id: 20,
    file_path: 'tools/storyboard.py',
    op: 'edit',
    batch_kind: 'edit',
    effective_reason: '补充说明',
    added: 7,
    removed: 0,
    created_at: '2026-10-29T11:18:55.312Z',
    maid: 'DeepSeek',
  },
];

function projectDetails(todos: any[] = BASE_TODOS, project: any = PROJECTS[0], stats: any = STATS) {
  return { command: 'GetProject', project, todos, stats };
}

function detailHandlers(overrides: Record<string, Handler> = {}): Record<string, Handler> {
  return {
    GetProject: () => okResponse(projectDetails()),
    SearchHistory: () => okResponse({ command: 'SearchHistory', count: HISTORY_NODES.length, nodes: HISTORY_NODES }),
    ListProjects: () => okResponse({ command: 'ListProjects', count: 2, limit: 50, projects: PROJECTS }, LIST_TEXT),
    ListWorkspaces: () =>
      okResponse({
        command: 'ListWorkspaces',
        workspaces: [
          { id: 'ws_b4e321dc8933', alias: 'ai打工人', path: '/home/user/示例工程', enabled: true, status: 'ready' },
          { id: 'ws_disabled', alias: '停用区', path: '/x', enabled: false, status: 'disabled' },
        ],
      }),
    ...overrides,
  };
}

function useStore() {
  return useProjectForgeStore();
}

beforeEach(() => {
  localStorage.clear();
  __resetBridgeConfigForTests();
  setActivePinia(createPinia());
  fetchMock.mockReset();
  vi.stubGlobal('fetch', fetchMock);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

// ============================ 纯函数 ============================

describe('parseProjectProgress', () => {
  it('从列表 Markdown 表格解析 todo 进度与报告标记', () => {
    const parsed = parseProjectProgress(LIST_TEXT);
    expect(parsed.get('proj1')).toEqual({ done: 3, total: 8, hasReport: true });
    expect(parsed.get('proj2')).toEqual({ done: 0, total: 7, hasReport: false });
  });
});

describe('groupTimeline / 聚合纯函数', () => {
  it('按 batch_id 聚合为 PfBatchSummary：files 为路径数组、倒序、补齐 reason/maid', () => {
    const timeline = groupTimeline(HISTORY_NODES);
    expect(timeline.map((item) => item.batchKey)).toEqual(['b21', 'b20']);
    expect(timeline.map((item) => item.id)).toEqual([21, 20]);
    expect(timeline[1].files).toEqual(['tools/compose.py', 'tools/storyboard.py']);
    expect(timeline[1].node_count).toBe(2);
    expect(timeline[1].added).toBe(14);
    expect(timeline[1].reason).toBe('补充说明');
    expect(timeline[1].maid).toBe('DeepSeek');
    expect(timeline[1].at).toBe('2026-10-29T11:18:55.312Z');
  });

  it('忽略没有 batch_id 的脏节点，limit<=0 返回全部', () => {
    expect(groupTimeline([{ id: 1, file_path: 'x' } as any])).toEqual([]);
    expect(groupTimeline(HISTORY_NODES, 0)).toHaveLength(2);
    expect(groupTimeline(HISTORY_NODES, 1)).toHaveLength(1);
  });

  it('aggregateFiles 按文件聚合并按最近节点倒序', () => {
    const files = aggregateFiles(HISTORY_NODES);
    expect(files.map((item) => item.file_path)).toEqual([
      'tools/video.py',
      'tools/compose.py',
      'tools/storyboard.py',
    ]);
    expect(files[0]).toMatchObject({ edits: 1, added: 7, removed: 0, last_node: 21 });
  });

  it('aggregateContributors 按 maid 聚合（null 归入外部/未署名），batches 去重', () => {
    const contributors = aggregateContributors(HISTORY_NODES);
    const anonymous = contributors.find((item) => item.maid === null)!;
    const deepseek = contributors.find((item) => item.maid === 'DeepSeek')!;
    expect(anonymous).toMatchObject({ batches: 2, added: 14, removed: 0 });
    expect(anonymous.last_at).toBe('2026-10-29T11:18:58.496Z');
    expect(deepseek).toMatchObject({ batches: 1, added: 7 });
  });

  it('statsFromHistory 用历史行兜底统计', () => {
    const stats = statsFromHistory(HISTORY_NODES);
    expect(stats).toMatchObject({ nodeCount: 3, fileCount: 3, added: 21, removed: 0, lastNodeId: 21 });
    expect(stats.lastAt).toBe('2026-10-29T11:18:58.496Z');
  });

  it('normalizeHistoryRow 补默认值', () => {
    const row = normalizeHistoryRow({ id: '5', batch_id: '3' });
    expect(row).toMatchObject({ id: 5, batch_id: 3, op: '', file_path: '', maid: null, added: 0, removed: 0 });
  });

  it('normalizeNodeRef / normalizeBatchRef 接受数字与 nX/bX/last，非法引用报 validation', () => {
    expect(normalizeNodeRef(12)).toBe('n12');
    expect(normalizeNodeRef('N12')).toBe('n12');
    expect(normalizeBatchRef(21)).toBe('b21');
    expect(normalizeBatchRef('b21')).toBe('b21');
    expect(normalizeBatchRef('last')).toBe('last');
    expect(() => normalizeNodeRef('x')).toThrow(ProjectForgeError);
    expect(() => normalizeBatchRef(0)).toThrow(/批次引用无效/);
  });
});

// ============================ 工作区 / 工程列表 ============================

describe('工程列表', () => {
  it('bootstrap 拉工作区 + 工程列表，拼接 todo 进度与报告标记', async () => {
    installBridge(detailHandlers());
    const store = useStore();
    await store.bootstrap();

    expect(store.projects).toHaveLength(2);
    expect(store.projects[0]).toMatchObject({
      id: 'proj1',
      status: 'review',
      todoDone: 3,
      todoTotal: 8,
      hasReport: true,
    });
    expect(store.projects[1]).toMatchObject({ id: 'proj2', todoDone: 0, todoTotal: 7, hasReport: false });
    expect(store.enabledWorkspaces.map((item) => item.alias)).toEqual(['ai打工人']);
    expect(store.connection.state).toBe('ok');
    expect(store.connectionMessage).toContain('已连接');
    expect(store.listError).toBe('');
    expect(store.loading.projects).toBe(false);

    const listCall = fetchMock.mock.calls.find((call) => parseBody(call[1] as FetchInit).args.command === 'ListProjects')!;
    const body = parseBody(listCall[1] as FetchInit);
    expect(body.toolName).toBe('ProjectForge');
    expect(body.args).toMatchObject({ command: 'ListProjects', workspace: 'all', limit: 50, includeDeleted: false });
    expect((listCall[1] as FetchInit).headers?.Authorization).toBe(`Bearer ${store.token}`);
  });

  it('搜索词/工作区/含已删除开关进入 ListProjects 参数', async () => {
    installBridge(detailHandlers({ ListProjects: () => okResponse({ command: 'ListProjects', projects: [] }, '') }));
    const store = useStore();

    store.query = '  proj1  ';
    store.projectQuery = 'proj1';
    store.workspaceFilter = 'ai打工人';
    store.includeDeleted = true;
    await store.loadProjects();

    const body = parseBody(fetchMock.mock.calls[0][1] as FetchInit);
    expect(body.args).toMatchObject({
      command: 'ListProjects',
      query: 'proj1',
      workspace: 'ai打工人',
      includeDeleted: true,
    });
    expect(store.query).toBe('proj1');
    expect(store.projectQuery).toBe('proj1');
  });

  it('详情/列表里的 progress/stats 字段优先于 Markdown 解析', async () => {
    installBridge(
      detailHandlers({
        ListProjects: () =>
          okResponse({
            command: 'ListProjects',
            projects: [
              {
                id: 'pnew',
                name: '新工程',
                root: '/x',
                status: 'active',
                progress: { done: 2, total: 5, text: '2/5' },
                stats: { nodeCount: 3, fileCount: 1, added: 9, removed: 2 },
                hasReport: true,
              },
            ],
          }),
      }),
    );
    const store = useStore();
    await store.loadProjects();
    expect(store.projects[0]).toMatchObject({ todoDone: 2, todoTotal: 5, hasReport: true });
    expect(store.projects[0].stats?.nodeCount).toBe(3);
  });

  it('并发 loadProjects 防重：只发一次请求', async () => {
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    fetchMock.mockImplementation(async () => {
      await gate;
      return okResponse({ command: 'ListProjects', projects: PROJECTS }, LIST_TEXT);
    });
    const store = useStore();
    const first = store.loadProjects();
    const second = store.loadProjects();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    release();
    const [a, b] = await Promise.all([first, second]);
    expect(a).toHaveLength(2);
    expect(b).toHaveLength(2);
    expect(store.loading.projects).toBe(false);
  });

  it('searchProjects 在有工作区时走 SearchProjects，无工作区退回 ListProjects', async () => {
    installBridge(
      detailHandlers({
        SearchProjects: () => okResponse({ command: 'SearchProjects', projects: [PROJECTS[1]] }),
      }),
    );
    const store = useStore();
    const result = await store.searchProjects('示例工程', { workspace: 'deepseek' });
    expect(result).toHaveLength(1);
    expect(result[0].id).toBe('proj2');
    expect(parseBody(fetchMock.mock.calls[0][1] as FetchInit).args).toMatchObject({
      command: 'SearchProjects',
      workspace: 'deepseek',
      query: '示例工程',
    });

    fetchMock.mockClear();
    await store.searchProjects('proj1', { workspace: 'all' });
    expect(parseBody(fetchMock.mock.calls[0][1] as FetchInit).args).toMatchObject({
      command: 'ListProjects',
      workspace: 'all',
      query: 'proj1',
    });
  });

  it('searchProjects 空关键字本地拦下', async () => {
    const store = useStore();
    await expect(store.searchProjects('   ')).rejects.toThrow(/请输入工程名称关键字/);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

// ============================ 工程详情聚合 ============================

describe('工程详情（GetProject + SearchHistory 聚合）', () => {
  it('聚合成 todo / 统计 / timeline / files / contributors', async () => {
    installBridge(detailHandlers());
    const store = useStore();
    await store.openProject('proj1');

    expect(store.currentProjectId).toBe('proj1');
    expect(store.currentProject?.id).toBe('proj1');
    expect(store.detail?.project.name).toContain('示例工程');
    expect(store.detail?.todos).toHaveLength(3);
    expect(store.todoProgress).toEqual({ done: 1, total: 3, doing: 1, percent: 33 });
    expect(store.detail?.stats).toMatchObject({ nodeCount: 14, fileCount: 7, added: 846, removed: 73 });
    expect(store.detail?.timeline.map((item) => item.id)).toEqual([21, 20]);
    expect(store.detail?.files.map((item) => item.file_path)).toContain('tools/video.py');
    expect(store.detail?.contributors).toHaveLength(2);
    expect(store.detailError).toBe('');
    expect(store.error).toBe('');

    const getProjectCall = fetchMock.mock.calls.find((call) => parseBody(call[1] as FetchInit).args.command === 'GetProject')!;
    expect(parseBody(getProjectCall[1] as FetchInit).args).toMatchObject({ command: 'GetProject', projectId: 'proj1' });
    const historyCall = fetchMock.mock.calls.find((call) => parseBody(call[1] as FetchInit).args.command === 'SearchHistory')!;
    expect(parseBody(historyCall[1] as FetchInit).args).toMatchObject({ projectId: 'proj1', limit: 300 });
  });

  it('SearchHistory 失败时详情主体仍然可用、timeline/files/contributors 为空', async () => {
    installBridge(
      detailHandlers({
        SearchHistory: () => httpResponse({ ok: false, error: { message: 'boom' } }, 502),
      }),
    );
    const store = useStore();
    await store.openProject('proj1');
    expect(store.detail?.todos).toHaveLength(3);
    expect(store.detail?.timeline).toEqual([]);
    expect(store.detail?.files).toEqual([]);
    expect(store.detail?.contributors).toEqual([]);
    expect(store.detailError).toBe('');
  });

  it('stats 缺失或为 0 且历史非空时用历史聚合兜底', async () => {
    installBridge(
      detailHandlers({
        GetProject: () => okResponse(projectDetails(BASE_TODOS, PROJECTS[0], { nodeCount: 0, fileCount: 0, added: 0, removed: 0 })),
      }),
    );
    const store = useStore();
    await store.openProject('proj1');
    expect(store.detail?.stats).toMatchObject({ nodeCount: 3, fileCount: 3, added: 21 });
  });

  it('重复打开先展示缓存并后台刷新；force 与普通刷新都会重读 GetProject', async () => {
    installBridge(detailHandlers());
    const store = useStore();
    await store.openProject('proj1');
    // GetProject + SearchHistory
    expect(fetchMock.mock.calls.length).toBe(2);

    // 命中详情缓存只用于「先行展示」，仍会后台刷新，避免多端改动不新鲜
    await store.openProject('proj1');
    expect(fetchMock.mock.calls.length).toBeGreaterThan(2);
    expect(sentCommands().filter((command) => command === 'GetProject')).toHaveLength(2);

    await store.openProject('proj1', { force: true });
    expect(sentCommands().filter((command) => command === 'GetProject')).toHaveLength(3);
  });

  it('closeProject 清空详情与当前工程；currentProject 回退到列表行', async () => {
    installBridge(detailHandlers());
    const store = useStore();
    await store.bootstrap({ force: true });
    await store.openProject('proj1');
    expect(store.currentProject?.id).toBe('proj1');

    store.closeProject();
    expect(store.detail).toBeNull();
    expect(store.currentProjectId).toBeNull();
    expect(store.currentProject).toBeNull();
  });

  it('GetProject 502 时记录 bridge 错误并清空详情', async () => {
    installBridge(
      detailHandlers({
        GetProject: () =>
          httpResponse(
            { ok: false, error: { code: 'BRIDGE_INVOKE_FAILED', message: '[ProjectForge] 缺少 projectId。可用 ListProjects 查询已有工程。' } },
            502,
          ),
      }),
    );
    const store = useStore();
    await expect(store.openProject('proj1')).rejects.toThrow(/缺少 projectId/);
    expect(store.detailErrorKind).toBe('bridge');
    expect(store.detailError).toContain('缺少 projectId');
    expect(store.detail).toBeNull();
    expect(store.errorKind).toBe('bridge');
  });
});

// ============================ Todo 写入与回读确认 ============================

describe('Todo 写入与回读确认', () => {
  function installTodoBridge(initialTodos = BASE_TODOS) {
    let todos = JSON.parse(JSON.stringify(initialTodos));
    installBridge({
      GetProject: () => okResponse(projectDetails(todos)),
      SearchHistory: () => okResponse({ command: 'SearchHistory', nodes: [] }),
      UpdateTodos: (args) => {
        const doneSeq = args.done ? Number(args.done) : 0;
        const pendingSeq = args.pending ? Number(args.pending) : 0;
        const doingSeq = args.doing ? Number(args.doing) : 0;
        const blockedSeq = args.blocked ? Number(args.blocked) : 0;
        const removeSeq = args.remove ? Number(args.remove) : 0;
        if (args.add) {
          const titles = String(args.add).split('\n').filter(Boolean);
          const nextSeq = todos.reduce((max: number, todo: any) => Math.max(max, todo.seq), 0) + 1;
          titles.forEach((title: string, index: number) => {
            todos.push({ id: 100 + nextSeq + index, project_id: 'proj1', seq: nextSeq + index, title, status: 'pending', note: null });
          });
        }
        if (args.seq !== undefined) {
          todos = todos.map((todo: any) => {
            if (todo.seq !== Number(args.seq)) return todo;
            return {
              ...todo,
              ...(args.title !== undefined ? { title: String(args.title) } : {}),
              ...(args.note !== undefined ? { note: String(args.note) } : {}),
            };
          });
        }
        todos = todos.filter((todo: any) => !removeSeq || todo.seq !== removeSeq);
        todos = todos.map((todo: any) => {
          if (doneSeq && todo.seq === doneSeq) return { ...todo, status: 'done' };
          if (pendingSeq && todo.seq === pendingSeq) return { ...todo, status: 'pending' };
          if (doingSeq && todo.seq === doingSeq) return { ...todo, status: 'doing' };
          if (blockedSeq && todo.seq === blockedSeq) return { ...todo, status: 'blocked' };
          return todo;
        });
        return okResponse({ command: 'UpdateTodos', todos });
      },
    });
    return () => todos;
  }

  it('勾选完成 = UpdateTodos(done) 后回读 GetProject 确认', async () => {
    const readTodos = installTodoBridge();
    const store = useStore();
    await store.openProject('proj1');
    const pending = store.detail!.todos.find((todo) => todo.status === 'pending')!;

    await store.toggleTodo(pending);

    expect(readTodos().find((todo: any) => todo.seq === pending.seq).status).toBe('done');
    expect(store.detail!.todos.find((todo) => todo.seq === pending.seq)!.status).toBe('done');
    expect(store.mutating).toBe(false);

    const commands = sentCommands();
    const updateIndex = commands.indexOf('UpdateTodos');
    expect(updateIndex).toBeGreaterThan(-1);
    expect(commands.slice(updateIndex + 1)).toContain('GetProject');
    const updateCall = fetchMock.mock.calls[updateIndex][1] as FetchInit;
    expect(parseBody(updateCall).args).toMatchObject({ command: 'UpdateTodos', projectId: 'proj1', done: String(pending.seq) });
  });

  it('标记进行中 = UpdateTodos(doing) 后回读确认', async () => {
    installTodoBridge();
    const store = useStore();
    await store.openProject('proj1');
    const pending = store.detail!.todos.find((todo) => todo.status === 'pending')!;

    await store.markTodoDoing(pending);

    expect(store.detail!.todos.find((todo) => todo.seq === pending.seq)!.status).toBe('doing');
    const updateCall = fetchMock.mock.calls.find((call) => parseBody(call[1] as FetchInit).args.command === 'UpdateTodos')!;
    expect(parseBody(updateCall[1] as FetchInit).args).toMatchObject({ doing: String(pending.seq) });
  });

  it('新增待办 = UpdateTodos(add) 后回读确认标题出现', async () => {
    installTodoBridge();
    const store = useStore();
    await store.openProject('proj1');

    await store.addTodos('手机端新增待办 A\n手机端新增待办 B');

    expect(store.detail!.todos.map((todo) => todo.title)).toContain('手机端新增待办 A');
    expect(store.detail!.todos.map((todo) => todo.title)).toContain('手机端新增待办 B');
    const updateCall = fetchMock.mock.calls.find((call) => parseBody(call[1] as FetchInit).args.command === 'UpdateTodos')!;
    expect(parseBody(updateCall[1] as FetchInit).args.add).toBe('手机端新增待办 A\n手机端新增待办 B');
  });

  it('通用 updateTodos 透传 seq + title/note 并回读确认', async () => {
    installTodoBridge();
    const store = useStore();
    await store.openProject('proj1');

    await store.updateTodos({ seq: '2', title: '新的标题', note: '新的备注' });

    const todo = store.detail!.todos.find((item) => item.seq === 2)!;
    expect(todo.title).toBe('新的标题');
    expect(todo.note).toBe('新的备注');
    const updateCall = fetchMock.mock.calls.find((call) => parseBody(call[1] as FetchInit).args.command === 'UpdateTodos')!;
    expect(parseBody(updateCall[1] as FetchInit).args).toMatchObject({ seq: '2', title: '新的标题', note: '新的备注' });
  });

  it('removeTodo / setTodoStatus 走 UpdateTodos 并回读校验', async () => {
    installTodoBridge();
    const store = useStore();
    await store.openProject('proj1');

    await store.setTodoStatus(2, 'blocked');
    expect(store.detail!.todos.find((todo) => todo.seq === 2)!.status).toBe('blocked');

    await store.removeTodo(3);
    expect(store.detail!.todos.some((todo) => todo.seq === 3)).toBe(false);
  });

  it('setTodoStatus 非法状态本地拦下', async () => {
    const store = useStore();
    expect(() => store.setTodoStatus(1, 'unknown' as any)).toThrow(/待办状态无效/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('桌面端未真正写入时回读校验会报错（protocol）', async () => {
    installBridge({
      GetProject: () => okResponse(projectDetails()),
      SearchHistory: () => okResponse({ command: 'SearchHistory', nodes: [] }),
      UpdateTodos: () => okResponse({ command: 'UpdateTodos', todos: BASE_TODOS }),
    });
    const store = useStore();
    await store.openProject('proj1');
    const pending = store.detail!.todos.find((todo) => todo.status === 'pending')!;

    await expect(store.toggleTodo(pending)).rejects.toThrow(/回读校验失败/);
    expect(store.mutating).toBe(false);
    expect(store.errorKind).toBe('protocol');
  });

  it('新增空待办在本地就被拦下（不发请求）', async () => {
    const store = useStore();
    await expect(store.addTodos('   \n  ')).rejects.toThrow(/请输入待办内容/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('并发写操作防重：进行中的写操作会让第二次调用直接拒绝', async () => {
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    let updateCount = 0;
    installBridge({
      GetProject: () => okResponse(projectDetails()),
      SearchHistory: () => okResponse({ command: 'SearchHistory', nodes: [] }),
      UpdateTodos: async () => {
        updateCount += 1;
        await gate;
        return okResponse({ command: 'UpdateTodos', todos: BASE_TODOS });
      },
    });
    const store = useStore();
    await store.openProject('proj1');

    const first = store.updateTodos({ add: '第一个' });
    await new Promise((resolve) => setTimeout(resolve, 0));
    await expect(store.updateTodos({ add: '第二个' })).rejects.toThrow(/尚未完成/);
    release();
    const resolved = await first;
    expect(resolved.todos).toBeDefined();
    expect(updateCount).toBe(1);
  });
});

// ============================ 新建工程 ============================

describe('新建工程', () => {
  it('CreateProject 成功后刷新列表并进入详情', async () => {
    installBridge(
      detailHandlers({
        CreateProject: () =>
          okResponse(
            {
              command: 'CreateProject',
              project: {
                id: 'ptest1',
                name: 'VCPMobile测试临时工程',
                workspace_id: 'ws_325cce8ae4f4',
                workspace_alias: 'deepseek',
                root: '/app/VCPToolBox/workspaces/agent/DeepSeek',
                subpath: '',
                status: 'active',
                report: null,
                created_at: '2026-10-29T12:27:14.929Z',
                updated_at: '2026-10-29T12:27:14.929Z',
              },
              todos: [],
            },
            '## 工程已创建',
          ),
        GetProject: () =>
          okResponse({
            command: 'GetProject',
            project: { id: 'ptest1', name: 'VCPMobile测试临时工程', workspace_alias: 'deepseek', root: '/x', status: 'active', report: null },
            todos: [{ id: 1, seq: 1, title: '第一项', status: 'pending' }],
            stats: { nodeCount: 0, fileCount: 0, added: 0, removed: 0 },
          }),
      }),
    );

    const store = useStore();
    const project = await store.createProject({
      name: '  VCPMobile测试临时工程  ',
      workspace: 'deepseek',
      todos: '第一项\n第二项',
    });

    expect(project.id).toBe('ptest1');
    expect(store.detail?.project.id).toBe('ptest1');
    expect(store.currentProjectId).toBe('ptest1');

    const createCall = fetchMock.mock.calls.find((call) => parseBody(call[1] as FetchInit).args.command === 'CreateProject')!;
    expect(parseBody(createCall[1] as FetchInit).args).toMatchObject({
      command: 'CreateProject',
      name: 'VCPMobile测试临时工程',
      workspace: 'deepseek',
      todos: '第一项\n第二项',
    });
    expect(sentCommands()).toContain('ListProjects');
    expect(sentCommands()).toContain('GetProject');
  });

  it('名称为空时不发请求', async () => {
    const store = useStore();
    await expect(store.createProject({ name: '   ' })).rejects.toThrow(/请填写工程名称/);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

// ============================ 删除 / 恢复 / 永久清除 ============================

describe('删除 / 恢复 / 永久清除', () => {
  it('deleteProjects 传 projectIds 与署名，删除当前工程后关闭详情', async () => {
    installBridge(
      detailHandlers({
        DeleteProjects: () => okResponse({ command: 'DeleteProjects', deleted: ['proj1'], skipped: [] }),
      }),
    );
    const store = useStore();
    store.setSignature('清理员');
    await store.openProject('proj1');

    const result = await store.deleteProjects(['proj1']);
    expect(result).toEqual({ deleted: ['proj1'], skipped: [] });
    expect(store.detail).toBeNull();
    expect(store.currentProjectId).toBeNull();

    const deleteCall = fetchMock.mock.calls.find((call) => parseBody(call[1] as FetchInit).args.command === 'DeleteProjects')!;
    expect(parseBody(deleteCall[1] as FetchInit).args).toMatchObject({
      command: 'DeleteProjects',
      projectIds: 'proj1',
      maid: '清理员',
    });
  });

  it('deleteProjects 支持逗号/换行分隔的字符串 id，并去重', async () => {
    installBridge(detailHandlers({ DeleteProjects: () => okResponse({ command: 'DeleteProjects', deleted: ['a', 'b'] }) }));
    const store = useStore();
    store.setSignature('清理员');
    const result = await store.deleteProjects('a, b\na');
    expect(result.deleted).toEqual(['a', 'b']);
    const deleteCall = fetchMock.mock.calls.find((call) => parseBody(call[1] as FetchInit).args.command === 'DeleteProjects')!;
    expect(parseBody(deleteCall[1] as FetchInit).args.projectIds).toBe('a,b');
  });

  it('空 id 列表本地拦下', async () => {
    const store = useStore();
    await expect(store.deleteProjects([])).rejects.toThrow(/请选择要删除的工程/);
    await expect(store.restoreProjects('  ')).rejects.toThrow(/请选择要恢复的工程/);
    await expect(store.purgeProjects('')).rejects.toThrow(/请选择要永久清除的工程/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('restoreProjects 传 projectIds 并刷新列表', async () => {
    installBridge(
      detailHandlers({
        RestoreProjects: () => okResponse({ command: 'RestoreProjects', restored: ['proj1'] }),
      }),
    );
    const store = useStore();
    const result = await store.restoreProjects('proj1');
    expect(result.restored).toEqual(['proj1']);
    const restoreCall = fetchMock.mock.calls.find((call) => parseBody(call[1] as FetchInit).args.command === 'RestoreProjects')!;
    expect(parseBody(restoreCall[1] as FetchInit).args).toMatchObject({ command: 'RestoreProjects', projectIds: 'proj1' });
    expect(sentCommands()).toContain('ListProjects');
  });

  it('purgeProjects 必须带 confirm=true，清除当前工程后关闭详情', async () => {
    installBridge(
      detailHandlers({
        PurgeProjects: () => okResponse({ command: 'PurgeProjects', purged: ['proj1'], freedBlobs: 3 }),
      }),
    );
    const store = useStore();
    await store.openProject('proj1');
    const result = await store.purgeProjects('proj1');
    expect(result).toEqual({ purged: ['proj1'], freedBlobs: 3 });
    expect(store.detail).toBeNull();

    const purgeCall = fetchMock.mock.calls.find((call) => parseBody(call[1] as FetchInit).args.command === 'PurgeProjects')!;
    expect(parseBody(purgeCall[1] as FetchInit).args).toMatchObject({
      command: 'PurgeProjects',
      projectIds: 'proj1',
      confirm: true,
    });
  });
});

// ============================ 历史搜索 / 批次节点 ============================

describe('历史搜索', () => {
  it('searchHistory 透传全部过滤参数（projectId/limit/batch 格式化）', async () => {
    installBridge(detailHandlers());
    const store = useStore();
    await store.openProject('proj1');

    const rows = await store.searchHistory(
      {
        keyword: 'format-tool',
        content: 'print(1)',
        file: 'tools/*.py',
        byMaid: 'DeepSeek',
        op: 'edit',
        todo: 2,
        batch: 21,
        since: '2026-10-01',
        until: '2026-10-31',
        limit: 5,
      },
      { projectId: 'proj1' },
    );

    expect(rows).toHaveLength(3);
    expect(store.history).toHaveLength(3);
    expect(store.historyLoading).toBe(false);
    const historyCall = fetchMock.mock.calls
      .filter((call) => parseBody(call[1] as FetchInit).args.command === 'SearchHistory')
      .pop()!;
    expect(parseBody(historyCall[1] as FetchInit).args).toEqual({
      command: 'SearchHistory',
      projectId: 'proj1',
      keyword: 'format-tool',
      content: 'print(1)',
      file: 'tools/*.py',
      byMaid: 'DeepSeek',
      op: 'edit',
      todo: 2,
      batch: 'b21',
      since: '2026-10-01',
      until: '2026-10-31',
      limit: 5,
    });
  });

  it('searchHistory 未传 projectId 时使用当前工程', async () => {
    installBridge(detailHandlers({ SearchHistory: () => okResponse({ command: 'SearchHistory', nodes: [] }) }));
    const store = useStore();
    await store.openProject('proj1');
    fetchMock.mockClear();
    await store.searchHistory({ file: 'a.ts' });
    expect(parseBody(fetchMock.mock.calls[0][1] as FetchInit).args).toMatchObject({
      projectId: 'proj1',
      file: 'a.ts',
      limit: 100,
    });
  });

  it('getBatchNodes 用 batch=bX 过滤并缓存结果', async () => {
    installBridge(detailHandlers());
    const store = useStore();
    const first = await store.getBatchNodes('proj1', 21);
    expect(first).toHaveLength(3);
    const callsAfterFirst = fetchMock.mock.calls.length;
    const second = await store.getBatchNodes('proj1', 'b21');
    expect(second).toHaveLength(3);
    expect(fetchMock.mock.calls.length).toBe(callsAfterFirst);
    const historyCall = fetchMock.mock.calls[0][1] as FetchInit;
    expect(parseBody(historyCall).args).toMatchObject({ projectId: 'proj1', batch: 'b21', limit: 500 });
  });
});

// ============================ ReadCode / Diff / 回退 / 报告 ============================

describe('ReadCode 与节点 diff', () => {
  it('ReadCode 正文取 content[1..]，并归一化 details.read/skipped/failed', async () => {
    installBridge({
      ReadCode: () =>
        okContentResponse(
          [
            { type: 'text', text: '## ReadCode · 1 个文件' },
            { type: 'text', text: '#### tools/compose.py (8.05 KB)\n```python\n 1 | print(1)\n```' },
          ],
          { command: 'ReadCode', read: [{ path: 'tools/compose.py', kind: 'text', size: 8239 }], skipped: [], failed: [] },
        ),
    });
    const store = useStore();
    const result = await store.readCode('proj1', 'tools/compose.py', '1-8');
    expect(result.title).toBe('tools/compose.py');
    expect(result.text).toContain('print(1)');
    expect(result.files[0]).toMatchObject({ path: 'tools/compose.py', size: 8239 });
    expect(result.skipped).toEqual([]);
    const body = parseBody(fetchMock.mock.calls[0][1] as FetchInit);
    expect(body.args).toMatchObject({ command: 'ReadCode', projectId: 'proj1', path: 'tools/compose.py', lines: '1-8' });
  });

  it('ReadCode 多文件读取：正文多段拼接、symbols/mode 透出', async () => {
    installBridge({
      ReadCode: () =>
        okContentResponse(
          [
            { type: 'text', text: '## ReadCode · 2 个文件' },
            { type: 'text', text: '### a.ts\n```ts\n1 | a\n```' },
            { type: 'text', text: '### b.ts\n```ts\n1 | b\n```' },
          ],
          {
            command: 'ReadCode',
            read: [{ path: 'a.ts' }, { path: 'b.ts' }],
            symbols: [{ path: 'a.ts', qualified: 'A' }],
            mode: 'read',
          },
        ),
    });
    const store = useStore();
    const result = await store.readCode('proj1', '', { paths: ['a.ts', 'b.ts'] });
    expect(result.text).toContain('a.ts');
    expect(result.text).toContain('b.ts');
    expect(result.files).toHaveLength(2);
    expect(result.symbols[0]).toMatchObject({ path: 'a.ts' });
    expect(result.mode).toBe('read');
    const body = parseBody(fetchMock.mock.calls[0][1] as FetchInit);
    expect(body.args).toMatchObject({ paths: 'a.ts\nb.ts' });
  });

  it('ReadCode 空路径本地拦下', async () => {
    const store = useStore();
    await expect(store.readCode('proj1', '   ')).rejects.toThrow(/请输入工程内文件路径/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('getNodeDiff 传 batch=bX 并从首行取标题；数字 node 自动格式化为 nX', async () => {
    installBridge({
      GetNodeDiff: () =>
        okContentResponse(
          [{ type: 'text', text: '## 批次 b21 · 补路径\n```diff\n+1\n```' }],
          { command: 'GetNodeDiff', count: 1 },
        ),
    });
    const store = useStore();
    const batchResult = await store.getNodeDiff('proj1', { batch: 21 });
    expect(batchResult.title).toBe('批次 b21 · 补路径');
    expect(batchResult.text).toContain('+1');
    expect(parseBody(fetchMock.mock.calls[0][1] as FetchInit).args).toMatchObject({
      command: 'GetNodeDiff',
      batch: 'b21',
      maxLines: 400,
    });

    await store.getNodeDiff('proj1', { node: 'n9', maxLines: 120 });
    expect(parseBody(fetchMock.mock.calls[1][1] as FetchInit).args).toMatchObject({ node: 'n9', maxLines: 120 });
  });

  it('getNodeDiff 缺 node/batch 时报 validation；旧 getBatchDiff 仍可用', async () => {
    installBridge({
      GetNodeDiff: () => okContentResponse([{ type: 'text', text: '## 批次 b7\n```diff\n+7\n```' }], { count: 1 }),
    });
    const store = useStore();
    await expect(store.getNodeDiff('proj1', {})).rejects.toThrow(/需要 node（nX）或 batch（bX）/);
    const legacy = await store.getBatchDiff('proj1', 7);
    expect(legacy.text).toContain('批次 b7');
    expect(parseBody(fetchMock.mock.calls[0][1] as FetchInit).args).toMatchObject({ batch: 'b7' });
  });
});

describe('rollback / submitReport', () => {
  it('rollback 格式化 batch、透传 reason/force 与署名，成功后强制重读详情', async () => {
    installBridge(
      detailHandlers({
        Rollback: () => okResponse({ command: 'Rollback', status: 'ok', batchId: 31, count: 1 }, '## ✅ 已回退'),
      }),
    );
    const store = useStore();
    store.setSignature('回退员');
    await store.openProject('proj1');
    fetchMock.mockClear();

    const result = await store.rollback({ projectId: 'proj1', batch: 21, reason: '撤销错误批次', force: true });
    expect(result.status).toBe('ok');
    expect(result.batchId).toBe(31);
    expect(result.maid).toBe('回退员');

    const rollbackCall = fetchMock.mock.calls.find((call) => parseBody(call[1] as FetchInit).args.command === 'Rollback')!;
    expect(parseBody(rollbackCall[1] as FetchInit).args).toEqual({
      command: 'Rollback',
      projectId: 'proj1',
      batch: 'b21',
      force: true,
      reason: '撤销错误批次',
      maid: '回退员',
    });
    // 回退成功后自动刷新列表 + 详情
    expect(sentCommands()).toContain('ListProjects');
    expect(sentCommands()).toContain('GetProject');
  });

  it('rollback 支持 toNode + path + dryRun（数字格式化为 nX）', async () => {
    installBridge(
      detailHandlers({
        Rollback: () => okResponse({ command: 'Rollback', status: 'dryRun', items: [{ rel: 'a.ts', action: '恢复内容', conflicts: [] }] }),
      }),
    );
    const store = useStore();
    const result = await store.rollback({ projectId: 'proj1', toNode: 9, path: 'a.ts', dryRun: true });
    expect(result.status).toBe('dryRun');
    expect(result.items?.[0].rel).toBe('a.ts');
    const rollbackCall = fetchMock.mock.calls.find((call) => parseBody(call[1] as FetchInit).args.command === 'Rollback')!;
    expect(parseBody(rollbackCall[1] as FetchInit).args).toMatchObject({ toNode: 'n9', path: 'a.ts', dryRun: true });
  });

  it('rollback 缺 batch/toNode 时报 validation，且不发请求', async () => {
    const store = useStore();
    await expect(store.rollback({ projectId: 'proj1' })).rejects.toThrow(/需要 batch/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('submitReport 校验 conclusion/status，成功后回读详情', async () => {
    let report: string | null = null;
    let status = 'review';
    installBridge(
      detailHandlers({
        SubmitReport: (args) => {
          report = `# 验收报告\n\n## 结论\n${args.conclusion}`;
          status = args.status || 'review';
          return okResponse({ command: 'SubmitReport', projectId: 'proj1', status }, report);
        },
        GetProject: () => okResponse(projectDetails(BASE_TODOS, { ...PROJECTS[0], report, status })),
      }),
    );
    const store = useStore();
    store.setSignature('验收员');
    await store.openProject('proj1');

    const latest = await store.submitReport({ conclusion: '上传通道已修复', issues: '无', status: 'accepted' });
    expect(latest.project.report).toContain('上传通道已修复');
    expect(latest.project.status).toBe('accepted');

    const submitCall = fetchMock.mock.calls.find((call) => parseBody(call[1] as FetchInit).args.command === 'SubmitReport')!;
    expect(parseBody(submitCall[1] as FetchInit).args).toMatchObject({
      command: 'SubmitReport',
      projectId: 'proj1',
      conclusion: '上传通道已修复',
      issues: '无',
      status: 'accepted',
      maid: '验收员',
    });

    await expect(store.submitReport({ conclusion: '  ' })).rejects.toThrow(/请填写验收结论/);
    await expect(store.submitReport({ conclusion: 'x', status: 'bad' as any })).rejects.toThrow(/status 只能是/);
  });

  it('submitReport 未打开工程时报 validation', async () => {
    const store = useStore();
    await expect(store.submitReport({ conclusion: 'ok' })).rejects.toThrow(/请先打开工程详情/);
  });
});

// ============================ 源码只读命令 / 写命令（T3 契约） ============================

describe('源码只读命令（Outline / FindSymbol / Trace / runCommand）', () => {
  it('outline 传 projectId + paths（多文件换行拼接）并返回 files/text', async () => {
    installBridge({
      Outline: () =>
        okContentResponse(
          [{ type: 'text', text: '## Outline · 2 个文件\n- func a' }],
          { command: 'Outline', files: [{ path: 'a.ts', symbols: [{ name: 'a' }] }, { path: 'b.ts', symbols: [] }] },
        ),
    });
    const store = useStore();
    const result = await store.outline('proj1', ['a.ts', 'b.ts'], { depth: 2, kind: 'function' });
    expect(result.files).toHaveLength(2);
    expect(result.text).toContain('func a');
    const body = parseBody(fetchMock.mock.calls[0][1] as FetchInit);
    expect(body.args).toMatchObject({ command: 'Outline', projectId: 'proj1', paths: 'a.ts\nb.ts', depth: 2, kind: 'function' });
  });

  it('findSymbol 校验 name 并透传过滤参数', async () => {
    installBridge({
      FindSymbol: () =>
        okContentResponse(
          [{ type: 'text', text: '## FindSymbol "Store.putBlob"' }],
          { command: 'FindSymbol', total: 1, scanned: 12, parsed: 12, hits: [{ path: 'store.ts', inProject: true, name: 'putBlob' }] },
        ),
    });
    const store = useStore();
    const result = await store.findSymbol('proj1', 'Store.putBlob', { exact: true, limit: 5, scope: 'workspace' });
    expect(result.total).toBe(1);
    expect(result.hits[0].path).toBe('store.ts');
    expect(parseBody(fetchMock.mock.calls[0][1] as FetchInit).args).toEqual({
      command: 'FindSymbol',
      projectId: 'proj1',
      name: 'Store.putBlob',
      exact: true,
      limit: 5,
      scope: 'workspace',
    });
    await expect(store.findSymbol('proj1', ' ')).rejects.toThrow(/FindSymbol 需要符号名/);
  });

  it('trace 透传 target 并合并 details/text', async () => {
    installBridge({
      Trace: () =>
        okContentResponse(
          [{ type: 'text', text: '## IPC 链路' }],
          { command: 'Trace', kind: 'ipc', found: true },
        ),
    });
    const store = useStore();
    const result = await store.trace('proj1', 'ipc:project-forge:get-project', { scope: 'project' });
    expect(result.kind).toBe('ipc');
    expect(result.found).toBe(true);
    expect(result.text).toContain('IPC 链路');
    expect(parseBody(fetchMock.mock.calls[0][1] as FetchInit).args).toMatchObject({
      command: 'Trace',
      target: 'ipc:project-forge:get-project',
      scope: 'project',
    });
    await expect(store.trace('proj1', '')).rejects.toThrow(/Trace 需要 target/);
  });

  it('runCommand 直通任意 ProjectForge 命令并返回 details', async () => {
    installBridge({
      SearchProjects: () => okResponse({ command: 'SearchProjects', projects: [PROJECTS[0]] }),
    });
    const store = useStore();
    const result = await store.runCommand<{ projects: unknown[] }>('SearchProjects', {
      workspace: 'ai打工人',
      query: 'proj1',
    });
    expect(result.projects).toHaveLength(1);
    expect(parseBody(fetchMock.mock.calls[0][1] as FetchInit).args).toEqual({
      command: 'SearchProjects',
      workspace: 'ai打工人',
      query: 'proj1',
    });
  });
});

describe('编辑写命令（EditCode / CreateFile / …）', () => {
  it('editCode 校验 path/reason，成功后带署名并静默刷新', async () => {
    installBridge(
      detailHandlers({
        EditCode: () => okResponse({ command: 'EditCode', status: 'ok', nodeId: 31, batchId: 9 }, '## ✅ 已写入'),
      }),
    );
    const store = useStore();
    store.setSignature('施工员');
    await store.openProject('proj1');
    fetchMock.mockClear();

    const result = await store.editCode({
      projectId: 'proj1',
      path: 'tools/a.py',
      reason: '修复路径',
      target: 'old',
      replace: 'new',
    });
    expect(result.status).toBe('ok');
    expect(result.nodeId).toBe(31);
    expect(result.text).toContain('已写入');

    const editCall = fetchMock.mock.calls.find((call) => parseBody(call[1] as FetchInit).args.command === 'EditCode')!;
    expect(parseBody(editCall[1] as FetchInit).args).toMatchObject({
      command: 'EditCode',
      projectId: 'proj1',
      path: 'tools/a.py',
      reason: '修复路径',
      target: 'old',
      replace: 'new',
      maid: '施工员',
    });
    expect(sentCommands()).toContain('GetProject');

    await expect(store.editCode({ path: '  ', reason: 'x' } as any)).rejects.toThrow(/文件路径/);
    await expect(store.editCode({ projectId: 'proj1', path: 'a.py', reason: ' ' })).rejects.toThrow(/原因/);
    setActivePinia(createPinia());
    const fresh = useProjectForgeStore();
    await expect(fresh.editCode({ path: 'a.py', reason: 'x' })).rejects.toThrow(/缺少工程 ID/);
  });

  it('editCode 歧义（ambiguous）不触发刷新', async () => {
    installBridge(
      detailHandlers({
        EditCode: () => okResponse({ command: 'EditCode', status: 'ambiguous', ticketId: 't-1' }),
      }),
    );
    const store = useStore();
    await store.openProject('proj1');
    fetchMock.mockClear();
    const result = await store.editCode({ projectId: 'proj1', path: 'a.py', reason: '改一下', target: 'x', replace: 'y' });
    expect(result.status).toBe('ambiguous');
    expect(result.ticketId).toBe('t-1');
    expect(sentCommands()).not.toContain('GetProject');
  });

  it('resolveEdit 需要 ticketId 与 pick', async () => {
    installBridge(
      detailHandlers({
        ResolveEdit: () => okResponse({ command: 'ResolveEdit', status: 'ok', nodeId: 32, batchId: 9 }),
      }),
    );
    const store = useStore();
    await expect(store.resolveEdit({ ticketId: '', pick: 1 })).rejects.toThrow(/ticketId/);
    const result = await store.resolveEdit({ projectId: 'proj1', ticketId: 't-1', pick: '2:1' });
    expect(result.status).toBe('ok');
    const call = fetchMock.mock.calls.find((entry) => parseBody(entry[1] as FetchInit).args.command === 'ResolveEdit')!;
    expect(parseBody(call[1] as FetchInit).args).toMatchObject({ ticketId: 't-1', pick: '2:1' });
  });

  it('createFile / removeFile / moveFile 参数归一化', async () => {
    installBridge(
      detailHandlers({
        CreateFile: () => okResponse({ command: 'CreateFile', status: 'ok', nodeId: 40, batchId: 10 }),
        RemoveFile: () => okResponse({ command: 'RemoveFile', status: 'ok', nodeId: 41, batchId: 11 }),
        MoveFile: () => okResponse({ command: 'MoveFile', status: 'ok', nodeId: 42, batchId: 12 }),
      }),
    );
    const store = useStore();
    await store.createFile({ projectId: 'proj1', path: 'src/new.ts', content: 'export {};', reason: '新建文件' });
    await store.removeFile({ projectId: 'proj1', paths: ['a.ts', 'b.ts'], reason: '清理' });
    await store.moveFile({ projectId: 'proj1', from: 'a.ts', to: 'src/a.ts', reason: '移动' });

    const createCall = fetchMock.mock.calls.find((call) => parseBody(call[1] as FetchInit).args.command === 'CreateFile')!;
    expect(parseBody(createCall[1] as FetchInit).args).toMatchObject({
      command: 'CreateFile',
      path: 'src/new.ts',
      content: 'export {};',
      reason: '新建文件',
    });
    const removeCall = fetchMock.mock.calls.find((call) => parseBody(call[1] as FetchInit).args.command === 'RemoveFile')!;
    expect(parseBody(removeCall[1] as FetchInit).args).toMatchObject({ paths: 'a.ts\nb.ts', reason: '清理' });
    const moveCall = fetchMock.mock.calls.find((call) => parseBody(call[1] as FetchInit).args.command === 'MoveFile')!;
    expect(parseBody(moveCall[1] as FetchInit).args).toMatchObject({ from: 'a.ts', to: 'src/a.ts' });
  });

  it('moveCode / copyCode 走对应命令并保留 reason', async () => {
    installBridge(
      detailHandlers({
        MoveCode: () => okResponse({ command: 'MoveCode', status: 'ok', nodeId: 50, batchId: 13 }),
        CopyCode: () => okResponse({ command: 'CopyCode', status: 'ok', nodeId: 51, batchId: 14 }),
      }),
    );
    const store = useStore();
    await store.moveCode({ projectId: 'proj1', from: 'a.ts:1-3', to: 'b.ts', after: 10, reason: '挪代码' });
    await store.copyCode({ projectId: 'proj1', from: 'a.ts#calc', to: 'b.ts', before: 2, reason: '复制代码' });
    const moveCall = fetchMock.mock.calls.find((call) => parseBody(call[1] as FetchInit).args.command === 'MoveCode')!;
    expect(parseBody(moveCall[1] as FetchInit).args).toMatchObject({ from: 'a.ts:1-3', to: 'b.ts', after: 10, reason: '挪代码' });
    const copyCall = fetchMock.mock.calls.find((call) => parseBody(call[1] as FetchInit).args.command === 'CopyCode')!;
    expect(parseBody(copyCall[1] as FetchInit).args).toMatchObject({ from: 'a.ts#calc', before: 2 });
  });

  it('createFile 缺 reason 时本地拦下', async () => {
    const store = useStore();
    await expect(store.createFile({ projectId: 'proj1', path: 'a.ts', content: '', reason: ' ' })).rejects.toThrow(/原因/);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

// ============================ 署名 / 设置 ============================

describe('署名与桥接设置', () => {
  it('setSignature 持久化到独立 key，不影响桥接配置 key', () => {
    const store = useStore();
    store.setSignature('  工具A  ');
    expect(store.signature).toBe('工具A');
    expect(store.hasSignature).toBe(true);
    expect(localStorage.getItem(SIGNATURE_STORAGE_KEY)).toBe('工具A');
    expect(localStorage.getItem(BRIDGE_STORAGE_KEY)).toBeNull();
    expect(localStorage.getItem('vcp.projectForge.settings.v1')).toBeNull();

    setActivePinia(createPinia());
    const next = useProjectForgeStore();
    expect(next.signature).toBe('工具A');

    next.setSignature('   ');
    expect(next.signature).toBe('');
    expect(localStorage.getItem(SIGNATURE_STORAGE_KEY)).toBeNull();
  });

  it('saveSettings 规整地址并持久化到统一配置 key（同时镜像旧 key），新实例优先生效', () => {
    const store = useStore();
    store.saveSettings({ baseUrl: ' http://127.0.0.1:8080/api/mobile-bridge/ ', token: ' test-token ' });

    expect(store.baseUrl).toBe('http://127.0.0.1:8080/api/mobile-bridge');
    expect(store.token).toBe('test-token');
    expect(store.configSource).toBe('saved');
    expect(JSON.parse(localStorage.getItem(BRIDGE_STORAGE_KEY)!)).toEqual({
      baseUrl: 'http://127.0.0.1:8080/api/mobile-bridge',
      token: 'test-token',
    });
    // 兼容旧版本：镜像写入 vcp.projectForge.settings.v1
    expect(JSON.parse(localStorage.getItem('vcp.projectForge.settings.v1')!)).toEqual({
      baseUrl: 'http://127.0.0.1:8080/api/mobile-bridge',
      token: 'test-token',
    });

    setActivePinia(createPinia());
    const next = useProjectForgeStore();
    expect(next.baseUrl).toBe('http://127.0.0.1:8080/api/mobile-bridge');
    expect(next.token).toBe('test-token');
  });

  it('未保存过时复用「远程桌面」的桥接配置', () => {
    localStorage.setItem('vcp.remoteDesktop.settings.v1', JSON.stringify({ baseUrl: 'http://desk:1/api/mobile-bridge', token: 'desk-token' }));
    const store = useStore();
    expect(store.baseUrl).toBe('http://desk:1/api/mobile-bridge');
    expect(store.token).toBe('desk-token');
    expect(store.configSource).toBe('legacy');
  });

  it('没有显式/旧配置时，从同步 HTTP 地址推导桥接地址（无影云场景）', () => {
    applyBridgeConfigFromAppSettings({
      syncHttpUrl: 'http://desk.example.com:5974',
      syncToken: 'sync-token',
    });
    const store = useStore();
    expect(store.baseUrl).toBe('http://desk.example.com:5974/api/mobile-bridge');
    expect(store.token).toBe('sync-token');
    expect(store.configSource).toBe('sync');
  });

  it('旧 key 仍是历史默认公网桥时视为未配置，改用同步设置推导', () => {
    localStorage.setItem(
      'vcp.projectForge.settings.v1',
      JSON.stringify({ baseUrl: LEGACY_DEFAULT_BASE, token: DEFAULT_BRIDGE_TOKEN }),
    );
    applyBridgeConfigFromAppSettings({ syncHttpUrl: 'http://10.0.0.2:5974', syncToken: 'tk' });
    const store = useStore();
    expect(store.baseUrl).toBe('http://10.0.0.2:5974/api/mobile-bridge');
    expect(store.configSource).toBe('sync');
  });

  it('保存设置后清空缓存并重置连接状态', async () => {
    installBridge(detailHandlers());
    const store = useStore();
    await store.bootstrap();
    expect(store.projects.length).toBe(2);

    store.saveSettings({ baseUrl: 'http://127.0.0.1:9/api/mobile-bridge', token: 'x' });
    expect(store.projects).toEqual([]);
    expect(store.detail).toBeNull();
    expect(store.currentProjectId).toBeNull();
    expect(store.connection.state).toBe('unknown');
  });

  it('resetSettings 清除显式保存（含旧 key 镜像）并回到默认推导', () => {
    const store = useStore();
    store.saveSettings({ baseUrl: 'http://127.0.0.1:8080/api/mobile-bridge', token: 'tk' });
    expect(localStorage.getItem(BRIDGE_STORAGE_KEY)).not.toBeNull();

    store.resetSettings();
    expect(localStorage.getItem(BRIDGE_STORAGE_KEY)).toBeNull();
    expect(localStorage.getItem('vcp.projectForge.settings.v1')).toBeNull();
    expect(store.baseUrl).toBe(LEGACY_DEFAULT_BASE);
    expect(store.configSource).toBe('default');
    expect(store.token).toBe(DEFAULT_BRIDGE_TOKEN);
  });

  it('testConnection 成功/失败都会写回连接状态', async () => {
    installBridge(detailHandlers());
    const store = useStore();
    expect(await store.testConnection()).toBe(true);
    expect(store.connection.ok).toBe(true);
    expect(store.connection.message).toContain('已连接');

    fetchMock.mockRejectedValue(new TypeError('Failed to fetch'));
    expect(await store.testConnection()).toBe(false);
    expect(store.connection.state).toBe('fail');
    expect(store.connection.message).toContain('无法连接桌面桥接');
  });
});

// ============================ 错误态 ============================

describe('错误态', () => {
  it('网络不可达 → network 错误 + 连接失败态 + error/errorKind', async () => {
    fetchMock.mockRejectedValue(new TypeError('Failed to fetch'));
    const store = useStore();
    await expect(store.loadProjects()).rejects.toThrow(/无法连接桌面桥接/);
    expect(store.listError).toContain('无法连接桌面桥接');
    expect(store.listErrorKind).toBe('network');
    expect(store.connection.state).toBe('fail');
    expect(store.loading.projects).toBe(false);
    expect(store.error).toContain('无法连接桌面桥接');
    expect(store.errorKind).toBe('network');
  });

  it('非 JSON 响应 → protocol 错误（提示检查 Base 地址）', async () => {
    fetchMock.mockResolvedValue(httpResponse('<html>bad gateway</html>', 200));
    const store = useStore();
    await expect(store.loadProjects()).rejects.toThrow(/非 JSON 响应/);
    expect(store.listErrorKind).toBe('protocol');
  });

  it('401 鉴权失败 → auth 错误（提示检查 Token）', async () => {
    fetchMock.mockResolvedValue(
      httpResponse({ ok: false, error: { code: 'BRIDGE_AUTH_FAILED', message: 'Unauthorized' } }, 401),
    );
    const store = useStore();
    await expect(store.loadProjects()).rejects.toThrow(/Token/);
    expect(store.listErrorKind).toBe('auth');
    expect(store.connection.state).toBe('fail');
  });

  it('响应缺少 result 字段 → protocol 错误', async () => {
    fetchMock.mockResolvedValue(httpResponse({ ok: true }, 200));
    const store = useStore();
    await expect(store.loadProjects()).rejects.toThrow(/缺少 result/);
    expect(store.listErrorKind).toBe('protocol');
  });

  it('刷新 refreshAll 汇集失败（工作区先失败时列表仍会执行并记录错误）', async () => {
    fetchMock.mockRejectedValue(new TypeError('Failed to fetch'));
    const store = useStore();
    await expect(store.refreshAll()).rejects.toThrow(/无法连接桌面桥接/);
    expect(sentCommands !== undefined).toBe(true);
  });
});
