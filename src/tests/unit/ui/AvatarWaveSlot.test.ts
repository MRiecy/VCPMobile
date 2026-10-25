import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import AvatarWaveSlot from '../../../components/ui/AvatarWaveSlot.vue';

describe('AvatarWaveSlot component', () => {
  it('renders default slot inside the wave surface', () => {
    const wrapper = mount(AvatarWaveSlot, {
      slots: {
        default: '<div class="test-avatar-inner">Avatar</div>',
      },
    });

    expect(wrapper.find('.test-avatar-inner').exists()).toBe(true);
    expect(wrapper.find('.vcp-avatar-wave-surface').exists()).toBe(true);
    expect(wrapper.classes()).not.toContain('is-active');
    expect(wrapper.attributes('data-wave-active')).toBe('false');
    expect(wrapper.attributes('data-generating')).toBe('false');
  });

  it('toggles active classes and attributes when active is true', () => {
    const wrapper = mount(AvatarWaveSlot, {
      props: {
        active: true,
      },
      slots: {
        default: '<span>Avatar</span>',
      },
    });

    expect(wrapper.classes()).toContain('is-active');
    expect(wrapper.classes()).toContain('is-generating');
    expect(wrapper.attributes('data-wave-active')).toBe('true');
    expect(wrapper.attributes('data-generating')).toBe('true');
    expect(wrapper.find('.vcp-avatar-wave-indicator').exists()).toBe(true);
    expect(wrapper.find('.vignette-mask').exists()).toBe(true);
    expect(wrapper.find('svg line.axis-line').exists()).toBe(true);
    expect(wrapper.findAll('rect.interlace-up').length).toBe(2);
    expect(wrapper.findAll('rect.interlace-down').length).toBe(2);
    expect(wrapper.find('circle.interlace-dot').exists()).toBe(true);
  });

  it('hides indicator layer when disabled is true', () => {
    const wrapper = mount(AvatarWaveSlot, {
      props: {
        active: true,
        disabled: true,
      },
    });

    expect(wrapper.find('.vcp-avatar-wave-indicator').exists()).toBe(false);
  });

  it('respects custom data-generating prop for legacy test contracts', () => {
    const wrapper = mount(AvatarWaveSlot, {
      props: {
        active: false,
        dataGenerating: 'custom-state',
      },
    });

    expect(wrapper.attributes('data-generating')).toBe('custom-state');
  });
});
