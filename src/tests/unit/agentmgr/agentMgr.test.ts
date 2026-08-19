import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import {
  collectTaskReferences,
  draftFromAgent,
  draftToAgentObject,
  emptyAgentDraft,
  normalizeAgentEntry,
  normalizeAgentList,
  normalizeGlobalConfig,
  validateAgentDraft,
} from '@/features/agentmgr/agentMgrTypes';
import { useAgentMgrStore } from '@/features/agentmgr/agentMgrStore';
import { clearInvokeMocks, mockInvoke } from '../../mocks/tauri';

const baseConfig = () => ({
  maxHistoryRounds: 9,
  contextTtlHours: 48,
  globalSystemPrompt: '共享提示词',
  delegationMaxRounds: 20,
  delegationTimeout: 600000,
  customFutureTopKey: { keep: true },
  agents: [
    {
      chineseName: '小娜',
      baseName: 'NOVA',
      modelId: 'gpt-4o',
      description: '代码助手',
      systemPrompt: '你是 {{MaidName}}',
      maxOutputTokens: 32000,
      temperature: 0.5,
      customField: 'preserved',
    },
    { chineseName: '小冰', modelId: 'default' },
  ],
});

describe('Agent 管理 · 归一化层', () => {
  it('normalizeAgentEntry 解析 7 字段并把未知键收进 extras', () => {
    const entry = normalizeAgentEntry(baseConfig().agents[0])!;
    expect(entry.chineseName).toBe('小娜');
    expect(entry.modelId).toBe('gpt-4o');
    expect(entry.maxOutputTokens).toBe(32000);
    expect(entry.extras).toEqual({ customField: 'preserved' });
  });

  it('normalizeAgentEntry 对缺省字段回落插件默认值', () => {
    const entry = normalizeAgentEntry({ chineseName: '小冰', modelId: 'default' })!;
    expect(entry.maxOutputTokens).toBe(40000);
    expect(entry.temperature).toBe(0.7);
    expect(entry.baseName).toBe('');
  });

  it('normalizeAgentList 丢弃无 chineseName 的条目并保持顺序', () => {
    const list = normalizeAgentList([
      { modelId: 'x' },
      { chineseName: '  ', modelId: 'x' },
      { chineseName: '小冰', modelId: 'default' },
    ]);
    expect(list.map((entry) => entry.chineseName)).toEqual(['小冰']);
  });

  it('normalizeGlobalConfig 应用后端默认值', () => {
    const global = normalizeGlobalConfig({});
    expect(global.maxHistoryRounds).toBe(7);
    expect(global.contextTtlHours).toBe(24);
    expect(global.delegationMaxRounds).toBe(15);
    expect(global.delegationTimeout).toBe(300000);
  });
});

describe('Agent 管理 · 草稿模型', () => {
  it('draftToAgentObject 透传未知字段（extras 铺底，已知键覆盖）', () => {
    const entry = normalizeAgentEntry(baseConfig().agents[0])!;
    const draft = draftFromAgent(entry);
    draft.description = '改过的描述';
    const object = draftToAgentObject(draft);
    expect(object.customField).toBe('preserved');
    expect(object.description).toBe('改过的描述');
    expect(object.chineseName).toBe('小娜');
  });

  it('validateAgentDraft 强制 chineseName 非空 + 唯一 + modelId 非空', () => {
    const draft = emptyAgentDraft();
    expect(validateAgentDraft(draft, [])).toContain('chineseName');

    draft.chineseName = '小娜';
    expect(validateAgentDraft(draft, ['小娜'])).toContain('重复');

    expect(validateAgentDraft(draft, [])).toContain('modelId');

    draft.modelId = 'gpt-4o';
    expect(validateAgentDraft(draft, [])).toBeNull();

    draft.temperature = 2.5;
    expect(validateAgentDraft(draft, [])).toContain('temperature');
  });

  it('collectTaskReferences 精确匹配目标 Agent（randomN 不误匹配）', () => {
    const taskConfig = {
      config: {
        tasks: [
          { name: '晨间巡航', targets: { agents: ['小娜', 'random2'] } },
          { name: '无关任务', targets: { agents: ['小冰'] } },
          { name: '无目标', targets: {} },
        ],
      },
    };
    expect(collectTaskReferences(taskConfig, '小娜')).toEqual(['晨间巡航']);
    // 精确匹配：前缀/子串不命中
    expect(collectTaskReferences(taskConfig, '小')).toEqual([]);
    expect(collectTaskReferences(taskConfig, '小娜2')).toEqual([]);
    expect(collectTaskReferences(taskConfig, '不存在')).toEqual([]);
  });
});

