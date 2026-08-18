import { describe, expect, it } from 'vitest';
import GroupModeHelpDialog from '../../../../features/agent/GroupModeHelpDialog.vue';
import { mountWithPinia } from '../../../utils/mount';

// 弹窗内容经 Teleport 挂载到 body，断言需查 document 而非 wrapper
describe('GroupModeHelpDialog', () => {
  it('renders all three speaking modes plus implicit-behavior notes when open', () => {
    mountWithPinia(GroupModeHelpDialog, {
      props: { modelValue: true },
      attachTo: document.body,
    });

    const text = document.body.textContent ?? '';
    expect(text).toContain('发言模式说明');
    expect(text).toContain('顺序发言');
    expect(text).toContain('自然随机');
    expect(text).toContain('邀请发言');
    // 隐式行为必须可见：新成员默认 Tag 为名字、严格模式无需 @
    expect(text).toContain('默认以「名字」作为 Tag');
    expect(text).toContain('无需 @');
  });

  it('renders nothing when closed', () => {
    mountWithPinia(GroupModeHelpDialog, {
      props: { modelValue: false },
      attachTo: document.body,
    });
    expect(document.body.querySelector('[role="dialog"]')).toBeNull();
  });

  it('emits close when the mask is clicked', async () => {
    const wrapper = mountWithPinia(GroupModeHelpDialog, {
      props: { modelValue: true },
      attachTo: document.body,
    });

    const mask = document.body.querySelector('.fixed.inset-0') as HTMLElement;
    mask.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await wrapper.vm.$nextTick();

    expect(wrapper.emitted('update:modelValue')?.[0]).toEqual([false]);
  });
});
