/**
 * T2（工程详情 / 列表 / diff）组件测试公共工具。
 *
 * 组件只依赖 useProjectForgeStore；各测试文件用 vi.mock 把该模块替换成这里
 * createMockStore() 生成的可观测对象（T1 落地前的契约先行测试）。
 */
import { createPinia, setActivePinia } from 'pinia';
import { reactive } from 'vue';
import { mount } from '@vue/test-utils';
import { vi } from 'vitest';
import { useNotificationStore } from '../../../../core/stores/notification';

export function createMockStore(overrides: Record<string, any> = {}): any {
  const store: any = {
    // —— state（spec §5 契约）——
    projects: [],
    includeDeleted: false,
    projectQuery: '',
    currentProjectId: null,
    currentProject: null,
    detail: null,
    workspaces: [],
    signature: '',
    error: '',
    errorKind: '',
    listError: '',
    detailError: '',
    connection: { ok: true, message: '', state: 'ok' },
    loading: {
      workspaces: false,
      projects: false,
      detail: false,
      history: false,
      action: false,
    },
    // —— actions ——
    bootstrap: vi.fn().mockResolvedValue(undefined),
    refreshAll: vi.fn().mockResolvedValue(undefined),
    loadWorkspaces: vi.fn().mockResolvedValue([]),
    loadProjects: vi.fn().mockResolvedValue(undefined),
    searchProjects: vi.fn().mockResolvedValue(undefined),
    openProject: vi.fn().mockResolvedValue(null),
    closeProject: vi.fn(),
    createProject: vi.fn().mockResolvedValue(null),
    deleteProjects: vi.fn().mockResolvedValue(undefined),
    restoreProjects: vi.fn().mockResolvedValue(undefined),
    purgeProjects: vi.fn().mockResolvedValue(undefined),
    updateTodos: vi.fn().mockResolvedValue(null),
    submitReport: vi.fn().mockResolvedValue(undefined),
    searchHistory: vi.fn().mockResolvedValue([]),
    getBatchNodes: vi.fn().mockResolvedValue([]),
    getNodeDiff: vi.fn().mockResolvedValue({ title: '', text: '' }),
    rollback: vi.fn().mockResolvedValue({ status: 'ok', batchId: 7 }),
    readCode: vi.fn(),
    setSignature: vi.fn(),
  };
  // 用 reactive 包裹，模拟真实 Pinia store 的响应式（测试里可以改写 state 观察重渲染）
  return reactive(Object.assign(store, overrides));
}

/** 用真实 Pinia + 真实通知 store（通知调用被 spy 掉，避免计时器与 Toast DOM）。 */
export function mountWithPinia(component: any, options: Record<string, any> = {}): {
  wrapper: any;
  notify: ReturnType<typeof vi.fn>;
  pinia: ReturnType<typeof createPinia>;
} {
  const pinia = createPinia();
  setActivePinia(pinia);
  const notificationStore = useNotificationStore(pinia);
  const notify = vi.fn();
  (notificationStore as any).addNotification = notify;
  const mergedPlugins = [pinia, ...(((options.global || {}).plugins as any[]) || [])];
  const wrapper = mount(component, {
    ...options,
    global: {
      ...(options.global || {}),
      plugins: mergedPlugins,
      // Teleport 内容渲染在组件树内（否则确认弹层 / 底部弹层会挂到 document.body，难以断言）
      stubs: { Teleport: true, ...(((options.global || {}).stubs) || {}) },
    },
  });
  return { wrapper, notify, pinia };
}

/** 让组件内的异步链（openProject / searchHistory 等 Promise）跑完。 */
export async function flush(times = 4): Promise<void> {
  const { flushPromises } = await import('@vue/test-utils');
  for (let index = 0; index < times; index += 1) {
    await flushPromises();
  }
}

// ---------------------------------------------------------------------------
// fixtures（字段与 spec §5 契约一致）
// ---------------------------------------------------------------------------

export function makeProject(overrides: Record<string, any> = {}): any {
  return {
    id: 'proj1',
    name: '示例工程 A',
    workspace_alias: 'ai打工人',
    status: 'review',
    root: '/home/user/ai/dsk-money-worker',
    rootInfo: {
      root: '/home/user/ai/dsk-money-worker',
      workspaceAlias: 'ai打工人',
      writable: true,
    },
    progress: { done: 3, total: 8, text: '3/8' },
    stats: {
      nodeCount: 14,
      fileCount: 7,
      added: 846,
      removed: 73,
      lastAt: '2026-10-29T11:19:14.830Z',
    },
    created_by: 'claude',
    created_at: '2026-10-20T03:00:00.000Z',
    updated_at: '2026-10-29T11:19:14.830Z',
    deleted_at: null,
    deleted_by: null,
    report: null,
    report_by: null,
    maids: ['claude'],
    ...overrides,
  };
}

export function makeTodo(overrides: Record<string, any> = {}): any {
  return {
    seq: 1,
    title: '修复上传通道',
    status: 'pending',
    note: null,
    updated_by: 'claude',
    updated_at: '2026-10-29T11:00:00.000Z',
    ...overrides,
  };
}

export function makeHistoryRow(overrides: Record<string, any> = {}): any {
  return {
    id: 21,
    batch_id: 5,
    batch_kind: 'edit',
    op: 'edit',
    file_path: 'tools/video.py',
    maid: 'claude',
    effective_reason: '补充说明',
    summary: '',
    added: 7,
    removed: 2,
    created_at: '2026-10-29T11:18:58.496Z',
    ...overrides,
  };
}

export function makeBatch(overrides: Record<string, any> = {}): any {
  return {
    id: 5,
    kind: 'edit',
    maid: 'claude',
    reason: '补充说明',
    created_at: '2026-10-29T11:18:58.496Z',
    node_count: 2,
    added: 10,
    removed: 3,
    files: ['tools/video.py', 'tools/run.py'],
    ...overrides,
  };
}

export function makeFileChange(overrides: Record<string, any> = {}): any {
  return {
    file_path: 'tools/video.py',
    edits: 3,
    added: 12,
    removed: 4,
    last_node: 21,
    ...overrides,
  };
}

export function makeContributor(overrides: Record<string, any> = {}): any {
  return {
    maid: 'claude',
    batches: 4,
    added: 120,
    removed: 30,
    last_at: '2026-10-29T11:18:58.496Z',
    ...overrides,
  };
}

export function makeDetail(overrides: Record<string, any> = {}): any {
  const project = overrides.project ?? makeProject();
  return {
    project,
    todos: [makeTodo()],
    stats: project.stats,
    timeline: [makeBatch()],
    files: [makeFileChange()],
    contributors: [makeContributor()],
    loadedAt: Date.UTC(2026, 9, 29, 11, 20, 0),
    ...overrides,
  };
}
