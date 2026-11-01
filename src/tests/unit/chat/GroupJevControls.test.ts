import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { disposePinia, getActivePinia } from 'pinia';
import { nextTick, type Component } from 'vue';
import type { VueWrapper } from '@vue/test-utils';
import GroupInviteBar from '@/features/chat/components/GroupInviteBar.vue';
import InputEnhancer from '@/features/chat/InputEnhancer.vue';
import { useAssistantStore } from '@/core/stores/assistant';
import { useChatSessionStore } from '@/core/stores/chatSessionStore';
import { useChatHistoryStore } from '@/core/stores/chatHistoryStore';
import { useChatStreamStore } from '@/core/stores/chatStreamStore';
import { useSettingsStore } from '@/core/stores/settings';
import { useNotificationStore } from '@/core/stores/notification';
import type { ActiveGroupTurnDto, GroupTurnSnapshotDto } from '@/core/types/chat';
import type { GroupConfigDto } from '@/core/types/assistant';
import { mockInvoke, invokeMock, type MockChannel } from '@/tests/mocks/tauri';
import { mountWithPinia } from '@/tests/utils/mount';
import { flushPromises } from '@/tests/utils/flush';
import { makeConfigSnapshot, makeSettings } from '@/tests/helpers/config';
import { installWindow, mockWindowRead, windowPage } from '@/tests/helpers/chatWindow';

let wrapper: VueWrapper | undefined;
let output: MockChannel<unknown> | undefined;
const turn: ActiveGroupTurnDto = {
  ownerId: 'g1', ownerType: 'group', topicId: 't1', turnId: 'turn-1', mode: 'jev',
  phase: 'deciding', queuedAgentIds: [], currentAgentId: null, autonomousRounds: 1, revision: 1,
};

async function mountGroup(component: Component = GroupInviteBar) {
  wrapper = mountWithPinia(component, {
    global: {
      directives: { longpress: {}, guide: {} },
      stubs: { VcpAvatar: true, StagedAttachmentPreview: true, GroupStopAllButton: true },
    },
  }, () => {
    const assistant = useAssistantStore();
    assistant.agents = [{ id: 'a1', name: 'Nova', model: 'm' }, { id: 'a2', name: 'Luna', model: 'm' }] as any;
    assistant.groups = [{ id: 'g1', name: '自然群聊', members: ['a1', 'a2'], mode: 'jev' }] as any;
    installWindow(useChatSessionStore(), { id: 'g1', type: 'group' }, 't1');
    useSettingsStore().snapshot = makeConfigSnapshot(makeSettings());
  });
  await useChatHistoryStore().ensureWindow();
  return wrapper;
}

function emitTurn(snapshot: GroupTurnSnapshotDto, active = true, turnId = turn.turnId) {
  output?.emit({
    type: 'group_turn', messageId: '', consumerRevision: 10,
    context: { ownerId: 'g1', ownerType: 'group', topicId: 't1', turnId },
    chunk: { ...snapshot, active },
  });
}

beforeEach(() => {
  output = undefined;
  mockWindowRead([]);
  mockInvoke('reconcile_chat_window', () => windowPage([]));
  mockInvoke('reconnect_chat_streams', args => {
    output = args?.channel as MockChannel<unknown>;
    return { generations: [], activeRequests: [], groupTurns: [] };
  });
});

afterEach(() => {
  wrapper?.unmount();
  wrapper = undefined;
  const pinia = getActivePinia();
  if (pinia) disposePinia(pinia);
});

