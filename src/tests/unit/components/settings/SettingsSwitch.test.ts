import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import SettingsSwitch from '@/components/settings/SettingsSwitch.vue';

describe('SettingsSwitch controlled native interaction', () => {
  it('exposes a named switch and only changes when its owner publishes a value', async () => {
    const wrapper = mount(SettingsSwitch, {
      props: { modelValue: false },
      attrs: { 'aria-label': '消息震动', 'data-testid': 'preference' },
    });
    const button = wrapper.get('button[role="switch"]');
    expect(button.attributes('aria-label')).toBe('消息震动');
    expect(button.attributes('data-testid')).toBe('preference');
    expect(button.attributes('type')).toBe('button');
    await button.trigger('click');
    expect(wrapper.emitted('update:modelValue')).toEqual([[true]]);
    expect(button.attributes('aria-checked')).toBe('false');
    await wrapper.setProps({ modelValue: true });
    expect(button.attributes('aria-checked')).toBe('true');
    wrapper.unmount();
  });

  it.each([{ disabled: true }, { loading: true }])('rejects duplicate input while unavailable: %o', async (state) => {
    const wrapper = mount(SettingsSwitch, { props: { modelValue: true, ...state } });
    const button = wrapper.get('button');
    expect(button.attributes()).toHaveProperty('disabled');
    await button.trigger('click');
    expect(wrapper.emitted('update:modelValue')).toBeUndefined();
    wrapper.unmount();
  });
});
