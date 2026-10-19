import { describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { h, nextTick, ref } from 'vue';
import SettingsCanvas from '../../../../components/settings/SettingsCanvas.vue';
import SettingsSection from '../../../../components/settings/SettingsSection.vue';
import SettingsRow from '../../../../components/settings/SettingsRow.vue';
import SettingsDisclosure from '../../../../components/settings/SettingsDisclosure.vue';
import SettingsTextField from '../../../../components/settings/SettingsTextField.vue';

describe('settings canvas opt-in', () => {
  it('keeps existing sections wrapped while canvas sections render directly', () => {
    const legacy = mount(SettingsSection, { props: { title: '现有页面' }, slots: { default: '内容' } });
    const canvas = mount(SettingsCanvas, {
      slots: { default: () => h(SettingsSection, { title: '画布页面' }, () => '内容') },
    });
    expect(legacy.find('.settings-card').exists()).toBe(true);
    expect(canvas.find('.settings-card').exists()).toBe(false);
    expect(canvas.get('section').text()).toContain('内容');
    legacy.unmount();
    canvas.unmount();
  });

  it('uses a single accessible trigger for choices and preserves disabled behavior', async () => {
    const select = vi.fn();
    const disabled = ref(false);
    const canvas = mount(SettingsCanvas, {
      slots: { default: () => h(SettingsRow, { title: '编码', value: 'MP3', clickable: true, disabled: disabled.value, onClick: select }) },
    });
    const row = canvas.get('button');
    expect(row.text()).toContain('MP3');
    await row.trigger('click');
    expect(select).toHaveBeenCalledOnce();
    disabled.value = true;
    await nextTick();
    await row.trigger('click');
    expect(select).toHaveBeenCalledOnce();
    canvas.unmount();
  });

  it('removes collapsed controls from the focus tree and labels secure fields', async () => {
    const canvas = mount(SettingsCanvas, {
      slots: { default: () => h(SettingsDisclosure, { title: '诊断' }, () => h(SettingsTextField, { label: '密钥', modelValue: '', isSecure: true, disabled: true })) },
    });
    expect(canvas.find('input').exists()).toBe(false);
    await canvas.get('.settings-disclosure-toggle').trigger('click');
    expect(canvas.get('label').attributes('for')).toBe(canvas.get('input').attributes('id'));
    expect(canvas.get('[aria-label="显示密钥"]').attributes('disabled')).toBeDefined();
    await canvas.get('.settings-disclosure-toggle').trigger('click');
    expect(canvas.find('input').exists()).toBe(false);
    canvas.unmount();
  });
});
