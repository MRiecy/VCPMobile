import { mount } from '@vue/test-utils';
import { describe, expect, it } from 'vitest';
import VoiceGuidanceBlock from '@/features/chat/blocks/VoiceGuidanceBlock.vue';

describe('voice guidance display', () => {
  it('starts collapsed and exposes selectable literal guidance with a local control', async () => {
    const content = '自然轻快。\n<button data-send="not a tool">保持原文</button>';
    const wrapper = mount(VoiceGuidanceBlock, {
      props: { block: { type: 'voice-guidance', content, is_complete: true } },
    });
    const toggle = wrapper.get('button');
    const body = wrapper.get('[data-message-content]');
    expect(toggle.attributes('aria-expanded')).toBe('false');
    expect(toggle.attributes('data-longpress-native')).toBeDefined();
    expect(body.attributes('style')).toContain('display: none');
    expect(toggle.text()).toBe('风格指导');
    await toggle.trigger('click');
    expect(toggle.attributes('aria-expanded')).toBe('true');
    expect(body.attributes('style') ?? '').not.toContain('display: none');
    expect(body.text()).toBe(content);
    expect(body.attributes('data-longpress-native')).toBeDefined();
    expect(body.find('button').exists()).toBe(false);
    expect(wrapper.findAll('button')).toHaveLength(1);
    wrapper.unmount();
  });

  it('describes an interrupted block without claiming the guidance was used', () => {
    const wrapper = mount(VoiceGuidanceBlock, {
      props: { block: { type: 'voice-guidance', content: '', is_complete: false } },
    });
    expect(wrapper.get('button').text()).toContain('未完成');
    expect(wrapper.get('[data-message-content]').text()).toBe('未生成指导内容');
    expect(wrapper.text()).not.toContain('已采用');
    wrapper.unmount();
  });
});
