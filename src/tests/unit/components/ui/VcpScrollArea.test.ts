import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import VcpScrollArea from '@/components/ui/VcpScrollArea.vue';

describe('VcpScrollArea', () => {
  it('renders slot and default scrollable class', () => {
    const wrapper = mount(VcpScrollArea, {
      slots: {
        default: '<div>滚动内容</div>',
      },
    });

    expect(wrapper.text()).toContain('滚动内容');
    expect(wrapper.classes()).toContain('vcp-scrollable');
    expect(wrapper.classes()).not.toContain('no-swipe');
  });

  it('adds no-swipe class when disableSwipe is true', () => {
    const wrapper = mount(VcpScrollArea, {
      props: {
        disableSwipe: true,
      },
    });

    expect(wrapper.classes()).toContain('vcp-scrollable');
    expect(wrapper.classes()).toContain('no-swipe');
  });
});
