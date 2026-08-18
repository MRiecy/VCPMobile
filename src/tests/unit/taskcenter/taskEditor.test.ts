import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import {
  draftFromTask,
  draftToPayload,
  emptyDraft,
  normalizeDelegations,
  normalizeTask,
  validateDraft,
} from '@/features/taskcenter/taskTypes';
import { useTaskCenterStore } from '@/features/taskcenter/taskCenterStore';
import { mockInvoke } from '../../mocks/tauri';

const baseTask = {
  id: 'task_custom_prompt_demo_1',
  name: '晨间巡航',
  type: 'forum_patrol',
  enabled: true,
  schedule: { mode: 'interval', intervalMinutes: 60 },
  targets: { agents: ['艾米莉亚', 'random2'] },
  dispatch: { maid: 'VCP系统', taskDelegation: false },
  payload: {
    promptTemplate: '[论坛小助手:]……{{forum_post_list}}',
    includeForumPostList: true,
    forumListPlaceholder: '{{forum_post_list}}',
    maxPosts: 150,
  },
  runtime: {},
  meta: {},
};

describe('任务编辑器草稿模型', () => {
  it('draftFromTask 回填并把 randomN 拆为控件态', () => {
    const task = normalizeTask(baseTask)!;
    const draft = draftFromTask(task, baseTask.payload);
    expect(draft.id).toBe(baseTask.id);
    expect(draft.agents).toEqual(['艾米莉亚']);
    expect(draft.randomCount).toBe(2);
    expect(draft.promptTemplate).toContain('论坛小助手');
    expect(draft.maxPosts).toBe(150);
  });

  it('draftToPayload 把 randomN 控件态拼回 agents', () => {
    const task = normalizeTask(baseTask)!;
    const draft = draftFromTask(task, baseTask.payload);
    const payload = draftToPayload(draft);
    expect((payload.targets as { agents: string[] }).agents).toEqual(['艾米莉亚', 'random2']);
    expect((payload.schedule as { intervalMinutes: number }).intervalMinutes).toBe(60);
  });

  it('roundtrip：draftFromTask → draftToPayload 保持关键字段', () => {
    const task = normalizeTask(baseTask)!;
    const payload = draftToPayload(draftFromTask(task, baseTask.payload));
    expect(payload.name).toBe('晨间巡航');
    expect(payload.type).toBe('forum_patrol');
    expect((payload.payload as { maxPosts: number }).maxPosts).toBe(150);
    expect((payload.dispatch as { maid: string }).maid).toBe('VCP系统');
  });

  it('validateDraft 对齐后端 sanitizeTaskInput 规则', () => {
    const draft = emptyDraft();
    expect(validateDraft(draft)).toBe('任务名称不能为空');

    draft.name = '测试';
    expect(validateDraft(draft)).toContain('目标 Agent');

    draft.randomCount = 1;
    expect(validateDraft(draft)).toBe('通用提示词任务必须填写提示词模板');

    draft.promptTemplate = '你好';
    expect(validateDraft(draft)).toBeNull();

    draft.schedule.mode = 'once';
    expect(validateDraft(draft)).toBe('一次性任务必须指定执行时间');

    draft.schedule.mode = 'cron';
    draft.schedule.cronValue = '';
    expect(validateDraft(draft)).toBe('CRON 任务必须填写表达式');
  });

  it('draftToPayload 强制 interval 下限 10', () => {
    const draft = emptyDraft();
    draft.name = 'x';
    draft.agents = ['A'];
    draft.promptTemplate = 'p';
    draft.schedule.mode = 'interval';
    draft.schedule.intervalMinutes = 3;
    const payload = draftToPayload(draft);
    expect((payload.schedule as { intervalMinutes: number }).intervalMinutes).toBe(10);
  });
});

describe('异步委托归一化', () => {
  it('解析数组与包裹形态', () => {
    const list = normalizeDelegations({
      delegations: [
        {
          delegationId: 'del_1',
          agent_name: '艾米莉亚',
          status: 'running',
          created_at: '2026-08-18T10:00:00+08:00',
        },
      ],
    });
    expect(list).toHaveLength(1);
    expect(list[0]).toMatchObject({ id: 'del_1', agentName: '艾米莉亚', status: 'running' });
  });

  it('无 id 项被丢弃', () => {
    expect(normalizeDelegations([{ agentName: 'A' }, null, 'x'])).toEqual([]);
  });
});

describe('taskCenterStore S2b 写操作', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  const configPayload = () => ({
    config: { globalEnabled: true, tasks: [baseTask], history: [] },
  });

  it('saveTask 新建走 task_create 并重拉 config', async () => {
    const calls: string[] = [];
    mockInvoke('task_get_config', () => {
      calls.push('config');
      return configPayload();
    });
    mockInvoke('task_create', (args) => {
      calls.push('create');
      expect((args?.task as { name: string }).name).toBe('新任务');
      return { success: true };
    });

    const store = useTaskCenterStore();
    await store.startSession();
    const draft = emptyDraft();
    draft.name = '新任务';
    draft.agents = ['艾米莉亚'];
    draft.promptTemplate = '你好';
    const ok = await store.saveTask('', draftToPayload(draft));
    store.stopSession();

    expect(ok).toBe(true);
    expect(calls).toEqual(['config', 'create', 'config']);
  });

  it('saveTask 编辑走 task_update；deleteTask 走 task_delete', async () => {
    const calls: string[] = [];
    mockInvoke('task_get_config', () => configPayload());
    mockInvoke('task_update', (args) => {
      calls.push(`update:${args?.taskId}`);
      return { success: true };
    });
    mockInvoke('task_delete', (args) => {
      calls.push(`delete:${args?.taskId}`);
      return { success: true };
    });

    const store = useTaskCenterStore();
    await store.startSession();

    const draft = draftFromTask(store.tasks[0], store.rawPayloadById.get(baseTask.id));
    draft.name = '改名巡航';
    expect(await store.saveTask(baseTask.id, draftToPayload(draft))).toBe(true);
    expect(await store.deleteTask(baseTask.id)).toBe(true);
    store.stopSession();

    expect(calls).toContain(`update:${baseTask.id}`);
    expect(calls).toContain(`delete:${baseTask.id}`);
  });

  it('saveTask 失败返回 false 且不重拉', async () => {
    let configCalls = 0;
    mockInvoke('task_get_config', () => {
      configCalls += 1;
      return configPayload();
    });
    mockInvoke('task_create', () => Promise.reject(new Error('任务名称不能为空')));

    const store = useTaskCenterStore();
    await store.startSession();
    const ok = await store.saveTask('', { name: '' });
    store.stopSession();

    expect(ok).toBe(false);
    expect(configCalls).toBe(1);
  });

  it('cancelDelegation 调用取消端点并刷新列表', async () => {
    const calls: string[] = [];
    mockInvoke('task_get_config', () => configPayload());
    mockInvoke('delegation_list', () => {
      calls.push('list');
      return { delegations: [] };
    });
    mockInvoke('delegation_cancel', (args) => {
      calls.push(`cancel:${args?.delegationId}`);
      return { success: true };
    });

    const store = useTaskCenterStore();
    await store.startSession();
    await store.cancelDelegation('del_9');
    store.stopSession();

    expect(calls).toEqual(['cancel:del_9', 'list']);
  });
});