describe('JEV group controls', () => {
  it('saves a local mode change without sending stale shared fields and projects the returned effective mode', async () => {
    await mountGroup();
    const assistant = useAssistantStore();
    const baseline: GroupConfigDto = {
      id: 'g1', name: '编辑器打开时的名称', members: ['a1'], mode: 'sequential', chatMode: 'sequential',
      avatarCalculatedColor: null, memberTags: {}, groupPrompt: '旧群提示词', invitePrompt: '旧邀请提示词',
      useUnifiedModel: false, unifiedModel: null, tagMatchMode: 'strict', topics: [], createdAt: 1,
      enableContextMessageWindow: false, contextMessageWindowSize: 100,
      modeSettings: {
        sequential: { speakerOrder: ['a1'] },
        jev: { navigatorPrompt: '', speakerThreshold: 0.16, continueThreshold: 0.45, stopThreshold: 0.55,
          maxSpeakersPerRound: 3, maxAutonomousRounds: 12, historyWindow: 12, continueDebounceMs: 800,
          fallbackPolicy: 'stop', memberStyles: {} },
      },
    };
    // 同步在设置页打开之后已经更新共享字段，保存本机选择不能把旧草稿回写。
    const current = { ...baseline, name: '同步后的名称', members: ['a1', 'a2'], mode: 'invite_only', groupPrompt: '同步后的提示词' };
    mockInvoke('update_group_config', args => ({ ...current, ...(args?.updates as object) }));
    const result = await assistant.saveGroup({ ...baseline, chatMode: 'jev' }, baseline);
    expect(invokeMock).toHaveBeenCalledWith('update_group_config', { groupId: 'g1', updates: { chatMode: 'jev' } });
    expect(invokeMock.mock.calls.some(([command]) => command === 'save_group_config')).toBe(false);
    expect(result).toMatchObject({ name: '同步后的名称', mode: 'invite_only', chatMode: 'jev', groupPrompt: '同步后的提示词' });
    expect(assistant.groups.find(group => group.id === 'g1')).toMatchObject({ name: '同步后的名称', members: ['a1', 'a2'], mode: 'jev' });
  });

  it('restores arbitration and queue state without a message, keeping a newer event ahead of the inventory', async () => {
    let resolveReceipt!: (receipt: unknown) => void;
    mockInvoke('reconnect_chat_streams', args => {
      output = args?.channel as MockChannel<unknown>;
      return new Promise(resolve => { resolveReceipt = resolve; });
    });
    const view = await mountGroup();
    const stream = useChatStreamStore();
    const connecting = stream.checkAndRecoverInterruptedStreams();
    emitTurn({ ...turn, revision: 3, queuedAgentIds: ['a2'] });
    resolveReceipt({ generations: [], activeRequests: [], groupTurns: [turn] });
    await connecting;
    await nextTick();
    expect(view.get('[role="status"]').text()).toBe('正在安排发言');
    expect(view.get('[aria-label="Luna等待发言"]').attributes('disabled')).toBeDefined();
    expect(stream.activeStreamMessages.size).toBe(0);
    expect(stream.isGroupGenerating).toBe(true);
    emitTurn({ ...turn, revision: 2, queuedAgentIds: [] });
    emitTurn({ revision: 99 }, false, 'old-turn');
    expect(stream.currentGroupTurn?.queuedAgentIds).toEqual(['a2']);
    emitTurn({ ...turn, phase: 'idle', revision: 4 }, false);
    expect(stream.currentGroupTurn).toBeNull();
    expect(stream.isGroupGenerating).toBe(false);
  });

  it('uses the reconnect inventory to restore a running speaker when no event arrives', async () => {
    mockInvoke('reconnect_chat_streams', () => ({ generations: [], activeRequests: [], groupTurns: [{
      ...turn, phase: 'speaking', currentAgentId: 'a1', queuedAgentIds: ['a2'],
    }] }));
    const view = await mountGroup();
    await useChatStreamStore().checkAndRecoverInterruptedStreams();
    await nextTick();
    expect(view.get('[role="status"]').text()).toBe('Nova 正在发言');
    expect(view.get('[aria-label="Luna等待发言"]').attributes('disabled')).toBeDefined();
  });

  it('reports the actual session failure once, keeping normal stops quiet', async () => {
    await mountGroup();
    await useChatStreamStore().checkAndRecoverInterruptedStreams();
    const notifications = useNotificationStore();
    emitTurn(turn);
    emitTurn({ ...turn, phase: 'idle', revision: 2, stopReason: 'cancelled' }, false);
    expect(notifications.activeToasts).toHaveLength(0);
    emitTurn(turn, true, 'failed-turn');
    const failure = { ...turn, phase: 'idle' as const, revision: 2, stopReason: 'error', error: '决策服务拒绝了请求（401）' };
    emitTurn(failure, false, 'failed-turn');
    emitTurn(failure, false, 'failed-turn');
    expect(notifications.activeToasts).toEqual([expect.objectContaining({ type: 'error', message: failure.error })]);
    expect(notifications.historyList).toHaveLength(1);
  });

  it('explains a network fallback stop once without surfacing internal codes or replaying notices', async () => {
    const view = await mountGroup();
    await useChatStreamStore().checkAndRecoverInterruptedStreams();
    const notifications = useNotificationStore();
    emitTurn(turn);
    const stopped = { ...turn, phase: 'idle' as const, revision: 2, stopReason: 'network_fallback_stop' };
    emitTurn(stopped, false);
    emitTurn(stopped, false);
    await nextTick();

    expect(useChatStreamStore().isGroupGenerating).toBe(false);
    expect(view.find('[aria-label="发起群聊"]').exists()).toBe(true);
    expect(notifications.activeToasts).toEqual([expect.objectContaining({
      type: 'info', message: '网络异常，已结束群聊',
    })]);
    expect(notifications.historyList).toHaveLength(1);
    output?.emit({
      type: 'group_turn', messageId: '', consumerRevision: 10,
      context: { ownerId: 'g1', ownerType: 'group', topicId: 't1', turnId: 'replayed-turn', replayed: true },
      chunk: { ...stopped, active: false },
    });
    expect(notifications.activeToasts).toHaveLength(1);
    expect(notifications.historyList).toHaveLength(1);
  });

  it('submits start and continue to the Rust owner without inventing a user message', async () => {
    mockInvoke('control_group_session', () => ({ status: 'accepted' }));
    const view = await mountGroup();
    for (const action of [{ label: '发起群聊', value: 'start' }, { label: '继续群聊', value: 'continue' }]) {
      await view.findAll('button').find(button => button.text().includes(action.label))!.trigger('click');
      await flushPromises();
      expect(invokeMock).toHaveBeenCalledWith('control_group_session', expect.objectContaining({
        payload: { groupId: 'g1', topicId: 't1', action: action.value },
      }));
    }
    expect(invokeMock.mock.calls.some(([command]) => command === 'append_single_message')).toBe(false);
  });

  it('queues a member while arbitration is active and uses the owner snapshot to acknowledge it', async () => {
    mockInvoke('invite_group_member_to_speak', () => ({ status: 'accepted' }));
    const view = await mountGroup();
    await useChatStreamStore().checkAndRecoverInterruptedStreams();
    emitTurn(turn);
    await nextTick();
    await view.get('[aria-label="邀请Nova发言"]').trigger('click');
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith('invite_group_member_to_speak', expect.objectContaining({
      payload: { groupId: 'g1', topicId: 't1', agentIds: ['a1'] },
    }));
    expect(view.find('[aria-label="Nova等待发言"]').exists()).toBe(false);
    emitTurn({ ...turn, revision: 2, queuedAgentIds: ['a1'] });
    await nextTick();
    expect(view.get('[aria-label="Nova等待发言"]').attributes('disabled')).toBeDefined();
  });

  it('sends a new draft during arbitration and keeps empty-draft stop as an interrupt of the whole group', async () => {
    const view = await mountGroup(InputEnhancer);
    const stream = useChatStreamStore();
    await stream.checkAndRecoverInterruptedStreams();
    emitTurn(turn);
    await nextTick();
    await view.get('textarea').setValue('补充一个问题');
    await view.get('[aria-label="发送消息"]').trigger('click');
    expect(view.emitted('send')).toEqual([['补充一个问题', expect.any(Function)]]);
    expect(invokeMock.mock.calls.some(([command]) => command === 'interruptGroupTurn')).toBe(false);
    await view.get('textarea').setValue('通话开始前的草稿');
    await view.setProps({ suspended: true });
    await view.get('textarea').trigger('keydown', { key: 'Enter', ctrlKey: true });
    expect(view.emitted('send')).toEqual([['补充一个问题', expect.any(Function)]]);
    expect(invokeMock.mock.calls.some(([command]) => command === 'interruptGroupTurn')).toBe(false);
    expect((view.get('textarea').element as HTMLTextAreaElement).value).toBe('通话开始前的草稿');
    await view.setProps({ suspended: false });
    await view.get('textarea').setValue('');
    await view.get('[aria-label="停止生成"]').trigger('click');
    expect(invokeMock).toHaveBeenCalledWith('interruptGroupTurn', { ownerId: 'g1', ownerType: 'group', topicId: 't1' });
    // IPC 接收不是完成事件；前端不能提前把仍在结束的会话抹掉。
    expect(stream.isGroupGenerating).toBe(true);
  });
});
