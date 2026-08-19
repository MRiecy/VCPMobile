import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vitest';
import RefreshButton from '@/components/ui/RefreshButton.vue';

const SPIN_CLASS = 'ub-refresh-spinning';

function mountButton(props: Record<string, unknown> = {}) {
  return mount(RefreshButton, {
    props: { label: '刷新', ...props },
  });
}

describe('RefreshButton（整圈停止动画契约）', () => {
  it('点击发射 refresh 并开始旋转', async () => {
    const wrapper = mountButton();
    await wrapper.find('button').trigger('click');
    expect(wrapper.emitted('refresh')).toHaveLength(1);
    expect(wrapper.find('svg').classes()).toContain(SPIN_CLASS);
  });

  it('disabled 时不发射也不旋转', async () => {
    const wrapper = mountButton({ disabled: true });
    await wrapper.find('button').trigger('click');
    expect(wrapper.emitted('refresh')).toBeUndefined();
    expect(wrapper.find('svg').classes()).not.toContain(SPIN_CLASS);
  });

  it('非加载下单击：转满一圈后停在圈界（保底一圈）', async () => {
    const wrapper = mountButton({ loading: false });
    await wrapper.find('button').trigger('click');
    expect(wrapper.find('svg').classes()).toContain(SPIN_CLASS);
    await wrapper.find('svg').trigger('animationiteration');
    expect(wrapper.find('svg').classes()).not.toContain(SPIN_CLASS);
  });

  it('加载持续到多个圈界仍保持旋转', async () => {
    const wrapper = mountButton({ loading: true });
    await wrapper.find('button').trigger('click');
    await wrapper.find('svg').trigger('animationiteration');
    expect(wrapper.find('svg').classes()).toContain(SPIN_CLASS);
    await wrapper.find('svg').trigger('animationiteration');
    expect(wrapper.find('svg').classes()).toContain(SPIN_CLASS);
  });

  it('加载结束后在下一个圈界停止', async () => {
    const wrapper = mountButton({ loading: true });
    await wrapper.find('button').trigger('click');
    // 加载结束
    await wrapper.setProps({ loading: false });
    await wrapper.find('svg').trigger('animationiteration');
    expect(wrapper.find('svg').classes()).not.toContain(SPIN_CLASS);
  });

  it('点击时未加载但加载随后拉起：转到加载结束那一圈', async () => {
    const wrapper = mountButton({ loading: false });
    await wrapper.find('button').trigger('click');
    // 加载异步开始 → 撤销停止请求
    await wrapper.setProps({ loading: true });
    await wrapper.find('svg').trigger('animationiteration');
    expect(wrapper.find('svg').classes()).toContain(SPIN_CLASS);
    // 加载结束 → 下一圈界停
    await wrapper.setProps({ loading: false });
    await wrapper.find('svg').trigger('animationiteration');
    expect(wrapper.find('svg').classes()).not.toContain(SPIN_CLASS);
  });

  it('loading 未点击自转（后台轮询不触发旋转）', async () => {
    const wrapper = mountButton({ loading: false });
    await wrapper.setProps({ loading: true });
    expect(wrapper.find('svg').classes()).not.toContain(SPIN_CLASS);
  });
});
