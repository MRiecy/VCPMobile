import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, setActivePinia, type Pinia } from 'pinia';
import { reactive } from 'vue';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import AgentVoiceSettings from '../../../features/agent/AgentVoiceSettings.vue';
import { useSettingsStore } from '../../../core/stores/settings';
import { SETTINGS_DEFAULTS } from '../../../core/config/generated/defaults';
import { makeConfigSnapshot, makeSettings } from '../../helpers/config';
import { invokeMock, mockInvoke } from '../../mocks/tauri';

const context = vi.hoisted(() => ({ voice: {} as { active: boolean; playbackActive: boolean } }));
vi.mock('../../../features/chat/voice/voiceCallStore', () => ({ useVoiceCallStore: () => context.voice }));

let pinia: Pinia;
const wrappers: VueWrapper[] = [];
beforeEach(() => {
  pinia = createPinia();
  setActivePinia(pinia);
  context.voice = reactive({ active: false, playbackActive: false });
  mockInvoke('read_settings', () => makeConfigSnapshot(makeSettings({
    'voice.connection.apiBaseUrl': 'https://voice.example/v1', 'voice.connection.apiKey': 'voice-key',
    'voice.tts.mode': 'builtin', 'voice.tts.voice': '苏打',
  })));
});
afterEach(async () => {
  wrappers.splice(0).forEach(wrapper => wrapper.unmount());
  await flushPromises();
  disposePinia(pinia);
});

describe('Agent voice selection ownership', () => {
  it('explicit none does not preview the global default or display a model', async () => {
    const wrapper = mount(AgentVoiceSettings, {
      props: { active: true, modelValue: { enabled: true, ...SETTINGS_DEFAULTS.voice.tts } },
    });
    wrappers.push(wrapper);
    await flushPromises();
    expect(wrapper.text()).toContain('当前助手不使用语音朗读');
    expect(wrapper.find('.voice-preview-button').exists()).toBe(false);
    expect(wrapper.find('code').exists()).toBe(false);
    expect(wrapper.find('[aria-label="流式朗读"]').exists()).toBe(false);
    expect(invokeMock.mock.calls.filter(([command]) => command === 'preview_tts')).toHaveLength(0);
    expect(wrapper.emitted('update:modelValue')).toBeUndefined();
  });

  it('previews the Agent draft using global credentials without changing global TTS settings', async () => {
    const voice = { enabled: true, mode: 'builtin' as const, voice: '冰糖', cloneSampleId: '', stream: false };
    const wrapper = mount(AgentVoiceSettings, { props: { active: true, modelValue: voice } });
    wrappers.push(wrapper);
    await flushPromises();
    await wrapper.get('.settings-disclosure-toggle').trigger('click');
    await wrapper.get('.voice-preview-button').trigger('click');
    await flushPromises();
    const submitted = invokeMock.mock.calls.find(([command]) => command === 'preview_tts')?.[1];
    expect(submitted?.settings).toEqual({
      connection: { apiBaseUrl: 'https://voice.example/v1', apiKey: 'voice-key' },
      tts: { mode: 'builtin', voice: '冰糖', cloneSampleId: '', stream: false },
    });
    expect(useSettingsStore().settings?.voice.tts.voice).toBe('苏打');
    expect(invokeMock.mock.calls.filter(([command]) => command === 'update_settings')).toHaveLength(0);
    expect(wrapper.emitted('update:modelValue')).toBeUndefined();
    await wrapper.setProps({ active: false });
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith('stop_tts', { ttsId: submitted?.ttsId });
  });
});
