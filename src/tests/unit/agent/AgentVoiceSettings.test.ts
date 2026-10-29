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
  const overlays = document.createElement('div');
  overlays.id = 'vcp-feature-overlays';
  document.body.append(overlays);
  context.voice = reactive({ active: false, playbackActive: false });
  mockInvoke('read_settings', () => makeConfigSnapshot(makeSettings({
    'voice.connection.apiBaseUrl': 'https://voice.example/v1', 'voice.connection.apiKey': 'voice-key',
    'voice.tts.mode': 'builtin', 'voice.tts.voice': '苏打',
  })));
  mockInvoke('list_voice_clone_samples', () => []);
});
afterEach(async () => {
  wrappers.splice(0).forEach(wrapper => wrapper.unmount());
  await flushPromises();
  disposePinia(pinia);
});

describe('Agent voice selection ownership', () => {
  it('the default builtin voice is 冰糖 and does not silently adopt the global voice', async () => {
    const wrapper = mount(AgentVoiceSettings, {
      props: { active: true, modelValue: { enabled: true, ...SETTINGS_DEFAULTS.voice.tts } },
    });
    wrappers.push(wrapper);
    await flushPromises();
    expect(wrapper.get('code').text()).toBe('mimo-v2.5-tts');
    expect(wrapper.get('[aria-label="当前音色"]').text()).toContain('冰糖');
    expect(wrapper.get('[aria-label="流式朗读"]').isVisible()).toBe(true);
    expect(wrapper.get('[aria-label="试听文本"]').isVisible()).toBe(true);
    expect(wrapper.get('[role="group"][aria-label="音色模式"]').findAll('button').map(button => button.text())).toEqual(['预制音色', '音色复刻']);
    expect(invokeMock.mock.calls.filter(([command]) => command === 'preview_tts')).toHaveLength(0);
    expect(wrapper.emitted('update:modelValue')).toBeUndefined();
  });

  it('keeps the disabled draft inactive, then previews its default voice without changing global TTS settings', async () => {
    const voice = { enabled: false, ...SETTINGS_DEFAULTS.voice.tts, stream: false };
    const wrapper = mount(AgentVoiceSettings, { props: { active: true, modelValue: voice } });
    wrappers.push(wrapper);
    await flushPromises();
    expect(wrapper.get('.voice-preview-button').attributes('disabled')).toBeDefined();
    await wrapper.get('.voice-preview-button').trigger('click');
    expect(invokeMock.mock.calls.filter(([command]) => command === 'preview_tts')).toHaveLength(0);
    await wrapper.setProps({ modelValue: { ...voice, enabled: true } });
    await flushPromises();
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
    await wrapper.setProps({ modelValue: { ...voice, enabled: false } });
    await flushPromises();
    expect(wrapper.get('[aria-label="试听文本"]').isVisible()).toBe(true);
    expect(wrapper.get('.voice-preview-button').attributes('disabled')).toBeDefined();
    expect(invokeMock).toHaveBeenCalledWith('stop_tts', { ttsId: submitted?.ttsId });
  });

  it('shows the Rust no-voice error for an empty clone selection instead of substituting the global voice', async () => {
    const reason = '当前未选择朗读音色，请先选择音色。';
    mockInvoke('preview_tts', () => Promise.reject(reason));
    const wrapper = mount(AgentVoiceSettings, {
      props: { active: true, modelValue: { enabled: true, ...SETTINGS_DEFAULTS.voice.tts, mode: 'clone', cloneSampleId: '' } },
    });
    wrappers.push(wrapper);
    await flushPromises();
    await wrapper.get('.voice-preview-button').trigger('click');
    await flushPromises();
    const submitted = invokeMock.mock.calls.find(([command]) => command === 'preview_tts')?.[1];
    expect(submitted?.settings).toMatchObject({ tts: { mode: 'clone', cloneSampleId: '' } });
    expect(wrapper.get('[role="alert"]').text()).toBe(reason);
    expect(wrapper.get('.voice-preview-button').text()).toBe('试听');
    expect(useSettingsStore().settings?.voice.tts.voice).toBe('苏打');
  });
});
