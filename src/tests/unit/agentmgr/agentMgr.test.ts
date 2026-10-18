import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { draftFromAgent, emptyAgentDraft } from '@/features/agentmgr/agentMgrTypes';
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



describe('Agent 管理 · Store 读写流', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    clearInvokeMocks();
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
