import { describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import SettingsActionButton from '@/components/settings/SettingsActionButton.vue';

const IconStub = {
  template: '<svg data-testid="icon-stub" />',
  props: ['size'],
};

describe('SettingsActionButton', () => {
  it('renders slot text and emits click', async () => {
    const wrapper = mount(SettingsActionButton, {
      slots: {
        default: '保存设置',
      },
    });

    await wrapper.get('button').trigger('click');

    expect(wrapper.text()).toContain('保存设置');
    expect(wrapper.emitted('click')).toHaveLength(1);
  });

  it('disables button while loading and shows spinner instead of icon', async () => {
    const onClick = vi.fn();
    const wrapper = mount(SettingsActionButton, {
      props: {
        loading: true,
        icon: IconStub,
        onClick,
      },
      slots: {
        default: '加载中',
      },
    });

    await wrapper.get('button').trigger('click');

    expect(wrapper.get('button').attributes('disabled')).toBeDefined();
    expect(wrapper.find('.animate-spin').exists()).toBe(true);
    expect(wrapper.find('[data-testid="icon-stub"]').exists()).toBe(false);
    expect(onClick).not.toHaveBeenCalled();
  });

  it('applies danger variant class', () => {
    const wrapper = mount(SettingsActionButton, {
      props: {
        variant: 'danger',
      },
    });

    expect(wrapper.get('button').classes()).toContain('text-red-500');
  });
});
