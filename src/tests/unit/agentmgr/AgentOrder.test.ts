import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { useAssistantStore } from '@/core/stores/assistant';
import { useSettingsStore } from '@/core/stores/settings';
import { useAgentOrderStore } from '@/features/agent/agentOrderStore';
import { sortByOrder } from '@/features/agent/agentOrder';
import type { ConfigSaveResult, ConfigSnapshot, ConfigUpdate } from '@/core/config/generated/config';
import { applyConfigRequest, makeConfigSnapshot, makeSettings } from '../../helpers/config';
import { mockInvoke } from '../../mocks/tauri';

const { notify } = vi.hoisted(() => ({ notify: vi.fn() }));
vi.mock('@/core/stores/notification', () => ({
  useNotificationStore: () => ({ addNotification: notify }),
}));

interface Write {
  request: ConfigUpdate;
  resolve(result: ConfigSaveResult): void;
  reject(error: Error): void;
}
let pinia: Pinia;
let persisted: ConfigSnapshot;
let writes: Write[];
const agents = (ids: string[]) => ids.map(id => ({ id, name: id, model: 'test', avatarCalculatedColor: null }));
const displayed = () => useAgentOrderStore().orderedAgents.map(item => item.id);
const commit = (index: number) => {
  persisted = applyConfigRequest(persisted, writes[index].request);
  writes[index].resolve({ status: 'saved', snapshot: persisted });
};
const settle = async () => { for (let i = 0; i < 20; i += 1) await Promise.resolve(); };

beforeEach(async () => {
  vi.useFakeTimers();
  pinia = createPinia();
  setActivePinia(pinia);
  writes = [];
  notify.mockClear();
  persisted = makeConfigSnapshot(makeSettings({ 'navigation.agentOrder': ['a', 'b', 'c'] }));
  mockInvoke('read_settings', () => persisted);
  mockInvoke('update_settings', args => new Promise<ConfigSaveResult>((resolve, reject) => {
    writes.push({ request: args!.request as ConfigUpdate, resolve, reject });
  }));
  useAssistantStore().agents = agents(['a', 'b', 'c']);
  await useSettingsStore().fetchSettings();
});

afterEach(() => {
  disposePinia(pinia);
  vi.useRealTimers();
});