describe('Agent 管理 · Store 读写流', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    clearInvokeMocks();
  });

  it('loadConfig 应用 agents 与全局字段', async () => {
    mockInvoke('agentmgr_get_config', () => baseConfig());
    const store = useAgentMgrStore();
    await store.loadConfig();

    expect(store.configLoaded).toBe(true);
    expect(store.agents.map((entry) => entry.chineseName)).toEqual(['小娜', '小冰']);
    expect(store.globalConfig.maxHistoryRounds).toBe(9);
    expect(store.globalConfig.delegationTimeout).toBe(600000);
    expect(store.error).toBeNull();
  });

  it('saveAgent 新建：整体回写 agents 数组并保留既有条目的 extras', async () => {
    let savedPayload: Record<string, unknown> = {};
    mockInvoke('agentmgr_get_config', () => baseConfig());
    mockInvoke('agentmgr_save_config', (args) => {
      savedPayload = (args?.config ?? {}) as Record<string, unknown>;
      return { success: true };
    });

    const store = useAgentMgrStore();
    await store.loadConfig();

    const draft = emptyAgentDraft();
    draft.chineseName = '新agent';
    draft.modelId = 'claude-3';
    const result = await store.saveAgent(draft);

    expect(result).toBe('ok');
    const agents = savedPayload.agents as Record<string, unknown>[];
    expect(agents).toHaveLength(3);
    expect(agents[0].customField).toBe('preserved');
    expect(agents[2]).toMatchObject({ chineseName: '新agent', modelId: 'claude-3' });
  });

  it('saveAgent 编辑：按 originalName 原位替换（改名场景）', async () => {
    let savedPayload: Record<string, unknown> = {};
    mockInvoke('agentmgr_get_config', () => baseConfig());
    mockInvoke('agentmgr_save_config', (args) => {
      savedPayload = (args?.config ?? {}) as Record<string, unknown>;
      return { success: true };
    });

    const store = useAgentMgrStore();
    await store.loadConfig();

    const entry = store.agents[1];
    const draft = draftFromAgent(entry);
    draft.chineseName = '小冰Pro';
    const result = await store.saveAgent(draft);

    expect(result).toBe('ok');
    const agents = savedPayload.agents as Record<string, unknown>[];
    expect(agents.map((agent) => agent.chineseName)).toEqual(['小娜', '小冰Pro']);
  });

  it('saveConfig 检测到他端变更时返回 conflict，force 后覆盖', async () => {
    let remote = baseConfig();
    mockInvoke('agentmgr_get_config', () => remote);
    mockInvoke('agentmgr_save_config', () => ({ success: true }));

    const store = useAgentMgrStore();
    await store.loadConfig();

    // 模拟他端在编辑期间新增了一个 Agent
    remote = {
      ...baseConfig(),
      agents: [...baseConfig().agents, { chineseName: '他端新增', modelId: 'x' }],
    };

    const draft = emptyAgentDraft();
    draft.chineseName = '本端新增';
    draft.modelId = 'y';

    expect(await store.saveAgent(draft)).toBe('conflict');
    expect(await store.saveAgent(draft, { force: true })).toBe('ok');
  });

  it('saveGlobalConfig 只提交全局键（不含 agents，服务端浅合并保留）', async () => {
    let savedPayload: Record<string, unknown> = {};
    mockInvoke('agentmgr_get_config', () => baseConfig());
    mockInvoke('agentmgr_save_config', (args) => {
      savedPayload = (args?.config ?? {}) as Record<string, unknown>;
      return { success: true };
    });

    const store = useAgentMgrStore();
    await store.loadConfig();

    const result = await store.saveGlobalConfig({
      ...store.globalConfig,
      maxHistoryRounds: 12,
    });

    expect(result).toBe('ok');
    expect(savedPayload.maxHistoryRounds).toBe(12);
    expect(savedPayload.agents).toBeUndefined();
  });

  it('deleteAgent 从数组移除后整体回写', async () => {
    let savedPayload: Record<string, unknown> = {};
    mockInvoke('agentmgr_get_config', () => baseConfig());
    mockInvoke('agentmgr_save_config', (args) => {
      savedPayload = (args?.config ?? {}) as Record<string, unknown>;
      return { success: true };
    });

    const store = useAgentMgrStore();
    await store.loadConfig();

    const result = await store.deleteAgent('小娜');
    expect(result).toBe('ok');
    const agents = savedPayload.agents as Record<string, unknown>[];
    expect(agents.map((agent) => agent.chineseName)).toEqual(['小冰']);
  });

  it('保存成功后失效任务调度中心的 agent 快选缓存', async () => {
    mockInvoke('agentmgr_get_config', () => baseConfig());
    mockInvoke('agentmgr_save_config', () => ({ success: true }));

    const { useTaskCenterStore } = await import('@/features/taskcenter/taskCenterStore');
    const taskCenter = useTaskCenterStore();
    taskCenter.agentsLoaded = true;

    const store = useAgentMgrStore();
    await store.loadConfig();
    await store.saveGlobalConfig({ ...store.globalConfig });

    expect(taskCenter.agentsLoaded).toBe(false);
  });
});
