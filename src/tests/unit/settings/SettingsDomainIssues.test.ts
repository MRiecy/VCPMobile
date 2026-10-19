import { describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import SettingsDomainIssues from '../../../features/settings/editing/SettingsDomainIssues.vue';
import { CONNECTION_FIELDS, VOICE_FIELDS } from '../../../features/settings/editing/fieldSets';

describe('configuration issue presentation', () => {
  it('keeps damaged group notices on the owning page without recovery controls', () => {
    const wrapper = mount(SettingsDomainIssues, { props: {
      fields: VOICE_FIELDS, errors: [],
      domains: [{ domain: 'voice', revision: '4', schemaVersion: 1, health: 'recovered', issues: [
        { domain: 'voice', field: 'tts.mode', code: 'invalidGroup', message: '该选项所在配置结构无效。' },
        { domain: 'voice', field: 'tts.voice', code: 'invalidGroup', message: '该选项所在配置结构无效。' },
      ] }],
    } });
    expect(wrapper.text()).toContain('语音 · 音色模式');
    expect(wrapper.text()).not.toContain('tts.mode');
    expect(wrapper.findAll('button')).toHaveLength(0);
    expect(wrapper.text()).toContain('设置首页');
    wrapper.unmount();
  });

  it('presents reset-required and ordinary validation errors without inventing a recovery action', () => {
    const wrapper = mount(SettingsDomainIssues, { props: {
      fields: VOICE_FIELDS, domains: [],
      errors: [
        { domain: 'voice', field: 'connection.apiKey', code: 'requiresReset', message: '请在设置首页重置此配置组。' },
        { domain: 'voice', field: 'vad.endSilenceMs', code: 'outOfRange', message: '数值超出范围。' },
      ],
    } });
    expect(wrapper.findAll('button')).toHaveLength(0);
    expect(wrapper.text()).toContain('语音 API Key');
    expect(wrapper.text()).not.toContain('voice.connection.apiKey');
    expect(wrapper.text()).toContain('说完等待时间：数值超出范围');
    wrapper.unmount();
  });

  it('offers one whole-domain reset only for an abnormal domain at the home entry', async () => {
    const reset = vi.fn();
    const wrapper = mount(SettingsDomainIssues, { props: {
      errors: [], reset,
      domains: [{ domain: 'server', revision: '3', schemaVersion: 1, health: 'recovered', issues: [
        { domain: 'server', field: 'url', code: 'invalidType', message: '值类型无效。' },
      ] }],
    } });
    expect(wrapper.get('button').text()).toBe('将主服务器整组恢复默认值');
    await wrapper.get('button').trigger('click');
    expect(reset).toHaveBeenCalledWith('server', '3');
    await wrapper.setProps({ domains: [{ domain: 'server', revision: '4', schemaVersion: 1, health: 'ready', issues: [] }] });
    expect(wrapper.findAll('button')).toHaveLength(0);
    expect(wrapper.text()).toBe('');
    wrapper.unmount();
  });

  it('filters unrelated field issues from a leaf page', () => {
    const wrapper = mount(SettingsDomainIssues, { props: {
      fields: CONNECTION_FIELDS, errors: [],
      domains: [{ domain: 'server', revision: '3', schemaVersion: 1, health: 'recovered', issues: [
        { domain: 'server', field: 'adminPassword', code: 'invalidType', message: '值类型无效。' },
      ] }],
    } });
    expect(wrapper.text()).toBe('');
    wrapper.unmount();
  });
});
