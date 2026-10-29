import { defineStore } from 'pinia';
import { computed, onScopeDispose, shallowRef, watch } from 'vue';
import { useAssistantStore } from '../../core/stores/assistant';
import { useSettingsStore } from '../../core/stores/settings';
import { useNotificationStore } from '../../core/stores/notification';
import { sortByOrder } from './agentOrder';

const SAVE_DELAY_MS = 300;
type OwnerType = 'agent' | 'group';
const sameOrder = (left: string[], right: string[]) =>
  left.length === right.length && left.every((id, index) => id === right[index]);

/** Owns the latest drag intent across sidebar tab switches and list remounts. */
export const useAgentOrderStore = defineStore('agentOrder', () => {
  const assistants = useAssistantStore();
  const settings = useSettingsStore();
  const notifications = useNotificationStore();

  function createOrder<T extends { id: string }>(type: OwnerType, items: () => T[]) {
    const field = type === 'agent' ? 'navigation.agentOrder' : 'navigation.groupOrder';
    const draft = shallowRef<string[] | null>(null);
    const committed = () => type === 'agent'
      ? settings.settings?.navigation.agentOrder ?? []
      : settings.settings?.navigation.groupOrder ?? [];
    const ordered = computed(() => sortByOrder(items(), draft.value ?? committed()));
    let timer: ReturnType<typeof setTimeout> | null = null;
    let ready = false;
    let saving = false;
    let disposed = false;

    const save = async () => {
      if (disposed || saving || !ready || !draft.value) return;
      ready = false;
      saving = true;
      const submitted = draft.value;
      try {
        // Build the request only after our preceding save has been acknowledged.
        // The settings Store then supplies its current revision and field base.
        await settings.updateFields({ [field]: submitted });
        if (draft.value === submitted) draft.value = null;
      } catch {
        if (!disposed) notifications.addNotification({
          type: 'error', title: '排序未保存', toastOnly: true,
          message: `${type === 'agent' ? '助手' : '群组'}当前顺序已保留，请再次拖动重试。`,
        });
      } finally {
        saving = false;
        // A timer may have elapsed during the request. There is only one latest
        // draft to send, never a queue of intermediate drag positions.
        if (ready && !disposed) void save();
      }
    };

    const stage = (order: string[]) => {
      if (disposed) return;
      draft.value = order;
      ready = false;
      if (timer !== null) clearTimeout(timer);
      timer = setTimeout(() => {
        timer = null;
        ready = true;
        void save();
      }, SAVE_DELAY_MS);
    };

    const setOrder = (order: string[]) => {
      const next = sortByOrder(items(), order).map(item => item.id);
      // Also accepts the unchanged displayed order after a failed save as a retry.
      if (!draft.value && sameOrder(next, ordered.value.map(item => item.id))) return;
      stage(next);
    };

    watch(items, () => {
      if (!draft.value) return;
      // Deleted entities leave the pending order; new entities append in their
      // inventory order. Metadata-only refreshes must not restart the timer.
      const next = ordered.value.map(item => item.id);
      if (!sameOrder(next, draft.value)) stage(next);
    }, { flush: 'sync' });

    onScopeDispose(() => {
      disposed = true;
      if (timer !== null) clearTimeout(timer);
    });
    return { ordered, setOrder };
  }

  const agents = createOrder('agent', () => assistants.agents);
  const groups = createOrder('group', () => assistants.groups);
  const setOrder = (type: OwnerType, order: string[]) =>
    (type === 'agent' ? agents : groups).setOrder(order);

  return { orderedAgents: agents.ordered, orderedGroups: groups.ordered, setOrder };
});
