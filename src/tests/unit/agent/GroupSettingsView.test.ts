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
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const makeGroup = (): GroupConfigDto => ({
  id: 'g1', name: '研究讨论', avatarCalculatedColor: null,
  members: ['a1', 'a2', 'a3'], mode: 'sequential', chatMode: 'sequential',
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
    await wrapper!.get(`button[role="radio"][aria-label="${label}"]`).trigger('click');
    await nextTick();
  }

  async function openMemberConfig(name: string) {
    await wrapper!.get(`button[aria-label="配置 ${name}"]`).trigger('click');
    await nextTick();
  }

  async function togglePrompt(title: string) {
    const trigger = wrapper!.findAll('button.group-prompt-trigger').find(btn => btn.text().includes(title));
    if (!trigger) throw new Error(`Missing prompt trigger: ${title}`);
    await trigger.trigger('click');
    await nextTick();
  }

  const changes = () => invokeMock.mock.calls.filter(([command]) => command === 'update_group_config');

  it('uses the independent chat mode without saving an untouched draft', async () => {
    persisted.mode = 'jev';
    const view = await open();
    expect(view.get('button[role="radio"][aria-label="顺序发言"]').attributes('aria-checked')).toBe('true');
    expect(view.find('button[aria-label="下移 Nova"]').exists()).toBe(true);
    await view.get('button[aria-label="返回并保存群组设置"]').trigger('click');
    await flushPromises();
    expect(changes()).toHaveLength(0);
  });

  it.each([
    { fieldTop: 380, fieldBottom: 480, expectedScroll: 80 },
    { fieldTop: 260, fieldBottom: 360, expectedScroll: 0 },
  ])('keeps keyboard reveal inside the member sheet (field bottom: $fieldBottom)', async ({ fieldTop, fieldBottom, expectedScroll }) => {
    const view = await open();
    await openMemberConfig('Luna');
    const field = control('Luna 的发言风格').element as HTMLTextAreaElement;
    const sheet = view.get('[aria-label="Luna 群内配置"]').element as HTMLElement;
    const scroller = sheet.querySelector<HTMLElement>('.group-sheet-body')!;
    const scrollAncestors = vi.spyOn(field, 'scrollIntoView');
    vi.spyOn(field, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, fieldTop, 320, fieldBottom - fieldTop));
    vi.spyOn(scroller, 'getBoundingClientRect').mockImplementation(() => {
      // The keyboard event also patches the sheet padding: reveal must use the updated layout.
      expect(sheet.style.paddingBottom).toBe('8px');
      return new DOMRect(0, 200, 360, 200);
    });
    field.focus();
    const focus = vi.spyOn(field, 'focus');
    const blur = vi.spyOn(field, 'blur');
    try {
      window.dispatchEvent(new CustomEvent('vcp-keyboard-inset', {
        detail: { imeBottomPx: 300, imeVisible: true, safeBottomPx: 0 },
      }));
      await nextTick();

      expect(document.activeElement).toBe(field);
      expect(focus).not.toHaveBeenCalled();
      expect(blur).not.toHaveBeenCalled();
      expect(scrollAncestors).not.toHaveBeenCalled();
      expect(scroller.scrollTop).toBe(expectedScroll);
    } finally {
      window.dispatchEvent(new CustomEvent('vcp-keyboard-inset', {
        detail: { imeBottomPx: 0, imeVisible: false, safeBottomPx: 0 },
      }));
      await nextTick();
    }
  });

  it('retains each mode draft and explicit speaker order across repeated mode changes', async () => {
    const view = await open();
    await view.get('button[aria-label="下移 Nova"]').trigger('click');
    await chooseMode('JEV 智能群聊');
    await togglePrompt('讨论规则');
    await control('讨论规则').setValue('有新证据时再发言');
    await openMemberConfig('Luna');
    await control('Luna 的发言风格').setValue('先辨认隐含前提');
    await view.get('[aria-label="Luna 群内配置"] .group-sheet-done-btn').trigger('click');
    await chooseMode('自然随机');
    await openMemberConfig('Nova');
    await control('Nova 的触发标签').setValue('量子,数学');
    await view.get('[aria-label="Nova 群内配置"] .group-sheet-done-btn').trigger('click');
    await chooseMode('顺序发言');
    expect(view.findAll('button[aria-label^="上移 "]').map(button => button.attributes('aria-label')))
      .toEqual(['上移 Luna', '上移 Nova', '上移 Mira']);
    await chooseMode('JEV 智能群聊');
    expect((control('讨论规则').element as HTMLTextAreaElement).value).toBe('有新证据时再发言');
    await openMemberConfig('Luna');
    expect((control('Luna 的发言风格').element as HTMLTextAreaElement).value).toBe('先辨认隐含前提');
    await view.get('[aria-label="Luna 群内配置"] .group-sheet-done-btn').trigger('click');
    expect(changes()).toHaveLength(0);

    await view.get('button[aria-label="返回并保存群组设置"]').trigger('click');
    await flushPromises();
    expect(persisted.modeSettings.sequential.speakerOrder).toEqual(['a2', 'a1', 'a3']);
    expect(persisted.modeSettings.jev.memberStyles.a2).toBe('先辨认隐含前提');
    expect(persisted.memberTags?.a1).toBe('量子,数学');
    expect(persisted.mode).toBe('sequential');
    expect(persisted.chatMode).toBe('jev');
  });

  it('pushes checked members to the end of the stack and extracts unchecked members directly', async () => {
    const view = await open();
    await view.get('button[aria-label="增减成员"]').trigger('click');
    const dialog = view.get('[aria-label="增减群组成员"]');
    const rowFor = (name: string) =>
      dialog.findAll('label.group-sheet-member-row').find(item => item.text().includes(name))!;

    // 直接抽离中间成员 Luna (a2)，其余成员保持 a1, a3 次序
    await rowFor('Luna').get('input[type="checkbox"]').trigger('change');
    expect(view.findAll('button[aria-label^="配置 "]').map(btn => btn.attributes('aria-label')))
      .toEqual(['配置 Nova', '配置 Mira']);

    // 重新勾选 Luna (a2)：按勾选顺序入栈到队尾 -> a1, a3, a2
    await rowFor('Luna').get('input[type="checkbox"]').trigger('change');
    expect(view.findAll('button[aria-label^="配置 "]').map(btn => btn.attributes('aria-label')))
      .toEqual(['配置 Nova', '配置 Mira', '配置 Luna']);

    await dialog.get('.group-sheet-done-btn').trigger('click');
    await view.get('button[aria-label="返回并保存群组设置"]').trigger('click');
    await flushPromises();
    expect(persisted.members).toEqual(['a1', 'a3', 'a2']);
    expect(persisted.modeSettings.sequential.speakerOrder).toEqual(['a1', 'a3', 'a2']);
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
    expect(Object.keys(payload.updates).sort()).toEqual(['chatMode', 'modeSettings']);
    expect(payload.updates.chatMode).toBe('jev');
    expect(persisted.name).toBe('桌面刚同步的名字');
    expect(persisted.groupPrompt).toBe('更新后的群背景');
    expect(persisted.mode).toBe('naturerandom');
    expect(useAssistantStore().groups[0].name).toBe('桌面刚同步的名字');
  });

  it('auto-saves inline group name on blur and updates JEV numeric parameters via direct prompt dialog', async () => {
    const view = await open();
    await view.get('button[aria-label="编辑群组名称"]').trigger('click');
    const nameInput = control('群组名称');
    await nameInput.setValue('量子前沿讨论组');
    await nameInput.trigger('blur');
    await flushPromises();

    expect(changes()).toHaveLength(1);
    expect(persisted.name).toBe('量子前沿讨论组');

    await chooseMode('JEV 智能群聊');
    const maxSpeakersRow = view.findAll('button.settings-row').find(btn => btn.text().includes('每轮发言人数上限'))!;
    await maxSpeakersRow.trigger('click');
    const overlay = useOverlayStore();
    expect(overlay.contextMenuConfig).toBeNull();
    expect(overlay.promptConfig?.title).toBe('每轮发言人数上限（1–32）');
    expect(overlay.promptConfig?.initialValue).toBe('3');
    overlay.promptConfig?.onConfirm('4');
    await nextTick();
    expect(maxSpeakersRow.text()).toContain('4位');

    await view.get('button[role="switch"][aria-label="限制参考消息"]').trigger('click');
    const windowChip = view.get('button[aria-label="回复参考消息数"]');
    expect(windowChip.text()).toContain('100条');
    await windowChip.trigger('click');
    expect(overlay.promptConfig?.title).toBe('回复参考消息数（1–10000）');
    overlay.promptConfig?.onConfirm('36');
    await nextTick();
    expect(windowChip.text()).toContain('36条');
  });

  it.each(['button', 'system-back'] as const)('joins the pending name save before closing via %s', async closeBy => {
    const save = deferred<GroupConfigDto>();
    mockInvoke('update_group_config', () => save.promise);
    const view = await open();
    await view.get('button[aria-label="编辑群组名称"]').trigger('click');
    await control('群组名称').setValue('等待保存后返回');
    await control('群组名称').trigger('blur');

    const back = view.get<HTMLButtonElement>('button[aria-label="返回并保存群组设置"]');
    expect(back.element.disabled).toBe(false);
    if (closeBy === 'button') back.element.click();
    else useModalHistory().closeTopModal();
    await nextTick();
    expect(view.emitted('close')).toBeUndefined();
    expect(changes()).toHaveLength(1);

    save.resolve({ ...persisted, name: '等待保存后返回' });
    await flushPromises();
    expect(changes()).toHaveLength(1);
    expect(view.emitted('close')).toHaveLength(1);
    expect(useOverlayStore().isGroupSettingsOpen).toBe(false);
  });

  it('keeps the draft and allows retry when the joined blur save fails', async () => {
    const save = deferred<GroupConfigDto>();
    mockInvoke('update_group_config', () => save.promise);
    const view = await open();
    await view.get('button[aria-label="编辑群组名称"]').trigger('click');
    await control('群组名称').setValue('失败后仍保留');
    await control('群组名称').trigger('blur');
    await view.get('button[aria-label="返回并保存群组设置"]').trigger('click');
    save.reject(new Error('storage busy'));
    await flushPromises();

    expect(view.emitted('close')).toBeUndefined();
    expect(changes()).toHaveLength(1);
    expect(view.get('[role="alert"]').text()).toContain('storage busy');
    expect((control('群组名称').element as HTMLInputElement).value).toBe('失败后仍保留');
    mockInvoke('update_group_config', args => ({ ...persisted, ...copy(args?.updates as Partial<GroupConfigDto>) }));
    await view.get('button[aria-label="返回并保存群组设置"]').trigger('click');
    await flushPromises();
    expect(changes()).toHaveLength(2);
    expect(view.emitted('close')).toHaveLength(1);
  });

  it.each(['other-group', 'same-group-reopened'] as const)('does not close a newer editor when the joined save finishes (%s)', async context => {
    const save = deferred<GroupConfigDto>();
    mockInvoke('update_group_config', () => save.promise);
    const view = await open();
    await view.get('button[aria-label="编辑群组名称"]').trigger('click');
    await control('群组名称').setValue('旧页面提交');
    await control('群组名称').trigger('blur');
    await view.get('button[aria-label="返回并保存群组设置"]').trigger('click');

    if (context === 'other-group') {
      mockInvoke('read_group_config', () => ({ ...copy(persisted), id: 'g2', name: '另一个群' }));
      useOverlayStore().closeGroupSettings();
      useOverlayStore().openGroupSettings('g2');
      await view.setProps({ id: 'g2' });
      await flushPromises();
    } else {
      await view.setProps({ isOpen: false });
      await view.setProps({ isOpen: true });
    }
    persisted.name = '旧页面提交';
    save.resolve(copy(persisted));
    await flushPromises();

    expect(view.emitted('close')).toBeUndefined();
    expect(changes()).toHaveLength(1);
    expect(view.props()).toMatchObject({ isOpen: true });
    expect((control('群组名称').element as HTMLInputElement).value)
      .toBe(context === 'other-group' ? '另一个群' : '旧页面提交');
  });

  it('honors a new close intent while the reopened group still awaits its earlier save', async () => {
    const save = deferred<GroupConfigDto>();
    mockInvoke('update_group_config', () => save.promise);
    const view = await open();
    await view.get('button[aria-label="编辑群组名称"]').trigger('click');
    await control('群组名称').setValue('正在提交的名称');
    await control('群组名称').trigger('blur');
    await view.setProps({ isOpen: false });
    await view.setProps({ isOpen: true });
    await view.get('button[aria-label="返回并保存群组设置"]').trigger('click');
    expect(view.emitted('close')).toBeUndefined();

    persisted.name = '正在提交的名称';
    save.resolve(copy(persisted));
    await flushPromises();
    expect(changes()).toHaveLength(1);
    expect(view.emitted('close')).toHaveLength(1);
  });

  it('saves defaults silently without alert popups when invalid values are entered via prompt dialogs', async () => {
    const view = await open();
    await chooseMode('JEV 智能群聊');
    const maxSpeakersRow = view.findAll('button.settings-row').find(btn => btn.text().includes('每轮发言人数上限'))!;
    await maxSpeakersRow.trigger('click');
    const overlay = useOverlayStore();
    overlay.promptConfig?.onConfirm('abc'); // 非法字符
    await nextTick();

    await view.get('button[role="switch"][aria-label="限制参考消息"]').trigger('click');
    const windowChip = view.get('button[aria-label="回复参考消息数"]');
    await windowChip.trigger('click');
    overlay.promptConfig?.onConfirm('999999'); // 严重超界
    await nextTick();

    await view.get('button[aria-label="编辑群组名称"]').trigger('click');
    const nameInput = control('群组名称');
    await nameInput.setValue('静默默认测试');
    await view.get('button[aria-label="返回并保存群组设置"]').trigger('click');
    await flushPromises();

    expect(view.emitted('close')).toHaveLength(1);
    expect(view.find('[role="alert"]').exists()).toBe(false);
    expect(persisted.name).toBe('静默默认测试');
    expect(persisted.modeSettings.jev.maxSpeakersPerRound).toBe(3);
    expect(persisted.contextMessageWindowSize).toBe(100);
  });

  it('normalizes prompt input to defaults when confirming invalid values directly', async () => {
    const view = await open();
    await chooseMode('JEV 智能群聊');
    const maxSpeakersRow = view.findAll('button.settings-row').find(btn => btn.text().includes('每轮发言人数上限'))!;
    await maxSpeakersRow.trigger('click');
    const overlay = useOverlayStore();
    overlay.promptConfig?.onConfirm('999'); // out of range 1-32
    await nextTick();
    expect(maxSpeakersRow.text()).toContain('3位'); // fallback to default 3

    await view.get('button[role="switch"][aria-label="限制参考消息"]').trigger('click');
    const windowChip = view.get('button[aria-label="回复参考消息数"]');
    await windowChip.trigger('click');
    overlay.promptConfig?.onConfirm('-50'); // out of range 1-10000
    await nextTick();
    expect(windowChip.text()).toContain('100条'); // fallback to default 100
  });

  it('still enforces mandatory UI guard when group name is empty', async () => {
    const view = await open();
    await view.get('button[aria-label="编辑群组名称"]').trigger('click');
    const nameInput = control('群组名称');
    await nameInput.setValue('   ');
    // Call requestClose directly
    await view.get('button[aria-label="返回并保存群组设置"]').trigger('click');
    await flushPromises();

    expect(view.emitted('close')).toBeUndefined();
    expect(view.get('[role="alert"]').text()).toContain('请填写群组名称。');
  });

  it('still enforces mandatory UI guard when unified model is enabled without a selected model', async () => {
    const view = await open();
    await view.get('button[role="switch"][aria-label="启用群组统一模型"]').trigger('click');
    const vm = view.vm as any;
    vm.groupConfig.unifiedModel = '';
    await view.get('button[aria-label="返回并保存群组设置"]').trigger('click');
    await flushPromises();

    expect(view.emitted('close')).toBeUndefined();
    expect(view.get('[role="alert"]').text()).toContain('启用群组统一模型后，请选择一个模型。');
  });

  it('provides safe initialValue in prompt even if underlying field was abnormal', async () => {
    const view = await open();
    await chooseMode('JEV 智能群聊');
    const vm = view.vm as any;
    vm.groupConfig.modeSettings.jev.maxSpeakersPerRound = NaN;
    const maxSpeakersRow = view.findAll('button.settings-row').find(btn => btn.text().includes('每轮发言人数上限'))!;
    await maxSpeakersRow.trigger('click');
    const overlay = useOverlayStore();
    // initialValue must be normalized '3', not 'NaN' or 'undefined'
    expect(overlay.promptConfig?.initialValue).toBe('3');
  });
});
