import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { defineComponent } from 'vue';
import { DOMWrapper, flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import AgentSettingsView from '../../../features/agent/AgentSettingsView.vue';
import type { AgentConfigDto } from '../../../core/types/assistant';
import { SETTINGS_DEFAULTS } from '../../../core/config/generated/defaults';
import { useModalHistory } from '../../../core/composables/useModalHistory';
import { mockInvoke } from '../../mocks/tauri';

const context = vi.hoisted(() => ({
  saveAgent: vi.fn(), deleteAgent: vi.fn(), saveAvatar: vi.fn(), notify: vi.fn(),
  showConfirm: vi.fn(), clearConversation: vi.fn(), stopVoice: vi.fn(),
  allowMutation: vi.fn(), blockReason: null as string | null,
  currentSelectedItem: null as { type: string; id: string } | null,
}));
vi.mock('../../../core/stores/assistant', () => ({ useAssistantStore: () => context }));
vi.mock('../../../core/stores/chatSessionStore', () => ({ useChatSessionStore: () => context }));
vi.mock('../../../core/stores/notification', () => ({ useNotificationStore: () => ({ addNotification: context.notify }) }));
vi.mock('../../../core/stores/overlay', () => ({ useOverlayStore: () => context }));
vi.mock('../../../features/chat/chatMutationPolicy', () => ({
  allowChatMutation: context.allowMutation,
  chatMutationBlockReason: () => context.blockReason,
}));
vi.mock('../../../features/agent/AgentVoiceSettings.vue', () => ({
  default: defineComponent({
    name: 'AgentVoiceSettings', props: ['modelValue', 'active', 'disabled'], emits: ['update:modelValue'],
    setup: (_, { expose }) => { expose({ stop: context.stopVoice }); }, template: '<div />',
  }),
}));

function agent(id: string): AgentConfigDto {
  return {
    id, name: `Agent ${id}`, systemPrompt: 'shared prompt', mobileSystemPrompt: 'local prompt',
    model: 'configured-model', temperature: 0.7, contextTokenLimit: 32000, maxOutputTokens: 4000,
    streamOutput: true, useTemperature: false, avatarCalculatedColor: null, topics: [],
    voice: { enabled: false, ...SETTINGS_DEFAULTS.voice.tts },
  };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(accept => { resolve = accept; });
  return { promise, resolve };
}
const copy = <T>(value: T): T => JSON.parse(JSON.stringify(value));
const wrappers: VueWrapper[] = [];
const modals = useModalHistory();

beforeEach(() => {
  context.saveAgent.mockReset().mockImplementation(async (draft: AgentConfigDto) => copy(draft));
  context.deleteAgent.mockReset().mockResolvedValue(true);
  context.showConfirm.mockReset().mockResolvedValue(true);
  context.stopVoice.mockReset().mockResolvedValue(undefined);
  context.allowMutation.mockReset().mockReturnValue(true);
  context.blockReason = null;
  context.currentSelectedItem = null;
  mockInvoke('read_agent_config', args => agent(String(args?.agentId)));
  window.history.replaceState({ vcpRoot: true, vcpMain: true }, '', '#/chat');
  vi.spyOn(window.history, 'back').mockImplementation(() => {
    window.history.replaceState({ vcpRoot: true, vcpMain: true }, '');
    window.dispatchEvent(new PopStateEvent('popstate', { state: window.history.state }));
  });
});
afterEach(async () => {
  wrappers.splice(0).forEach(wrapper => wrapper.unmount());
  for (const id of ['a', 'b']) modals.unregisterModal(`Page:agentSettings:${id}`);
  await flushPromises();
});

async function render(id = 'a') {
  modals.registerModal(`Page:agentSettings:${id}`, vi.fn());
  const wrapper = mount(AgentSettingsView, {
    props: { id, isOpen: true, active: true },
    global: { stubs: {
      SlidePage: { template: '<div><slot /></div>' }, AvatarCropper: true, VcpAvatar: true,
      ModelSelector: { name: 'ModelSelector', props: ['modelValue', 'currentModel'], emits: ['select', 'update:modelValue'], template: '<div />' },
    } },
  });
  wrappers.push(wrapper);
  await flushPromises();
  return wrapper;
}
function button(wrapper: VueWrapper, text: string) {
  const result = wrapper.findAll('button').find(candidate => candidate.text().includes(text));
  if (!result) throw new Error(`Missing button: ${text}`);
  return result;
}
function inputFor(wrapper: VueWrapper, label: string) {
  const node = wrapper.findAll('label').find(candidate => candidate.text() === label)?.element.parentElement?.querySelector('input');
  if (!node) throw new Error(`Missing input: ${label}`);
  return new DOMWrapper(node);
}
async function close(wrapper: VueWrapper) {
  await wrapper.get('header button').trigger('click');
  await flushPromises();
}

describe('Agent settings authoritative voice draft', () => {
  it('submits voice with the original full baseline and adopts the canonical result', async () => {
    const wrapper = await render();
    const initial = agent('a');
    const voice = { ...initial.voice, enabled: true, mode: 'builtin' as const, voice: '冰糖', stream: false };
    wrapper.findComponent({ name: 'AgentVoiceSettings' }).vm.$emit('update:modelValue', voice);
    const canonical = { ...initial, name: 'Remote renamed agent', voice };
    context.saveAgent.mockResolvedValueOnce(canonical);
    await close(wrapper);
    expect(context.saveAgent).toHaveBeenCalledExactlyOnceWith({ ...initial, voice }, initial);
    expect((wrapper.get('input[placeholder="为你的助手起个名字..."]').element as HTMLInputElement).value).toBe(canonical.name);
    expect(wrapper.emitted('close')).toHaveLength(1);
  });

  it('retains a failed draft on the page and retries with the unchanged baseline', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    const wrapper = await render();
    await wrapper.get('input[placeholder="为你的助手起个名字..."]').setValue('Unsaved name');
    context.saveAgent.mockRejectedValueOnce(new Error('名称已被其他操作修改，请重新载入。'));
    await close(wrapper);
    expect(wrapper.emitted('close')).toBeUndefined();
    expect(wrapper.get('[role="alert"]').text()).toContain('名称已被其他操作修改');
    expect((wrapper.get('input[placeholder="为你的助手起个名字..."]').element as HTMLInputElement).value).toBe('Unsaved name');
    await close(wrapper);
    expect(context.saveAgent.mock.calls[1]?.[1]).toEqual(agent('a'));
    expect(wrapper.emitted('close')).toHaveLength(1);
  });

  it('does not let a late save for A replace the newly opened B draft', async () => {
    const pending = deferred<AgentConfigDto>();
    const wrapper = await render('a');
    await wrapper.get('input[placeholder="为你的助手起个名字..."]').setValue('A edited');
    context.saveAgent.mockImplementationOnce(() => pending.promise);
    await wrapper.setProps({ id: 'b' });
    await flushPromises();
    expect(context.saveAgent).toHaveBeenCalledOnce();
    expect(context.saveAgent.mock.calls[0]?.[0].id).toBe('a');
    await wrapper.get('input[placeholder="为你的助手起个名字..."]').setValue('B edited');
    pending.resolve({ ...agent('a'), name: 'A canonical' });
    await flushPromises();
    expect((wrapper.get('input[placeholder="为你的助手起个名字..."]').element as HTMLInputElement).value).toBe('B edited');
    await close(wrapper);
    expect(context.saveAgent.mock.calls[1]?.[0]).toMatchObject({ id: 'b', name: 'B edited' });
    expect(context.saveAgent.mock.calls[1]?.[1]).toEqual(agent('b'));
  });

  it('covering the page stops preview but keeps the draft without saving', async () => {
    const wrapper = await render();
    await wrapper.get('input[placeholder="为你的助手起个名字..."]').setValue('Covered draft');
    context.stopVoice.mockClear();
    await wrapper.setProps({ active: false });
    await flushPromises();
    expect(context.stopVoice).toHaveBeenCalled();
    expect(context.saveAgent).not.toHaveBeenCalled();
    await wrapper.setProps({ active: true });
    expect((wrapper.get('input[placeholder="为你的助手起个名字..."]').element as HTMLInputElement).value).toBe('Covered draft');
  });

  it('keeps all six model controls and the temperature enable relationship', async () => {
    const wrapper = await render();
    const temperature = inputFor(wrapper, 'Temperature (0-2):');
    expect(temperature.attributes('disabled')).toBeDefined();
    const switches = wrapper.findAll('input[type="checkbox"]');
    expect(switches).toHaveLength(2);
    await switches[1].setValue(true);
    expect(temperature.attributes('disabled')).toBeUndefined();
    await temperature.setValue('1.2');
    await inputFor(wrapper, '上下文 Token 上限').setValue('64000');
    await inputFor(wrapper, '最大输出 Token').setValue('8000');
    await switches[0].setValue(false);
    await inputFor(wrapper, '模型名称').setValue('manual-model');
    wrapper.findComponent({ name: 'ModelSelector' }).vm.$emit('select', 'selected-model');
    await close(wrapper);
    expect(context.saveAgent.mock.calls[0]?.[0]).toMatchObject({
      model: 'selected-model', temperature: 1.2, contextTokenLimit: 64000,
      maxOutputTokens: 8000, streamOutput: false, useTemperature: true,
    });
  });

  it('keeps deletion confirmation and clears the selected conversation only on success', async () => {
    const wrapper = await render();
    context.currentSelectedItem = { type: 'agent', id: 'a' };
    context.showConfirm.mockResolvedValueOnce(false);
    await button(wrapper, '删除此 Agent').trigger('click');
    await flushPromises();
    expect(context.deleteAgent).not.toHaveBeenCalled();
    context.deleteAgent.mockResolvedValueOnce(false);
    await button(wrapper, '删除此 Agent').trigger('click');
    await flushPromises();
    expect(context.clearConversation).not.toHaveBeenCalled();
    expect(wrapper.emitted('close')).toBeUndefined();
    await button(wrapper, '删除此 Agent').trigger('click');
    await flushPromises();
    expect(context.showConfirm).toHaveBeenLastCalledWith({ title: '删除 Agent', message: '确定要删除这个 Agent 吗？此操作不可撤销。', isDanger: true });
    expect(context.clearConversation).toHaveBeenCalledOnce();
    expect(wrapper.emitted('close')).toHaveLength(1);
    expect(context.saveAgent).not.toHaveBeenCalled();
  });

  it('retains the existing deletion block reason and does not open confirmation', async () => {
    context.blockReason = '请先结束当前通话';
    const wrapper = await render();
    expect(button(wrapper, '删除此 Agent').attributes('disabled')).toBeDefined();
    expect(wrapper.text()).toContain(context.blockReason);
    await button(wrapper, '删除此 Agent').trigger('click');
    expect(context.showConfirm).not.toHaveBeenCalled();
  });
});
