import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import SlidePage from '@/components/ui/SlidePage.vue';

describe('SlidePage', () => {
  it('renders slot content and custom z-index', () => {
    const wrapper = mount(SlidePage, {
      props: {
        isOpen: true,
        zIndex: 77,
      },
      slots: {
        default: '<span>页面内容</span>',
      },
    });

    const page = wrapper.get('.fixed');
    expect(wrapper.text()).toContain('页面内容');
    expect(page.attributes('style')).toContain('z-index: 77');
  });

  it('uses v-show display none when closed', () => {
    const wrapper = mount(SlidePage, {
      props: {
        isOpen: false,
      },
    });

    expect(wrapper.get('.fixed').attributes('style')).toContain('display: none');
  });
});
