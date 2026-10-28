import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { reactive } from 'vue';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import AgentSettingsView from '../../../features/agent/AgentSettingsView.vue';
import GlobalOverlayManager from '../../../components/GlobalOverlayManager.vue';
import type { AgentConfigDto } from '../../../core/types/assistant';
import { SETTINGS_DEFAULTS } from '../../../core/config/generated/defaults';
import { useModalHistory } from '../../../core/composables/useModalHistory';
import { makeConfigSnapshot, makeSettings } from '../../helpers/config';
import { invokeMock, mockInvoke } from '../../mocks/tauri';

const context = vi.hoisted(() => ({
  saveAgent: vi.fn(), deleteAgent: vi.fn(), saveAvatar: vi.fn(),
  voice: {} as { active: boolean; playbackActive: boolean },
}));
vi.mock('../../../core/stores/assistant', () => ({ useAssistantStore: () => context }));
vi.mock('../../../core/stores/chatSessionStore', () => ({ useChatSessionStore: () => ({ currentSelectedItem: null }) }));
vi.mock('../../../core/stores/notification', () => ({ useNotificationStore: () => ({ addNotification: vi.fn() }) }));
vi.mock('../../../features/chat/voice/voiceCallStore', () => ({ useVoiceCallStore: () => context.voice }));
vi.mock('../../../features/chat/chatMutationPolicy', () => ({
  allowChatMutation: () => true, chatMutationBlockReason: () => null,
}));

function agent(): AgentConfigDto {
  return {
    id: 'voice-integration', name: 'Independent assistant', systemPrompt: '', mobileSystemPrompt: '',
    model: 'configured-model', temperature: 1, contextTokenLimit: 32000, maxOutputTokens: 4000,
    streamOutput: true, useTemperature: false, avatarCalculatedColor: null, topics: [],
    voice: { enabled: false, ...SETTINGS_DEFAULTS.voice.tts },
  };
}
let pinia: Pinia;
const wrappers: VueWrapper[] = [];
beforeEach(() => {
  pinia = createPinia();
  setActivePinia(pinia);
  context.voice = reactive({ active: false, playbackActive: false });
  context.saveAgent.mockReset().mockImplementation(async (draft: AgentConfigDto) => structuredClone(draft));
  mockInvoke('read_agent_config', agent);
  mockInvoke('read_settings', () => makeConfigSnapshot(makeSettings({ 'voice.tts.mode': 'builtin', 'voice.tts.voice': '苏打' })));
  mockInvoke('list_voice_clone_samples', () => []);
  window.history.replaceState({ vcpRoot: true, vcpMain: true }, '', '#/chat');
  vi.spyOn(window.history, 'back').mockImplementation(() => {
    window.history.replaceState({ vcpRoot: true, vcpMain: true }, '');
    window.dispatchEvent(new PopStateEvent('popstate', { state: window.history.state }));
  });
  wrappers.push(mount(GlobalOverlayManager, {
    attachTo: document.body,
    global: { stubs: { ToastManager: true, RenderedImageViewer: true }, directives: { guide: {} } },
  }));
});
afterEach(async () => {
  wrappers.splice(0).reverse().forEach(wrapper => wrapper.unmount());
  useModalHistory().unregisterModal('Page:agentSettings:voice-integration');
  await flushPromises();
  disposePinia(pinia);
});

async function render() {
  const wrapper = mount(AgentSettingsView, {
    props: { id: 'voice-integration', isOpen: true, active: true }, attachTo: document.body,
    global: { stubs: {
      SlidePage: { template: '<div><slot /></div>' }, ModelSelector: true, AvatarCropper: true, VcpAvatar: true,
    } },
  });
  wrappers.push(wrapper);
  await flushPromises();
  return wrapper;
}

describe('Agent settings with the real voice section', () => {
  it('keeps the full voice form visible and enables editing with the independent switch', async () => {
    const wrapper = await render();
    const section = wrapper.get('[aria-labelledby="agent-voice-settings-title"]');
    const toggle = section.get('[role="switch"][aria-label="启用独立音色"]');
    expect(toggle.attributes('aria-checked')).toBe('false');
    expect(section.text()).toContain('关闭则使用全局默认音色');
    expect(section.get('code').text()).toBe('mimo-v2.5-tts');
    expect(section.get('[aria-label="当前音色"]').isVisible()).toBe(true);
    expect(section.get('[aria-label="当前音色"]').text()).toContain('冰糖');
    expect(section.get('[aria-label="当前音色"]').attributes('disabled')).toBeDefined();
    expect(section.get('[aria-label="试听文本"]').isVisible()).toBe(true);
    expect(section.get('[aria-label="试听文本"]').attributes('disabled')).toBeDefined();
    await toggle.trigger('click');
    await flushPromises();
    expect(toggle.attributes('aria-checked')).toBe('true');
    expect(section.get('[aria-label="试听文本"]').attributes('disabled')).toBeUndefined();
    const modes = section.get('[role="group"][aria-label="音色模式"]').findAll('button');
    expect(modes.map(mode => mode.text())).toEqual(['预制音色', '音色复刻']);
    expect(section.get('code').text()).toBe('mimo-v2.5-tts');
    expect(section.get('[aria-label="当前音色"]').text()).toContain('冰糖');
    expect(section.get('[role="switch"][aria-label="流式朗读"]').isVisible()).toBe(true);
    expect(section.get('[aria-label="试听文本"]').isVisible()).toBe(true);
    await modes[1].trigger('click');
    await flushPromises();
    expect(section.get('code').text()).toBe('mimo-v2.5-tts-voiceclone');
    expect(section.get('[aria-label="当前音色"]').text()).toContain('无');
    expect(section.get('[aria-label="音色管理"]').isVisible()).toBe(true);
    expect(section.get('[aria-label="试听文本"]').isVisible()).toBe(true);
    await modes[0].trigger('click');
    await wrapper.get('button[aria-label="返回，关闭助手设置"]').trigger('click');
    await flushPromises();
    expect(context.saveAgent.mock.calls[0]?.[0].voice).toEqual({ ...agent().voice, enabled: true });
    expect(invokeMock.mock.calls.filter(([command]) => command === 'update_settings')).toHaveLength(0);
  });

  it('turning independence off preserves the chosen voice draft and leaves global inheritance intact', async () => {
    const initial = agent();
    initial.voice = { ...initial.voice, enabled: true, voice: '冰糖' };
    mockInvoke('read_agent_config', () => initial);
    const wrapper = await render();
    await wrapper.get('[aria-label="试听文本"]').setValue('保留这段试听内容');
    await wrapper.get('[role="switch"][aria-label="启用独立音色"]').trigger('click');
    expect(wrapper.get('[aria-label="当前音色"]').text()).toContain('冰糖');
    expect(wrapper.get('[aria-label="当前音色"]').attributes('disabled')).toBeDefined();
    expect((wrapper.get('[aria-label="试听文本"]').element as HTMLInputElement).value).toBe('保留这段试听内容');
    expect(wrapper.get('.voice-preview-button').attributes('disabled')).toBeDefined();
    await wrapper.get('button[aria-label="返回，关闭助手设置"]').trigger('click');
    await flushPromises();
    expect(context.saveAgent.mock.calls[0]?.[0].voice).toEqual({ ...initial.voice, enabled: false });
    expect(invokeMock.mock.calls.filter(([command]) => command === 'update_settings' || command === 'preview_tts')).toHaveLength(0);
  });
});
