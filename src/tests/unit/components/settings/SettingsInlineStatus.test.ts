import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import SettingsInlineStatus from '@/components/settings/SettingsInlineStatus.vue';

describe('SettingsInlineStatus', () => {
  it('renders message and semantic type class', () => {
    const wrapper = mount(SettingsInlineStatus, {
      props: {
        type: 'success',
        message: '连接成功',
      },
    });

    expect(wrapper.text()).toContain('连接成功');
    expect(wrapper.find('.settings-status').classes()).toContain('text-green-500');
  });

  it('shows loading indicator', () => {
    const wrapper = mount(SettingsInlineStatus, {
      props: {
        type: 'loading',
        message: '加载中',
      },
    });

    expect(wrapper.find('.animate-ping').exists()).toBe(true);
  });

  it('supports mono and multiline presentation', () => {
    const wrapper = mount(SettingsInlineStatus, {
      props: {
        type: 'info',
        message: 'line1\nline2',
        mono: true,
        multiline: true,
      },
    });

    expect(wrapper.find('.settings-status').classes()).toContain('font-mono');
    expect(wrapper.find('span').classes()).toContain('whitespace-pre-wrap');
  });
});
