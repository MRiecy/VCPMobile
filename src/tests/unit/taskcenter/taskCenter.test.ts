import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import {
  formatDuration,
  mergeRuntimeIntoTasks,
  normalizeHistory,
  normalizeTask,
  normalizeTaskList,
  scheduleSummary,
  splitRandomTag,
} from '@/features/taskcenter/taskTypes';
import {
  PLUGIN_UNAVAILABLE_PREFIX,
  useTaskCenterStore,
} from '@/features/taskcenter/taskCenterStore';
import { mockInvoke } from '../../mocks/tauri';

const baseTask = {
  id: 'task_custom_prompt_demo_1',
  name: '晨间巡航',
  type: 'forum_patrol',
  enabled: true,
  schedule: { mode: 'interval', intervalMinutes: 60 },
  targets: { agents: ['艾米莉亚', 'random2'] },
  dispatch: { maid: 'VCP系统', taskDelegation: false },
  payload: {},
  runtime: { running: false, runCount: 3, successCount: 2, errorCount: 1 },
  meta: {},
};

describe('taskTypes 归一化', () => {
  it('normalizeTask 应用默认值并保留字段', () => {
    const task = normalizeTask(baseTask);
    expect(task).not.toBeNull();
    expect(task!.name).toBe('晨间巡航');
    expect(task!.schedule.intervalMinutes).toBe(60);
    expect(task!.runtime.runCount).toBe(3);
    expect(task!.maid).toBe('VCP系统');
  });

  it('normalizeTask 非法 type/mode 回落，interval 下限 10', () => {
    const task = normalizeTask({
      ...baseTask,
      type: 'unknown_type',
      schedule: { mode: 'weird', intervalMinutes: 3 },
    });
    expect(task!.type).toBe('forum_patrol');
    expect(task!.schedule.mode).toBe('interval');
    expect(task!.schedule.intervalMinutes).toBe(10);
  });

  it('normalizeTask 缺 id 返回 null；列表过滤空项', () => {
    expect(normalizeTask({ ...baseTask, id: '' })).toBeNull();
    expect(normalizeTaskList([baseTask, null, { id: '' }])).toHaveLength(1);
  });

  it('mergeRuntimeIntoTasks 按 id 合并 runtime 与 enabled', () => {
    const tasks = normalizeTaskList([baseTask]);
    const merged = mergeRuntimeIntoTasks(tasks, [
      {
        id: baseTask.id,
        enabled: false,
        runtime: { running: true, runCount: 4, lastResult: 'running via scheduler' },
      },
    ]);
    expect(merged[0].enabled).toBe(false);
    expect(merged[0].runtime.running).toBe(true);
    expect(merged[0].runtime.runCount).toBe(4);
    // 未命中 id 的任务保持原样
    const untouched = mergeRuntimeIntoTasks(tasks, [{ id: 'other' }]);
    expect(untouched[0].enabled).toBe(true);
  });

  it('splitRandomTag 解析 randomN 魔法标签', () => {
    expect(splitRandomTag(['艾米莉亚', 'random2'])).toEqual({
      agents: ['艾米莉亚'],
      randomCount: 2,
    });
    expect(splitRandomTag(['艾米莉亚'])).toEqual({ agents: ['艾米莉亚'], randomCount: null });
  });

  it('scheduleSummary 覆盖四种模式', () => {
    expect(scheduleSummary(normalizeTask(baseTask)!)).toBe('每 60 分钟');
    expect(
      scheduleSummary(normalizeTask({ ...baseTask, schedule: { mode: 'manual' } })!),
    ).toBe('仅手动触发');
    expect(
      scheduleSummary(
        normalizeTask({ ...baseTask, schedule: { mode: 'cron', cronValue: '0 9 * * *' } })!,
      ),
    ).toBe('CRON 0 9 * * *');
    expect(
      scheduleSummary(
        normalizeTask({ ...baseTask, schedule: { mode: 'once', runAt: '2026-08-20T09:00:00+08:00' } })!,
      ),
    ).toContain('定时');
  });

  it('normalizeHistory 状态映射与非法状态回落', () => {
    const records = normalizeHistory([
      {
        id: 'run_1',
        taskId: 't1',
        taskName: '巡航',
        triggerSource: 'manual-trigger',
        startedAt: '2026-08-18T10:00:00+08:00',
        finishedAt: '2026-08-18T10:00:30+08:00',
        durationMs: 30000,
        status: 'partial_success',
        agents: ['艾米莉亚'],
        message: 'ok',
      },
      { id: 'run_2', status: 'weird' },
    ]);
    expect(records[0].status).toBe('partial_success');
    expect(records[1].status).toBe('success');
  });

  it('formatDuration 格式化', () => {
    expect(formatDuration(null)).toBe('—');
    expect(formatDuration(500)).toBe('500ms');
    expect(formatDuration(34000)).toBe('34.0s');
    expect(formatDuration(95000)).toBe('1m35s');
  });
});

