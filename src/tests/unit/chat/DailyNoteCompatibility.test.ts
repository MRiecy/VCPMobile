import { beforeEach, describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import DiaryBlock from '@/features/chat/blocks/DiaryBlock.vue';
import { useNotificationProcessor } from '@/core/composables/useNotificationProcessor';
import { clearHtmlCache } from '@/core/utils/astRenderer';
import type { ContentBlock, MarkdownNode } from '@/core/types/chat';

function paragraph(value: string): MarkdownNode {
  return {
    type: 'paragraph',
    children: [{ type: 'text', value }],
  };
}

describe('DiaryBlock', () => {
  beforeEach(() => {
    clearHtmlCache();
  });

  it('uses Maid before Valet and renders create metadata', () => {
    const block: ContentBlock = {
      type: 'diary',
      maid: 'Sakura',
      valet: 'Sebastian',
      date: '2026-08-10',
      file_name: 'Field Log',
      folder: 'missions/day-1',
      content: 'finished',
      hash: 11,
    };
    const wrapper = mount(DiaryBlock, {
      props: { block, messageId: 'message-create' },
    });

    expect(wrapper.get('.vcp-diary-block').classes()).not.toContain('is-valet');
    expect(wrapper.get('.vcp-diary-title').text()).toBe('Field Log');
    expect(wrapper.get('.vcp-diary-agent-label').text()).toBe('Maid:');
    expect(wrapper.get('.vcp-diary-maid-name').text()).toBe('Sakura');
    expect(wrapper.text()).toContain('missions/day-1');
    expect(wrapper.text()).not.toContain('Sebastian');
  });

  it('uses the Valet variant only when Maid is empty', () => {
    const block: ContentBlock = {
      type: 'diary',
      maid: '',
      valet: 'Sebastian',
      date: '',
      content: 'finished',
      hash: 12,
    };
    const wrapper = mount(DiaryBlock, {
      props: { block, messageId: 'message-valet' },
    });

    expect(wrapper.get('.vcp-diary-block').classes()).toContain('is-valet');
    expect(wrapper.get('.vcp-diary-title').text()).toBe("Valet's Diary");
    expect(wrapper.get('.vcp-diary-agent-label').text()).toBe('Valet:');
  });

  it('keeps update target and replacement AST caches isolated', () => {
    const block: ContentBlock = {
      type: 'diary-update',
      maid: 'Sakura',
      target: 'old source',
      replace: 'new source',
      target_nodes: [paragraph('OLD AST')],
      replace_nodes: [paragraph('NEW AST')],
      hash: 21,
    };
    const wrapper = mount(DiaryBlock, {
      props: { block, messageId: 'message-update' },
    });
    const sides = wrapper.findAll('.vcp-diary-update-content');

    expect(sides).toHaveLength(2);
    expect(sides[0].text()).toContain('OLD AST');
    expect(sides[1].text()).toContain('NEW AST');
  });

  it('renders fixed placeholders for missing update sides', () => {
    const block: ContentBlock = {
      type: 'diary-update',
      maid: 'Sakura',
      target: '',
      replace: '',
      hash: 22,
    };
    const wrapper = mount(DiaryBlock, {
      props: { block, messageId: 'message-update-missing' },
    });

    expect(wrapper.text()).toContain('原文解析失败');
    expect(wrapper.text()).toContain('替换内容解析失败');
  });

});

describe('DailyNote notifications', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  function processDailyNote(content: unknown, status = 'success') {
    return useNotificationProcessor().processPayload({
      type: 'vcp_log',
      data: {
        tool_name: 'DailyNote',
        status,
        content,
      },
    });
  }

  it('prefers message from object plugin output', () => {
    const result = processDailyNote(JSON.stringify({
      original_plugin_output: {
        status: 'success',
        message: '日记已写入 daily/2026-08-10.md',
      },
    }));

    expect(result.message).toBe('✅ 日记已写入 daily/2026-08-10.md');
    expect(result.isPreformatted).toBe(false);
  });

  it('accepts JSON-string plugin output and direct failure messages', () => {
    const wrapped = processDailyNote(JSON.stringify({
      original_plugin_output: JSON.stringify({ message: '字符串包装消息' }),
    }));
    const direct = processDailyNote({ message: '未找到目标文本' }, 'error');

    expect(wrapped.message).toBe('✅ 字符串包装消息');
    expect(direct.message).toBe('❌ 未找到目标文本');
    expect(direct.type).toBe('error');
  });
});

describe('sync notification ownership', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it.each([
    { type: 'vcp-sync-status', status: 'error', source: 'Sync' },
    {
      type: 'vcp-log-message',
      data: {
        id: 'vcp_sync_connection_status',
        status: 'success',
        source: 'Sync',
      },
    },
  ])('keeps sync status inside the sync panel', (payload) => {
    const result = useNotificationProcessor().processPayload(payload);
    expect(result).toEqual({ silent: true });
  });
});
