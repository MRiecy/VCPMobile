/**
 * T3 源码数据层（useProjectForgeSource）单测。
 *
 * mock `projectForgeApi`：ProjectForge 命令 / FileOperator / PTYShellExecutor 全走内存 bridge，
 * 覆盖：工作区工程选择、文件树懒加载、FileOperator→PTY 降级、文件名搜索、读文件、保存成功/回读不一致/
 * target 失败退行区间、新建、删除、移动、Outline/FindSymbol/Trace、选择持久化。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

const apiMock = vi.hoisted(() => ({
  pfInvoke: vi.fn(),
  pfInvokeEnvelope: vi.fn(),
  pluginInvoke: vi.fn(),
  ptyExec: vi.fn(),
}));

vi.mock('../../../../features/projectforge/projectForgeApi', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../../../features/projectforge/projectForgeApi')>();
  return { ...actual, ...apiMock };
});

import {
  __resetProjectForgeSourceForTests,
  SOURCE_PROJECT_STORAGE_KEY,
  SOURCE_WORKSPACE_STORAGE_KEY,
  unwrapFilePayload,
  useProjectForgeSource,
} from '../../../../features/projectforge/useProjectForgeSource';
import { installSourceBridge, PROJECT_ROOT, settle } from './sourceTestUtils';

function setup(options: Parameters<typeof installSourceBridge>[1] = {}) {
  const state = installSourceBridge(apiMock, options);
  const source = useProjectForgeSource();
  return { state, source, options: options };
}

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  __resetProjectForgeSourceForTests();
});

describe('工作区 / 工程选择', () => {
  it('initSource 依次加载工作区、工程、根目录并渲染文件树', async () => {
    const { source } = setup();
    await source.initSource();
    await settle(2);

    expect(apiMock.pfInvoke).toHaveBeenCalledWith('ListWorkspaces');
    expect(apiMock.pfInvoke).toHaveBeenCalledWith('ListProjects', { limit: 100, workspace: '工程区' });
    expect(apiMock.pfInvoke).toHaveBeenCalledWith('GetProject', { projectId: 'p1', timeline: 1 });
    expect(source.selectedWorkspaceAlias.value).toBe('工程区');
    expect(source.selectedProjectId.value).toBe('p1');
    expect(source.projectRoot.value).toBe(PROJECT_ROOT);
    expect(source.workspaces.value).toHaveLength(1);
    expect(source.projects.value.map((item) => item.id)).toEqual(['p1']);
    expect(source.treeRows.value.map((row) => row.name)).toEqual(['src', 'README.md']);
    expect(source.transport.value).toBe('fileoperator');
    expect(apiMock.pluginInvoke).toHaveBeenCalledWith('FileOperator', {
      command: 'ListDirectory',
      directoryPath: PROJECT_ROOT,
      showHidden: false,
    });
  });

  it('选择状态持久化到 localStorage，下次初始化优先恢复', async () => {
    const { source } = setup();
    await source.initSource();
    await settle(2);
    expect(localStorage.getItem(SOURCE_WORKSPACE_STORAGE_KEY)).toBe('工程区');
    expect(localStorage.getItem(SOURCE_PROJECT_STORAGE_KEY)).toBe('p1');

    __resetProjectForgeSourceForTests();
    vi.clearAllMocks();
    installSourceBridge(apiMock);
    const next = useProjectForgeSource();
    await next.initSource();
    await settle(2);
    expect(next.selectedWorkspaceAlias.value).toBe('工程区');
    expect(next.selectedProjectId.value).toBe('p1');
  });
});

describe('文件树懒加载与降级', () => {
  it('展开目录时才按需加载子目录，折叠后子行隐藏', async () => {
    const { source } = setup();
    await source.initSource();
    await settle(2);

    expect(source.treeRows.value).toHaveLength(2);
    // 初始化时先取一次 FileOperator 白名单（ListAllowedDirectories），再列一次根目录。
    const dirCalls = () =>
      apiMock.pluginInvoke.mock.calls.filter(
        (call) => call[0] === 'FileOperator' && (call[1] as { command?: string })?.command === 'ListDirectory',
      ).length;
    expect(dirCalls()).toBe(1);

    await source.toggleDir('src');
    expect(source.isExpanded('src')).toBe(true);
    expect(apiMock.pluginInvoke).toHaveBeenCalledWith('FileOperator', {
      command: 'ListDirectory',
      directoryPath: `${PROJECT_ROOT}/src`,
      showHidden: false,
    });
    const rows = source.treeRows.value;
    expect(rows.map((row) => row.path)).toEqual(['src', 'src/main.ts', 'README.md']);
    expect(rows[1].depth).toBe(1);

    await source.toggleDir('src');
    expect(source.isExpanded('src')).toBe(false);
    expect(source.treeRows.value.map((row) => row.name)).toEqual(['src', 'README.md']);
  });

  it('FileOperator 异常时自动使用 PTY 兼容模式（提示为中性文案，不暴露原始错误）', async () => {
    const options: Parameters<typeof installSourceBridge>[1] = { fileOperatorFails: true };
    const { source } = setup(options);
    await source.initSource();
    await settle(2);

    expect(source.transport.value).toBe('pty');
    expect(source.transportNote.value).toContain('兼容模式');
    expect(source.transportNote.value).not.toContain('Access denied');
    expect(source.transportNote.value).not.toContain('降级');
    expect(apiMock.ptyExec).toHaveBeenCalled();
    expect(source.treeRows.value.map((row) => row.name)).toEqual(['src', 'README.md']);

    // 恢复后「重试」会重新探测 FileOperator
    if (options) options.fileOperatorFails = false;
    await source.refreshTree({ retryTransport: true });
    expect(source.transport.value).toBe('fileoperator');
  });

  it('工程根目录不在 FileOperator 白名单时静默走 PTY 兼容模式（不算错误）', async () => {
    const { state, source } = setup({ fileOperatorNotAllowed: true });
    await source.initSource();
    await settle(2);

    expect(source.transport.value).toBe('pty');
    expect(source.transportNote.value).toBe('');
    expect(source.connectionLabel.value).toContain('兼容模式');
    expect(state.fileOperatorCommands).toContain('ListAllowedDirectories');
    expect(state.fileOperatorCommands).not.toContain('ListDirectory');
    expect(source.treeRows.value.map((row) => row.name)).toEqual(['src', 'README.md']);
  });

  it('真实 FileOperator 返回形态（details.items + content 摘要）按 items 解析，不把 Markdown 当文件行', async () => {
    const { source } = setup({ fileOperatorRealShape: true });
    await source.initSource();
    await settle(2);

    expect(source.transport.value).toBe('fileoperator');
    const rows = source.treeRows.value;
    expect(rows.map((row) => row.name)).toEqual(['src', 'README.md']);
    expect(rows.find((row) => row.name === 'src')?.type).toBe('dir');
    expect(rows.find((row) => row.name === 'README.md')?.size).toBe(6);
    // 不允许把 Markdown 表格行（`| ... |`）当成文件名
    expect(rows.some((row) => row.name.includes('|'))).toBe(false);

    await source.runSearch('main');
    expect(source.searchTransport.value).toBe('fileoperator');
    expect(source.searchResults.value.map((entry) => entry.path)).toEqual(['src/main.ts']);
  });

  it('unwrapFilePayload 在 items/results 存在时不把 details.content 摘要当成载荷', () => {
    const payload = {
      path: '/home/user/example-repo',
      items: [{ name: 'README.md', path: '/home/user/example-repo/README.md', type: 'file', size: 386 }],
      totalItems: 1,
      content: [{ type: 'text', text: '### 目录内容\n| 文件名称 | 类型 |\n|---|---|' }],
    };
    const unwrapped = unwrapFilePayload(payload) as Record<string, unknown>;
    expect(Array.isArray(unwrapped.items)).toBe(true);
    expect(typeof unwrapped).toBe('object');
  });

  it('目录加载失败时写入错误状态（可重试）', async () => {
    const { source } = setup({ fileOperatorFails: true });
    apiMock.ptyExec.mockRejectedValue(new Error('PTY 执行失败'));
    await source.initSource();
    await settle(2);
    expect(source.treeError.value).toContain('PTY 执行失败');
    expect(source.treeRows.value).toHaveLength(0);
  });
});

describe('文件名搜索', () => {
  it('走 FileOperator.SearchFiles，并对多关键字做客户端过滤', async () => {
    const { source } = setup();
    await source.initSource();
    await settle(2);

    await source.runSearch('main');
    expect(apiMock.pluginInvoke).toHaveBeenCalledWith('FileOperator', {
      command: 'SearchFiles',
      searchPath: PROJECT_ROOT,
      pattern: '*main*',
    });
    expect(source.searchResults.value.map((entry) => entry.path)).toEqual(['src/main.ts']);
    expect(source.searchTotal.value).toBe(1);
    expect(source.searchTransport.value).toBe('fileoperator');

    await source.runSearch('src main');
    expect(source.searchResults.value.map((entry) => entry.path)).toEqual(['src/main.ts']);

    source.clearSearch();
    expect(source.searchQuery.value).toBe('');
    expect(source.searchResults.value).toHaveLength(0);
  });

  it('FileOperator 搜索失败时降级到 PTY find', async () => {
    const { source } = setup({ fileOperatorFails: true });
    await source.initSource();
    await settle(2);

    await source.runSearch('main');
    expect(apiMock.ptyExec).toHaveBeenCalled();
    expect(source.searchTransport.value).toBe('pty');
    expect(source.searchResults.value.map((entry) => entry.path)).toEqual(['src/main.ts']);
    // 文件树阶段已提示兼容模式；搜索静默复用同一通道，不再重复横幅。
    expect(source.searchNote.value).toBe('');
    expect(source.transportNote.value).toContain('兼容模式');
  });
});

describe('读取文件', () => {
  it('openFile 用 ReadCode（showLine:false）读取正文并展开所在目录', async () => {
    const { source } = setup();
    await source.initSource();
    await settle(2);

    const file = await source.openFile('src/main.ts');
    expect(apiMock.pfInvokeEnvelope).toHaveBeenCalledWith('ReadCode', {
      projectId: 'p1',
      path: 'src/main.ts',
      showLine: false,
    });
    expect(file.path).toBe('src/main.ts');
    expect(file.text).toBe('export const a = 1;\n');
    expect(file.lineCount).toBe(2);
    expect(file.eol).toBe('LF');
    expect(source.currentFile.value?.path).toBe('src/main.ts');
    expect(source.treeRows.value.some((row) => row.path === 'src/main.ts')).toBe(true);
  });

  it('读取不存在的文件时记录 fileError', async () => {
    const { source } = setup();
    await source.initSource();
    await settle(2);

    await expect(source.openFile('src/none.ts')).rejects.toThrow('文件不存在');
    expect(source.fileError.value).toContain('文件不存在');
    expect(source.currentFile.value).toBeNull();
  });
});

describe('保存（EditCode 整文件替换 + 回读校验）', () => {
  it('保存成功后回读校验一致并记录代码审查提示', async () => {
    const { state, source } = setup();
    await source.initSource();
    await settle(2);
    await source.openFile('src/main.ts');

    const result = await source.saveFile('export const a = 2;\n', '修正常量值');
    expect(apiMock.pfInvoke).toHaveBeenCalledWith('EditCode', {
      projectId: 'p1',
      path: 'src/main.ts',
      reason: '修正常量值',
      target: 'export const a = 1;\n',
      replace: 'export const a = 2;\n',
    });
    expect(state.disk['src/main.ts']).toBe('export const a = 2;\n');
    expect(result.path).toBe('src/main.ts');
    expect(result.warnings).toHaveLength(1);
    expect(source.currentFile.value?.text).toBe('export const a = 2;\n');
    expect(source.lastWriteNotice.value).toContain('已保存');
  });

  it('reason 为空时拒绝写入', async () => {
    const { source } = setup();
    await source.initSource();
    await settle(2);
    await source.openFile('src/main.ts');

    await expect(source.saveFile('changed', '   ')).rejects.toThrow('请填写本次改动原因');
    expect(apiMock.pfInvoke).not.toHaveBeenCalledWith('EditCode', expect.anything());
  });

  it('回读内容不一致时抛错并重新载入磁盘内容', async () => {
    const { source } = setup({ skipApplyEdits: true });
    await source.initSource();
    await settle(2);
    await source.openFile('src/main.ts');

    await expect(source.saveFile('export const a = 2;\n', '修正常量值')).rejects.toThrow('回读校验失败');
    expect(source.actionError.value).toContain('回读校验失败');
    expect(source.currentFile.value?.text).toBe('export const a = 1;\n');
  });

  it('target 精确替换失败时退化为行区间整文件替换', async () => {
    const { state, source } = setup({ failTargetReplace: true });
    await source.initSource();
    await settle(2);
    await source.openFile('src/main.ts');

    await source.saveFile('export const a = 3;\n', '再次修正');
    expect(state.editCalls).toHaveLength(2);
    expect(state.editCalls[0].target).toBe('export const a = 1;\n');
    expect(state.editCalls[1]).toEqual({
      projectId: 'p1',
      path: 'src/main.ts',
      reason: '再次修正',
      start: 1,
      end: 1,
      content: 'export const a = 3;\n',
    });
    expect(source.currentFile.value?.text).toBe('export const a = 3;\n');
  });
});

describe('新建 / 删除 / 移动', () => {
  it('createFile 新建后回读校验并刷新父目录', async () => {
    const { state, source } = setup();
    await source.initSource();
    await settle(2);

    const file = await source.createFile({ path: 'src/added.ts', content: 'export const b = 2;\n', reason: '新增文件' });
    expect(state.createCalls[0]).toMatchObject({
      projectId: 'p1',
      path: 'src/added.ts',
      content: 'export const b = 2;\n',
      reason: '新增文件',
    });
    expect(file.path).toBe('src/added.ts');
    expect(source.currentFile.value?.path).toBe('src/added.ts');
    expect(source.treeRows.value.some((row) => row.path === 'src/added.ts')).toBe(true);
    expect(source.lastWriteNotice.value).toContain('已新建');
  });

  it('removeFile 删除后目录回读校验通过并关闭当前文件', async () => {
    const { state, source } = setup();
    await source.initSource();
    await settle(2);
    await source.openFile('README.md');

    await source.removeFile('README.md', '清理旧说明');
    expect(state.removeCalls[0]).toMatchObject({ projectId: 'p1', path: 'README.md', reason: '清理旧说明' });
    expect(source.currentFile.value).toBeNull();
    expect(source.treeRows.value.map((row) => row.name)).toEqual(['src']);
    expect(source.lastWriteNotice.value).toContain('已删除');
  });

  it('removeFile 在文件仍出现在目录中时判定回读失败', async () => {
    const { source } = setup({ keepDeletedListed: true });
    await source.initSource();
    await settle(2);
    await source.openFile('README.md');

    await expect(source.removeFile('README.md', '清理旧说明')).rejects.toThrow('回读校验失败');
    expect(source.actionError.value).toContain('回读校验失败');
  });

  it('moveFile 移动后回读校验并更新当前文件路径', async () => {
    const { state, source } = setup();
    await source.initSource();
    await settle(2);
    await source.openFile('src/main.ts');

    const file = await source.moveFile({ from: 'src/main.ts', to: 'src/entry.ts', reason: '统一入口命名' });
    expect(state.moveCalls[0]).toMatchObject({
      projectId: 'p1',
      from: 'src/main.ts',
      to: 'src/entry.ts',
      reason: '统一入口命名',
    });
    expect(file.path).toBe('src/entry.ts');
    expect(source.currentFile.value?.path).toBe('src/entry.ts');
    expect(state.disk['src/entry.ts']).toBe('export const a = 1;\n');
    expect(source.lastWriteNotice.value).toContain('已移动到');
  });

  it('moveFile 校验目标路径不能与原路径相同', async () => {
    const { source } = setup();
    await source.initSource();
    await settle(2);
    await source.openFile('src/main.ts');

    await expect(source.moveFile({ from: 'src/main.ts', to: 'src/main.ts', reason: '无效' })).rejects.toThrow(
      '目标路径与原路径相同',
    );
  });
});

describe('符号大纲 / 符号查找 / 链路查询', () => {
  it('loadOutline / findSymbol / trace 解析结构化结果', async () => {
    const { source } = setup();
    await source.initSource();
    await settle(2);
    await source.openFile('src/main.ts');

    await source.loadOutline();
    expect(apiMock.pfInvokeEnvelope).toHaveBeenCalledWith('Outline', {
      projectId: 'p1',
      path: 'src/main.ts',
      depth: 3,
    });
    expect(source.outline.value).toEqual([{ name: 'a', kind: 'variable', start: 1, end: 1, depth: 0 }]);

    await source.findSymbol('a');
    expect(apiMock.pfInvokeEnvelope).toHaveBeenCalledWith('FindSymbol', {
      projectId: 'p1',
      name: 'a',
      limit: 50,
    });
    expect(source.findSymbolHits.value[0].path).toBe('src/main.ts');
    expect(source.findSymbolHits.value[0].start).toBe(1);
    expect(source.findSymbolNote.value).toContain('扫描');

    const trace = await source.trace('ipc:demo:invoke');
    expect(source.traceResult.value?.text).toContain('主进程');
    expect(trace.target).toBe('ipc:demo:invoke');
    expect(source.traceError.value).toBe('');
  });

  it('empty 查询参数时给出中文校验错误', async () => {
    const { source } = setup();
    await source.initSource();
    await settle(2);
    await expect(source.trace('')).rejects.toThrow('请输入要查询的链路目标');
    await expect(source.findSymbol('')).rejects.toThrow('请输入要查找的符号名称');
  });
});
