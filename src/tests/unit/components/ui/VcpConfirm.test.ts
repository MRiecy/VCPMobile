import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import VcpConfirm from '@/components/ui/VcpConfirm.vue';

describe('VcpConfirm', () => {
  it('renders title and message when isOpen is true', () => {
    const wrapper = mount(VcpConfirm, {
      props: {
        isOpen: true,
        title: '测试确认',
        message: '你确定要测试吗？',
      },
    });

    const bodyText = document.body.innerHTML;
    expect(bodyText).toContain('测试确认');
    expect(bodyText).toContain('你确定要测试吗？');
    wrapper.unmount();
  });

  it('emits confirm when confirm button is clicked', async () => {
    const wrapper = mount(VcpConfirm, {
      props: {
        isOpen: true,
        title: '测试确认',
        message: '你确定吗？',
      },
    });

    const buttons = document.querySelectorAll('button');
    let confirmBtn: HTMLButtonElement | null = null;
    buttons.forEach((btn) => {
      if (btn.textContent?.includes('确认')) {
        confirmBtn = btn;
      }
    });

    expect(confirmBtn).not.toBeNull();
    confirmBtn!.click();

    expect(wrapper.emitted('confirm')).toBeTruthy();
    wrapper.unmount();
  });

  it('emits cancel when cancel button is clicked', async () => {
    const wrapper = mount(VcpConfirm, {
      props: {
        isOpen: true,
        title: '测试确认',
        message: '你确定吗？',
      },
    });

    const buttons = document.querySelectorAll('button');
    let cancelBtn: HTMLButtonElement | null = null;
    buttons.forEach((btn) => {
      if (btn.textContent?.includes('取消')) {
        cancelBtn = btn;
      }
    });

    expect(cancelBtn).not.toBeNull();
    cancelBtn!.click();

    expect(wrapper.emitted('cancel')).toBeTruthy();
    wrapper.unmount();
  });

  it('hides cancel button when onlyConfirm is true', () => {
    const wrapper = mount(VcpConfirm, {
      props: {
        isOpen: true,
        title: '测试确认',
        message: '你确定吗？',
        onlyConfirm: true,
      },
    });

    const buttons = document.querySelectorAll('button');
    let hasCancel = false;
    buttons.forEach((btn) => {
      if (btn.textContent?.includes('取消')) {
        hasCancel = true;
      }
    });

    expect(hasCancel).toBe(false);
    wrapper.unmount();
  });
});
