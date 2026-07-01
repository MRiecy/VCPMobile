import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import SettingsSwitch from '@/components/settings/SettingsSwitch.vue';

describe('SettingsSwitch', () => {
  it('emits toggled value when clicked', async () => {
    const wrapper = mount(SettingsSwitch, {
      props: {
        modelValue: false,
      },
    });

    await wrapper.find('.settings-switch').trigger('click');

    expect(wrapper.emitted('update:modelValue')).toEqual([[true]]);
  });

  it('does not emit when disabled', async () => {
    const wrapper = mount(SettingsSwitch, {
      props: {
        modelValue: true,
        disabled: true,
      },
    });

    await wrapper.find('.settings-switch').trigger('click');

    expect(wrapper.emitted('update:modelValue')).toBeUndefined();
    expect(wrapper.find('.settings-switch').classes()).toContain('cursor-not-allowed');
  });

  it('uses active color class when enabled', () => {
    const wrapper = mount(SettingsSwitch, {
      props: {
        modelValue: true,
        activeColor: 'bg-emerald-500',
      },
    });

    expect(wrapper.find('.rounded-full').classes()).toContain('bg-emerald-500');
  });
});
