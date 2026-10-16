import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { defineComponent } from 'vue';
import { vLongpress } from '../../core/directives/longpress';

describe('long press gesture ownership', () => {
  afterEach(() => { vi.useRealTimers(); window.getSelection()?.removeAllRanges(); });

  it('leaves message text, an existing selection and nested controls to their own interaction', async () => {
    vi.useFakeTimers();
    const handler = vi.fn();
    const wrapper = mount(defineComponent({
      directives: { longpress: vLongpress },
      setup: () => ({ handler }),
      template: '<div v-longpress="handler"><p class="vcp-markdown-block">select text</p><button>Copy</button><span class="header">Header</span></div>',
    }), { attachTo: document.body });
    await wrapper.get('p').trigger('contextmenu');
    await wrapper.get('button').trigger('contextmenu');
    expect(handler).not.toHaveBeenCalled();
    const range = document.createRange();
    range.selectNodeContents(wrapper.get('p').element);
    window.getSelection()?.addRange(range);
    await wrapper.get('.header').trigger('contextmenu');
    expect(handler).not.toHaveBeenCalled();
    window.getSelection()?.removeAllRanges();
    await wrapper.get('.header').trigger('mousedown', { button: 0 });
    vi.advanceTimersByTime(650);
    expect(handler).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it('keeps a button directive working when the touch starts on its SVG icon', async () => {
    const handler = vi.fn();
    const wrapper = mount(defineComponent({
      directives: { longpress: vLongpress },
      setup: () => ({ handler }),
      template: '<button v-longpress="handler"><svg><path d="M0 0" /></svg></button>',
    }));
    await wrapper.get('path').trigger('contextmenu');
    expect(handler).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });
});