describe('debounced assistant order persistence', () => {
  it('shows every drag immediately but saves only the last order after 300 quiet milliseconds', async () => {
    const order = useAgentOrderStore();
    order.setOrder('agent', ['b', 'a', 'c']);
    expect(displayed()).toEqual(['b', 'a', 'c']);
    await vi.advanceTimersByTimeAsync(250);
    order.setOrder('agent', ['b', 'c', 'a']);
    expect(displayed()).toEqual(['b', 'c', 'a']);
    await vi.advanceTimersByTimeAsync(299);
    expect(writes).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(1);
    expect(writes).toHaveLength(1);
    expect(writes[0].request.patches[0].changes).toEqual({ agentOrder: ['b', 'c', 'a'] });
    commit(0);
    await settle();
    expect(useSettingsStore().settings!.navigation.agentOrder).toEqual(['b', 'c', 'a']);
    expect(displayed()).toEqual(['b', 'c', 'a']);
  });

  it('keeps the newest draft through an older acknowledgement and builds its request from the confirmed base', async () => {
    const order = useAgentOrderStore();
    order.setOrder('agent', ['b', 'a', 'c']);
    await vi.advanceTimersByTimeAsync(300);
    order.setOrder('agent', ['b', 'c', 'a']);
    await vi.advanceTimersByTimeAsync(100);
    order.setOrder('agent', ['c', 'b', 'a']);
    await vi.advanceTimersByTimeAsync(300);
    expect(writes).toHaveLength(1);
    commit(0);
    await settle();
    expect(displayed()).toEqual(['c', 'b', 'a']);
    expect(writes).toHaveLength(2);
    expect(writes[1].request.patches[0]).toMatchObject({
      expectedRevision: '2', baseValues: { agentOrder: ['b', 'a', 'c'] },
      changes: { agentOrder: ['c', 'b', 'a'] },
    });
    commit(1);
    await settle();
    expect(useSettingsStore().settings!.navigation.agentOrder).toEqual(['c', 'b', 'a']);
  });

  it('does not let an old acknowledgement shorten the latest drag debounce', async () => {
    const order = useAgentOrderStore();
    order.setOrder('agent', ['b', 'a', 'c']);
    await vi.advanceTimersByTimeAsync(300);
    order.setOrder('agent', ['c', 'b', 'a']);
    await vi.advanceTimersByTimeAsync(100);
    commit(0);
    await settle();
    expect(displayed()).toEqual(['c', 'b', 'a']);
    await vi.advanceTimersByTimeAsync(199);
    expect(writes).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(writes).toHaveLength(2);
    commit(1);
    await settle();
  });

  it('removes deleted entities and appends newly loaded entities to pending intent', async () => {
    const order = useAgentOrderStore();
    order.setOrder('agent', ['c', 'a', 'b']);
    useAssistantStore().agents = agents(['b', 'c', 'd']);
    expect(displayed()).toEqual(['c', 'b', 'd']);
    await vi.advanceTimersByTimeAsync(300);
    expect(writes[0].request.patches[0].changes).toEqual({ agentOrder: ['c', 'b', 'd'] });
    useAssistantStore().agents = agents(['c', 'd']);
    await vi.advanceTimersByTimeAsync(300);
    commit(0);
    await settle();
    expect(displayed()).toEqual(['c', 'd']);
    expect(writes[1].request.patches[0]).toMatchObject({
      expectedRevision: '2', baseValues: { agentOrder: ['c', 'b', 'd'] },
      changes: { agentOrder: ['c', 'd'] },
    });
    commit(1);
    await settle();
  });

  it('does not postpone saving for an inventory refresh that only changes labels', async () => {
    const order = useAgentOrderStore();
    order.setOrder('agent', ['c', 'a', 'b']);
    await vi.advanceTimersByTimeAsync(250);
    useAssistantStore().agents = agents(['a', 'b', 'c']).map(item => ({ ...item, name: `${item.name} updated` }));
    await vi.advanceTimersByTimeAsync(50);
    expect(writes).toHaveLength(1);
    commit(0);
    await settle();
  });

  it('keeps agent and group intent independent while using the settings Store queue', async () => {
    useAssistantStore().groups = ['g1', 'g2'].map(id => ({ id, name: id, members: [], mode: 'sequential', avatarCalculatedColor: null }));
    const order = useAgentOrderStore();
    order.setOrder('agent', ['b', 'a', 'c']);
    order.setOrder('group', ['g2', 'g1']);
    expect(order.orderedGroups.map(item => item.id)).toEqual(['g2', 'g1']);
    await vi.advanceTimersByTimeAsync(300);
    expect(writes).toHaveLength(1);
    commit(0);
    await settle();
    expect(writes).toHaveLength(2);
    expect(writes[1].request.patches[0].changes).toEqual({ groupOrder: ['g2', 'g1'] });
    commit(1);
    await settle();
    expect(displayed()).toEqual(['b', 'a', 'c']);
    expect(useSettingsStore().settings!.navigation.groupOrder).toEqual(['g2', 'g1']);
  });

  it('reports a failed save, retains the draft, and retries only after another drag', async () => {
    const order = useAgentOrderStore();
    order.setOrder('agent', ['b', 'a', 'c']);
    await vi.advanceTimersByTimeAsync(300);
    writes[0].reject(new Error('disk full'));
    await settle();
    expect(displayed()).toEqual(['b', 'a', 'c']);
    expect(notify).toHaveBeenCalledWith(expect.objectContaining({ type: 'error', title: '排序未保存' }));
    await vi.advanceTimersByTimeAsync(1000);
    expect(writes).toHaveLength(1);
    order.setOrder('agent', ['b', 'c', 'a']);
    await vi.advanceTimersByTimeAsync(300);
    expect(writes[1].request.patches[0]).toMatchObject({
      expectedRevision: '1', baseValues: { agentOrder: ['a', 'b', 'c'] },
      changes: { agentOrder: ['b', 'c', 'a'] },
    });
    commit(1);
    await settle();
  });
});

describe('rank-map ordering', () => {
  it('preserves first duplicate rank, unknown-item stability and the input arrays', () => {
    const items = agents(['u1', 'b', 'a', 'u2', 'u3']);
    const order = ['a', 'b', 'a', 'missing'];
    expect(sortByOrder(items, order).map(item => item.id)).toEqual(['a', 'b', 'u1', 'u2', 'u3']);
    expect(items.map(item => item.id)).toEqual(['u1', 'b', 'a', 'u2', 'u3']);
    expect(order).toEqual(['a', 'b', 'a', 'missing']);
    expect(sortByOrder(items, [])).toBe(items);
  });
});
