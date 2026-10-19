import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import SettingsDomainIssues from '../../../features/settings/editing/SettingsDomainIssues.vue';
import { CONNECTION_FIELDS, VOICE_FIELDS } from '../../../features/settings/editing/fieldSets';

const { confirm } = vi.hoisted(() => ({ confirm: vi.fn() }));
vi.mock('../../../core/stores/overlay', () => ({ useOverlayStore: () => ({ showConfirm: confirm }) }));
beforeEach(() => { confirm.mockReset(); confirm.mockResolvedValue(true); });

describe('configuration issue recovery controls', () => {
  it('offers explicit repair for a damaged parent object despite recovered domain health', async () => {
    const repair = vi.fn().mockResolvedValue(true);
    const wrapper = mount(SettingsDomainIssues, { props: {
      fields: VOICE_FIELDS, errors: [], repair,
      domains: [{ domain: 'voice', revision: '4', schemaVersion: 1, health: 'recovered', issues: [
        { domain: 'voice', field: 'tts.mode', code: 'invalidGroup', message: '该选项所在配置结构无效。' },
        { domain: 'voice', field: 'tts.voice', code: 'invalidGroup', message: '该选项所在配置结构无效。' },
      ] }],
    } });
    expect(wrapper.text()).toContain('语音 · 音色模式');
    expect(wrapper.text()).not.toContain('tts.mode');
    expect(wrapper.findAll('button').map(button => button.text())).toEqual(['修复语音配置']);
    await wrapper.get('button').trigger('click');
    await flushPromises();
    expect(confirm).toHaveBeenCalledWith(expect.objectContaining({ title: '修复语音配置', message: expect.stringContaining('仅将损坏字段或关联配置组恢复默认值') }));
    expect(repair).toHaveBeenCalledWith('voice', '4');
    wrapper.unmount();
  });

  it('offers repair after a requiresRepair rejection even without a prior read issue', async () => {
    const repair = vi.fn().mockResolvedValue(true);
    const wrapper = mount(SettingsDomainIssues, { props: {
      fields: VOICE_FIELDS, domains: [], repair,
      errors: [{ domain: 'voice', field: 'connection.apiKey', code: 'requiresRepair', message: '请先修复此配置组。' }],
    } });
    await wrapper.get('button').trigger('click');
    await flushPromises();
    expect(repair).toHaveBeenCalledWith('voice', undefined);
    expect(wrapper.text()).toContain('语音 API Key');
    expect(wrapper.text()).not.toContain('voice.connection.apiKey');
    wrapper.unmount();
  });

  it('names and confirms credential group reset rather than silently restoring only an address', async () => {
    const wrapper = mount(SettingsDomainIssues, { props: {
      fields: CONNECTION_FIELDS, errors: [], repair: vi.fn(),
      domains: [{ domain: 'server', revision: '3', schemaVersion: 1, health: 'recovered', issues: [
        { domain: 'server', field: 'url', code: 'invalidType', message: '值类型无效。' },
      ] }],
    } });
    confirm.mockResolvedValueOnce(false);
    expect(wrapper.get('button').text()).toBe('恢复主服务器连接默认值');
    await wrapper.get('button').trigger('click');
    await flushPromises();
    expect(wrapper.emitted('reset')).toBeUndefined();
    expect(confirm).toHaveBeenCalledWith(expect.objectContaining({ message: expect.stringContaining('Chat API Key、Chat 服务器地址') }));
    await wrapper.get('button').trigger('click');
    await flushPromises();
    expect(wrapper.emitted('reset')).toEqual([['server.url']]);
    wrapper.unmount();
  });
});
