import { afterEach, describe, expect, it, vi } from 'vitest';
import { enableAutoUnmount, mount } from '@vue/test-utils';
import { defineComponent, type PropType } from 'vue';
import { vLongpress } from '../../core/directives/longpress';

enableAutoUnmount(afterEach);

function touchPointer(target: Element, type: string, pointerId = 9): PointerEvent {
  const event = new PointerEvent(type, {
    bubbles: true,
    cancelable: true,
    pointerType: 'touch',
    pointerId,
    isPrimary: true,
    button: type === 'contextmenu' ? 2 : 0,
    buttons: type === 'pointerup' || type === 'click' ? 0 : 1,
    detail: type === 'click' ? 1 : 0,
    clientX: 20,
    clientY: 20,
  });
  target.dispatchEvent(event);
  return event;
}

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
    touchPointer(wrapper.get('p').element, 'pointerdown');
    vi.advanceTimersByTime(650);
    touchPointer(wrapper.get('p').element, 'contextmenu');
    touchPointer(wrapper.get('p').element, 'pointerup');
    touchPointer(wrapper.get('button').element, 'pointerdown');
    vi.advanceTimersByTime(650);
    touchPointer(wrapper.get('button').element, 'contextmenu');
    touchPointer(wrapper.get('button').element, 'pointerup');
    expect(handler).not.toHaveBeenCalled();
    const range = document.createRange();
    range.selectNodeContents(wrapper.get('p').element);
    window.getSelection()?.addRange(range);
    touchPointer(wrapper.get('.header').element, 'pointerdown');
    vi.advanceTimersByTime(650);
    touchPointer(wrapper.get('.header').element, 'contextmenu');
    touchPointer(wrapper.get('.header').element, 'pointerup');
    expect(handler).not.toHaveBeenCalled();
    window.getSelection()?.removeAllRanges();
    touchPointer(wrapper.get('.header').element, 'pointerdown');
    vi.advanceTimersByTime(650);
    expect(handler).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it('keeps a button directive working when the touch starts on its SVG icon', async () => {
    vi.useFakeTimers();
    const handler = vi.fn();
    const wrapper = mount(defineComponent({
      directives: { longpress: vLongpress },
      setup: () => ({ handler }),
      template: '<button v-longpress="handler"><svg><path d="M0 0" /></svg></button>',
    }), { attachTo: document.body });
    touchPointer(wrapper.get('path').element, 'pointerdown');
    vi.advanceTimersByTime(650);
    touchPointer(wrapper.get('path').element, 'contextmenu');
    touchPointer(wrapper.get('path').element, 'pointerup');
    expect(handler).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it('uses the updated callback while a held element keeps the same DOM identity', async () => {
    vi.useFakeTimers();
    const original = vi.fn();
    const replacement = vi.fn();
    const wrapper = mount(defineComponent({
      directives: { longpress: vLongpress },
      props: { handler: { type: Function as PropType<(event: Event) => void>, required: true } },
      template: '<button v-longpress="handler">theme</button>',
    }), { props: { handler: original }, attachTo: document.body });
    const button = wrapper.get('button').element;
    touchPointer(button, 'pointerdown');
    vi.advanceTimersByTime(300);
    await wrapper.setProps({ handler: replacement });
    expect(wrapper.get('button').element).toBe(button);
    vi.advanceTimersByTime(300);
    expect(original).not.toHaveBeenCalled();
    expect(replacement).toHaveBeenCalledTimes(1);
    touchPointer(button, 'pointerup');
  });

  it('unmount cancels pending work and detaches listeners from the former element', () => {
    vi.useFakeTimers();
    const handler = vi.fn();
    const wrapper = mount(defineComponent({
      directives: { longpress: vLongpress },
      setup: () => ({ handler }),
      template: '<button v-longpress="handler">theme</button>',
    }), { attachTo: document.body });
    const button = wrapper.get('button').element;
    touchPointer(button, 'pointerdown');
    vi.advanceTimersByTime(300);
    wrapper.unmount();
    vi.advanceTimersByTime(700);
    expect(handler).not.toHaveBeenCalled();

    document.body.appendChild(button);
    touchPointer(button, 'pointerdown', 10);
    vi.advanceTimersByTime(700);
    touchPointer(button, 'contextmenu', 10);
    touchPointer(button, 'pointerup', 10);
    expect(handler).not.toHaveBeenCalled();
  });
});
