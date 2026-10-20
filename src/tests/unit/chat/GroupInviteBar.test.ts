import { describe, expect, it } from 'vitest';
import { nextTick } from 'vue';
import { mockInvoke } from '../../mocks/tauri';
import GroupInviteBar from '../../../features/chat/components/GroupInviteBar.vue';
import { useAssistantStore } from '../../../core/stores/assistant';
import { useChatSessionStore } from '../../../core/stores/chatSessionStore';
import { mountWithPinia } from '../../utils/mount';
import type { ConversationOwnerItem, GroupConfigDto } from '../../../core/types/assistant';

// 注意：mountWithPinia 内部自建 pinia 实例，store 数据必须在挂载后写入
const mountBar = async (
  item: ConversationOwnerItem | null,
) => {
  const wrapper = mountWithPinia(GroupInviteBar);

  const assistantStore = useAssistantStore();
  assistantStore.agents = [
    { id: 'a1', name: 'Nova', model: 'm', avatarCalculatedColor: null },
    { id: 'a2', name: 'Luna', model: 'm', avatarCalculatedColor: null },
  ];
  assistantStore.groups = [
    { id: 'g1', name: '邀约群', members: ['a1', 'a2'], mode: 'invite_only', avatarCalculatedColor: null },
    { id: 'g2', name: '顺序群', members: ['a1'], mode: 'sequential', avatarCalculatedColor: null },
  ];

  const sessionStore = useChatSessionStore();
  sessionStore.currentSelectedItem = item;

  await nextTick();
  return wrapper;
};

describe('GroupInviteBar', () => {
  it('saveGroup projects the saved chatMode into the invite bar snapshot', async () => {
    // 列表 mode 呈现本机 chatMode；完整配置中的共享 mode 不控制邀请入口。
    mockInvoke('save_group_config', (args) => args?.group);
    const wrapper = await mountBar({ id: 'g2', type: 'group', name: '顺序群' });
    expect(wrapper.findAll('button')).toHaveLength(0);

    const assistantStore = useAssistantStore();
    const group: GroupConfigDto = {
      id: 'g2',
      name: '顺序群',
      members: ['a1'],
      mode: 'sequential',
      chatMode: 'invite_only',
      avatarCalculatedColor: null,
      memberTags: {},
      groupPrompt: null,
      invitePrompt: null,
      useUnifiedModel: false,
      unifiedModel: null,
      topics: [],
      tagMatchMode: 'strict',
      createdAt: 1,
      enableContextMessageWindow: false,
      contextMessageWindowSize: 100,
      modeSettings: {
        sequential: { speakerOrder: ['a1'] },
        jev: {
          navigatorPrompt: '', speakerThreshold: 0.16, continueThreshold: 0.45, stopThreshold: 0.55,
          maxSpeakersPerRound: 3, maxAutonomousRounds: 12, historyWindow: 12, continueDebounceMs: 800,
          fallbackPolicy: 'stop', memberStyles: {},
        },
      },
    };
    await assistantStore.saveGroup(group);
    await nextTick();

    expect(assistantStore.groups.find((g) => g.id === 'g2')?.mode).toBe('invite_only');
    expect(wrapper.findAll('button')).toHaveLength(1);
  });
});
