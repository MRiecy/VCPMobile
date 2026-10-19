import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { nextTick } from 'vue';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import GroupSettingsView from '@/features/agent/GroupSettingsView.vue';
import { useAssistantStore } from '@/core/stores/assistant';
import { useOverlayStore } from '@/core/stores/overlay';
import { useModalHistory } from '@/core/composables/useModalHistory';
import type { GroupConfigDto } from '@/core/types/assistant';
import { invokeMock, mockInvoke } from '../../mocks/tauri';

vi.mock('@/features/chat/chatMutationPolicy', () => ({
  allowChatMutation: () => true,
  chatMutationBlockReason: () => null,
}));

const copy = <T,>(value: T): T => JSON.parse(JSON.stringify(value));
const makeGroup = (): GroupConfigDto => ({
  id: 'g1', name: '研究讨论', avatarCalculatedColor: null,
  members: ['a1', 'a2', 'a3'], mode: 'sequential', modeOverride: null,
  memberTags: { a1: '物理', a2: '文学' }, tagMatchMode: 'strict',
  groupPrompt: '原有群背景', invitePrompt: '', useUnifiedModel: false, unifiedModel: null,
  topics: [], createdAt: 1,
  modeSettings: {
    sequential: { speakerOrder: ['a1', 'a2', 'a3'] },
    jev: {
      navigatorPrompt: '围绕当前主题发言', speakerThreshold: 0.16, continueThreshold: 0.45,
      stopThreshold: 0.55, maxSpeakersPerRound: 3, maxAutonomousRounds: 12,
      historyWindow: 12, continueDebounceMs: 800, fallbackPolicy: 'stop', memberStyles: { a2: '善于文学分析' },
    },
  },
  enableContextMessageWindow: false, contextMessageWindowSize: 100,
});

