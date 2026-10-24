import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import SettingsNote from '@/components/settings/SettingsNote.vue';

describe('SettingsNote', () => {
  it('renders default slot inside a blockquote with settings-note class', () => {
    const wrapper = mount(SettingsNote, {
      slots: {
        default: '这是一个说明文本',
      },
    });

    const blockquote = wrapper.find('blockquote');
    expect(blockquote.exists()).toBe(true);
    expect(blockquote.classes()).toContain('settings-note');
    expect(blockquote.text()).toBe('这是一个说明文本');
  });

  it('renders text prop when slot is not provided', () => {
    const wrapper = mount(SettingsNote, {
      props: {
        text: '通过属性传入的注释文本',
      },
    });

    const blockquote = wrapper.find('blockquote');
    expect(blockquote.exists()).toBe(true);
    expect(blockquote.text()).toBe('通过属性传入的注释文本');
  });

  it('inherits id and custom attributes on blockquote root', () => {
    const wrapper = mount(SettingsNote, {
      attrs: {
        id: 'sample-help',
        'data-testid': 'custom-note',
      },
      props: {
        text: '属性穿透测试',
      },
    });

    const blockquote = wrapper.get('blockquote');
    expect(blockquote.attributes('id')).toBe('sample-help');
    expect(blockquote.attributes('data-testid')).toBe('custom-note');
  });
});
