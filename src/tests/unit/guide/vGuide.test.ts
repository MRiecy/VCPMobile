import { describe, expect, it, vi } from 'vitest';
import { defineComponent, h, withDirectives } from 'vue';
import { mount } from '@vue/test-utils';
import { vGuide } from '@/features/guide/directives/vGuide';
import { hasTarget, resolveTarget, targetRegistry } from '@/features/guide/registry';

// happy-dom 默认 rect 全零；resolveTarget 要求渲染盒非零，这里给出确定性几何。
Element.prototype.getBoundingClientRect = function getBoundingClientRect() {
  return {
    top: 0,
    left: 0,
    width: 100,
    height: 40,
    right: 100,
    bottom: 40,
    x: 0,
    y: 0,
    toJSON: () => ({}),
  } as DOMRect;
};

const Host = defineComponent({
  props: {
    ids: { type: Array as () => string[], required: true },
  },
  setup(props) {
    return () =>
      h(
        'div',
        props.ids.map((id, index) =>
          withDirectives(
            h('div', { 'data-row': index }, `row-${index}`),
            [[vGuide, id]],
          ),
        ),
      );
  },
});

describe('v-guide directive', () => {
  it('registers elements on mount and supports duplicate ids', () => {
    const wrapper = mount(Host, { props: { ids: ['dup', 'single', 'dup'] }, attachTo: document.body });
    expect(targetRegistry.get('dup')).toHaveLength(2);
    expect(targetRegistry.get('single')).toHaveLength(1);
    expect(hasTarget('dup')).toBe(true);

    // 步骤解析取首个已连接元素（挂载顺序）
    const resolved = resolveTarget('dup');
    expect(resolved).not.toBeNull();
    expect(resolved?.getAttribute('data-row')).toBe('0');

    wrapper.unmount();
  });

  it('unregisters all elements on unmount', () => {
    const wrapper = mount(Host, { props: { ids: ['x', 'x', 'y'] }, attachTo: document.body });
    expect(hasTarget('x')).toBe(true);
    expect(hasTarget('y')).toBe(true);

    wrapper.unmount();
    expect(hasTarget('x')).toBe(false);
    expect(hasTarget('y')).toBe(false);
    expect(targetRegistry.size).toBe(0);
  });

  it('warns but does not register on invalid binding values', () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const BadHost = defineComponent({
      setup() {
        return () => withDirectives(h('div'), [[vGuide, '' as unknown as string]]);
      },
    });
    const wrapper = mount(BadHost, { attachTo: document.body });
    expect(targetRegistry.size).toBe(0);
    expect(warnSpy).toHaveBeenCalled();
    warnSpy.mockRestore();
    wrapper.unmount();
  });
});
