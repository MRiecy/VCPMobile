import { describe, expect, it } from 'vitest';
import { nextTick } from 'vue';
import { mockInvoke } from '../../mocks/tauri';
import GroupInviteBar from '../../../features/chat/components/GroupInviteBar.vue';
import { useAssistantStore } from '../../../core/stores/assistant';
import { useChatSessionStore } from '../../../core/stores/chatSessionStore';
import { useChatStreamStore } from '../../../core/stores/chatStreamStore';
import { mountWithPinia } from '../../utils/mount';
import type { ConversationOwnerItem } from '../../../core/types/assistant';

// 注意：mountWithPinia 内部自建 pinia 实例，store 数据必须在挂载后写入
const mountBar = async (
  item: ConversationOwnerItem | null,
  options?: { generating?: boolean },
) => {
  const wrapper = mountWithPinia(GroupInviteBar);

  const assistantStore = useAssistantStore();
  assistantStore.agents = [
    { id: 'a1', name: 'Nova', model: 'm' },
    { id: 'a2', name: 'Luna', model: 'm' },
  ] as any;
  assistantStore.groups = [
    { id: 'g1', name: '邀约群', members: ['a1', 'a2'], mode: 'invite_only' },
    { id: 'g2', name: '顺序群', members: ['a1'], mode: 'sequential' },
  ] as any;

  const sessionStore = useChatSessionStore();
  sessionStore.currentSelectedItem = item;

  if (options?.generating && item) {
    sessionStore.currentTopicId = 't1';
    const streamStore = useChatStreamStore();
    streamStore.addSessionStream(item.id, 'group', 't1', 'msg-1');
  }

  await nextTick();
  return wrapper;
};

describe('GroupInviteBar', () => {
  it('is visible only for invite_only groups and lists members', async () => {
    const wrapper = await mountBar({ id: 'g1', type: 'group', name: '邀约群' });
    const buttons = wrapper.findAll('button');
    expect(buttons).toHaveLength(2);
    expect(wrapper.text()).toContain('Nova');
    expect(wrapper.text()).toContain('Luna');
  });

  it('stays hidden for sequential groups and agent chats', async () => {
    const wrapper = await mountBar({ id: 'g2', type: 'group', name: '顺序群' });
    expect(wrapper.findAll('button')).toHaveLength(0);

    const wrapper2 = await mountBar({ id: 'a1', type: 'agent', name: 'Nova' });
    expect(wrapper2.findAll('button')).toHaveLength(0);
  });

  it('is disabled while a group turn is generating', async () => {
    const wrapper = await mountBar(
      { id: 'g1', type: 'group', name: '邀约群' },
      { generating: true },
    );
    const first = wrapper.find('button');
    expect(first.exists()).toBe(true);
    expect(first.attributes('disabled')).toBeDefined();
  });

  it('saveGroup syncs mode into the groups snapshot (invite bar data source)', async () => {
    // 回归契约：群设置保存后 mode 必须进入 assistantStore.groups 快照，
    // 否则邀约横条/@选择器永远读到旧模式（本 bug 的血训）
    mockInvoke('save_group_config', (args) => args?.group);
    const wrapper = await mountBar({ id: 'g2', type: 'group', name: '顺序群' });
    expect(wrapper.findAll('button')).toHaveLength(0);

    const assistantStore = useAssistantStore();
    await assistantStore.saveGroup({
      id: 'g2',
      name: '顺序群',
      members: ['a1'],
      mode: 'invite_only',
    } as any);
    await nextTick();

    expect(assistantStore.groups.find((g) => g.id === 'g2')?.mode).toBe('invite_only');
    expect(wrapper.findAll('button')).toHaveLength(1);
  });
});