describe('taskCenterStore', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  const configPayload = (overrides: Record<string, unknown> = {}) => ({
    config: {
      globalEnabled: true,
      settings: { maxHistory: 200 },
      tasks: [baseTask],
      history: [],
      ...overrides,
    },
    availableTaskTypes: [],
    taskTemplates: {},
  });

  it('首次轮询拉 config，之后拉 status 并合并 runtime', async () => {
    const calls: string[] = [];
    mockInvoke('task_get_config', () => {
      calls.push('config');
      return configPayload();
    });
    mockInvoke('task_get_status', () => {
      calls.push('status');
      return {
        globalEnabled: true,
        activeTimerCount: 1,
        tasks: [
          { id: baseTask.id, runtime: { running: true, runCount: 4 } },
        ],
        history: [],
      };
    });

    const store = useTaskCenterStore();
    await store.startSession();
    expect(calls).toEqual(['config']);
    expect(store.tasks[0].name).toBe('晨间巡航');

    await store.pollOnce();
    store.stopSession();
    expect(calls).toEqual(['config', 'status']);
    expect(store.tasks[0].runtime.running).toBe(true);
    expect(store.tasks[0].runtime.runCount).toBe(4);
  });

  it('setTaskEnabled 乐观更新，失败回滚', async () => {
    mockInvoke('task_get_config', () => configPayload());
    // 注意：同步 throw 会让 invoke 在等待前就完成回滚，无法观察乐观更新中间态；
    // 返回 rejected Promise 才能模拟真实异步失败。
    mockInvoke('task_set_enabled', () => Promise.reject(new Error('server rejected')));

    const store = useTaskCenterStore();
    await store.startSession();
    store.stopSession();

    const promise = store.setTaskEnabled(baseTask.id, false);
    // 乐观更新：调用未返回时 UI 已切换
    expect(store.tasks[0].enabled).toBe(false);
    await promise;
    // 失败回滚
    expect(store.tasks[0].enabled).toBe(true);
  });

  it('triggerTask 进行 in-flight 去重', async () => {
    mockInvoke('task_get_config', () => configPayload());
    let triggerCalls = 0;
    let releaseTrigger: (value: unknown) => void = () => {};
    mockInvoke(
      'task_trigger',
      () =>
        new Promise((resolve) => {
          triggerCalls += 1;
          releaseTrigger = resolve;
        }),
    );
    mockInvoke('task_get_status', () => ({
      globalEnabled: true,
      activeTimerCount: 1,
      tasks: [],
      history: [],
    }));

    const store = useTaskCenterStore();
    await store.startSession();
    store.stopSession();

    const first = store.triggerTask(baseTask.id);
    const second = store.triggerTask(baseTask.id);
    expect(store.triggeringIds.has(baseTask.id)).toBe(true);
    releaseTrigger({ message: 'ok' });
    await Promise.all([first, second]);
    expect(triggerCalls).toBe(1);
    expect(store.triggeringIds.size).toBe(0);
  });

  it('setGlobalEnabled 失败回滚', async () => {
    mockInvoke('task_get_config', () => configPayload());
    mockInvoke('task_set_global_enabled', () => {
      throw new Error('forbidden');
    });

    const store = useTaskCenterStore();
    await store.startSession();
    store.stopSession();
    expect(store.globalEnabled).toBe(true);

    await store.setGlobalEnabled(false);
    expect(store.globalEnabled).toBe(true);
  });

  it('PLUGIN_UNAVAILABLE 前缀识别为插件未加载态', async () => {
    mockInvoke('task_get_config', () => {
      throw new Error(`${PLUGIN_UNAVAILABLE_PREFIX}VCPTaskAssistant 插件未加载`);
    });

    const store = useTaskCenterStore();
    await store.startSession();
    store.stopSession();
    expect(store.pluginUnavailable).toBe(true);
    expect(store.error).toBe('VCPTaskAssistant 插件未加载');
  });
});
