import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import SettingsTextField from '@/components/settings/SettingsTextField.vue';

describe('SettingsTextField', () => {
  it('renders label and emits value updates', async () => {
    const wrapper = mount(SettingsTextField, {
      props: {
        modelValue: 'old',
        label: '名称',
        placeholder: '输入名称',
      },
    });

    const input = wrapper.get('input');
    await input.setValue('new');

    expect(wrapper.text()).toContain('名称');
    expect(input.attributes('placeholder')).toBe('输入名称');
    expect(wrapper.emitted('update:modelValue')).toEqual([['new']]);
  });

  it('emits focus and blur events', async () => {
    const wrapper = mount(SettingsTextField, {
      props: {
        modelValue: '',
      },
    });

    await wrapper.get('input').trigger('focus');
    await wrapper.get('input').trigger('blur');

    expect(wrapper.emitted('focus')).toHaveLength(1);
    expect(wrapper.emitted('blur')).toHaveLength(1);
  });

  it('toggles secure mask class', async () => {
    const wrapper = mount(SettingsTextField, {
      props: {
        modelValue: 'secret',
        isSecure: true,
      },
    });

    expect(wrapper.get('input').classes()).toContain('masked-input');
    await wrapper.get('button').trigger('click');
    expect(wrapper.get('input').classes()).not.toContain('masked-input');
  });
});
