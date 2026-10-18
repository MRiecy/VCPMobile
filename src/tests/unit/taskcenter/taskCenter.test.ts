import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
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
  payload: {},
  runtime: { running: false, runCount: 3, successCount: 2, errorCount: 1 },
  meta: {},
};


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

});