describe('GroupSettingsView editing and return saving', () => {
  let wrapper: VueWrapper | null;
  let persisted: GroupConfigDto;

  beforeEach(() => {
    setActivePinia(createPinia());
    wrapper = null;
    persisted = makeGroup();
    window.history.replaceState({ vcpRoot: true, vcpMain: true }, '', '#/chat');
    vi.spyOn(window.history, 'back').mockImplementation(() => {
      window.history.replaceState({ vcpRoot: true, vcpMain: true }, '');
    });
    mockInvoke('read_group_config', () => copy(persisted));
    mockInvoke('update_group_config', args => {
      persisted = { ...persisted, ...copy(args?.updates as Partial<GroupConfigDto>) };
      return copy(persisted);
    });
    const assistants = useAssistantStore();
    assistants.agents = ['Nova', 'Luna', 'Mira'].map((name, index) => ({
      id: `a${index + 1}`, name, model: 'chat-model', avatarCalculatedColor: null,
    }));
    assistants.groups = [{ id: 'g1', name: persisted.name, members: [...persisted.members], mode: 'sequential', avatarCalculatedColor: null }];
  });

  afterEach(() => {
    wrapper?.unmount();
    useOverlayStore().closeContextMenu();
    useOverlayStore().popToRoot();
  });

  async function open() {
    const overlay = useOverlayStore();
    overlay.openGroupSettings('g1');
    wrapper = mount(GroupSettingsView, {
      props: {
        id: 'g1', isOpen: true,
        onClose: () => {
          overlay.closeGroupSettings();
          void wrapper?.setProps({ isOpen: false });
        },
      },
      attachTo: document.body,
      global: { stubs: { VcpAvatar: true, ModelSelector: true, AvatarCropper: true, GroupModeHelpDialog: true } },
    });
    await flushPromises();
    return wrapper;
  }

  function control(label: string) {
    const forId = wrapper!.findAll('label').find(item => item.text() === label && item.attributes('for'))?.attributes('for');
    if (!forId) throw new Error(`Missing field: ${label}`);
    return wrapper!.get(`[id="${forId}"]`);
  }

  async function chooseMode(label: string) {
    await wrapper!.findAll('button').find(button => button.text().includes('本机发言模式'))!.trigger('click');
    const overlay = useOverlayStore();
    const action = overlay.contextMenuConfig?.actions.find(item => item.label === label);
    if (!action) throw new Error(`Missing mode: ${label}`);
    action.handler();
    overlay.closeContextMenu();
    await nextTick();
  }

  const changes = () => invokeMock.mock.calls.filter(([command]) => command === 'update_group_config');

  it('retains each mode draft and explicit speaker order across repeated mode changes', async () => {
    const view = await open();
    await view.get('button[aria-label="下移 Nova"]').trigger('click');
    await chooseMode('JEV 智能群聊');
    await control('JEV 导航员提示词').setValue('有新证据时再发言');
    await view.findAll('button').find(button => button.text().includes('成员发言风格'))!.trigger('click');
    await control('Luna').setValue('先辨认隐含前提');
    await chooseMode('自然随机');
    await control('Nova 的触发标签').setValue('量子,数学');
    await chooseMode('顺序发言');
    expect(view.findAll('button[aria-label^="上移 "]').map(button => button.attributes('aria-label')))
      .toEqual(['上移 Luna', '上移 Nova', '上移 Mira']);
    await chooseMode('JEV 智能群聊');
    expect((control('JEV 导航员提示词').element as HTMLTextAreaElement).value).toBe('有新证据时再发言');
    await view.findAll('button').find(button => button.text().includes('成员发言风格'))!.trigger('click');
    expect((control('Luna').element as HTMLTextAreaElement).value).toBe('先辨认隐含前提');
    expect(changes()).toHaveLength(0);

    await view.get('button[aria-label="返回并保存群组设置"]').trigger('click');
    await flushPromises();
    expect(persisted.modeSettings.sequential.speakerOrder).toEqual(['a2', 'a1', 'a3']);
    expect(persisted.modeSettings.jev.memberStyles.a2).toBe('先辨认隐含前提');
    expect(persisted.memberTags?.a1).toBe('量子,数学');
    expect(persisted.mode).toBe('sequential');
    expect(persisted.modeOverride).toBe('jev');
  });

  it('retains the page and draft after system-back save failure, then retries the same edit', async () => {
    const save = vi.fn()
      .mockRejectedValueOnce(new Error('database unavailable'))
      .mockImplementation(args => ({ ...persisted, ...copy(args.updates) }));
    mockInvoke('update_group_config', save);
    const view = await open();
    await control('群组名称').setValue('尚未保存的名字');
    useModalHistory().closeTopModal();
    await flushPromises();

    expect(view.emitted('close')).toBeUndefined();
    expect(useOverlayStore().isGroupSettingsOpen).toBe(true);
    expect((control('群组名称').element as HTMLInputElement).value).toBe('尚未保存的名字');
    expect(view.get('[role="alert"]').text()).toContain('database unavailable');
    await view.get('button[aria-label="返回并保存群组设置"]').trigger('click');
    await flushPromises();

    expect(save).toHaveBeenCalledTimes(2);
    expect(save.mock.calls[1][0]).toEqual(save.mock.calls[0][0]);
    expect(view.emitted('close')).toHaveLength(1);
    expect(useOverlayStore().isGroupSettingsOpen).toBe(false);
  });

  it('sends only local mode and ordering changes while accepting newer shared fields from save', async () => {
    const view = await open();
    await view.get('button[aria-label="下移 Nova"]').trigger('click');
    await chooseMode('JEV 智能群聊');
    // A sync updated the shared data after this editor took its baseline.
    persisted.name = '桌面刚同步的名字';
    persisted.groupPrompt = '更新后的群背景';
    persisted.mode = 'naturerandom';
    await view.get('button[aria-label="返回并保存群组设置"]').trigger('click');
    await flushPromises();

    expect(changes()).toHaveLength(1);
    const payload = changes()[0][1] as { groupId: string; updates: Record<string, unknown> };
    expect(payload.groupId).toBe('g1');
    expect(Object.keys(payload.updates).sort()).toEqual(['modeOverride', 'modeSettings']);
    expect(payload.updates.modeOverride).toBe('jev');
    expect(persisted.name).toBe('桌面刚同步的名字');
    expect(persisted.groupPrompt).toBe('更新后的群背景');
    expect(persisted.mode).toBe('naturerandom');
    expect(useAssistantStore().groups[0].name).toBe('桌面刚同步的名字');
  });
});
