import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { ref } from 'vue';
import {
  extractMentionQuery,
  useMentionSelector,
  MENTION_ALL_ID,
} from '../../../features/chat/composables/useMentionSelector';
import { useAssistantStore } from '../../../core/stores/assistant';
import { useChatSessionStore } from '../../../core/stores/chatSessionStore';

describe('extractMentionQuery', () => {
  it('detects an active mention token at the cursor', () => {
    expect(extractMentionQuery('@', 1)).toEqual({ start: 0, end: 1, query: '' });
    expect(extractMentionQuery('@Nov', 4)).toEqual({ start: 0, end: 4, query: 'Nov' });
    expect(extractMentionQuery('你好 @Nov', 7)).toEqual({ start: 3, end: 7, query: 'Nov' });
  });

  it('ignores @ inside an email-like token (no preceding whitespace)', () => {
    expect(extractMentionQuery('user@host', 9)).toBeNull();
    expect(extractMentionQuery('a@b', 3)).toBeNull();
  });

  it('treats whitespace after the token as mention termination', () => {
    expect(extractMentionQuery('@Nova 你好', 4)).toEqual({ start: 0, end: 4, query: 'Nov'.slice(0, 3) });
    expect(extractMentionQuery('@Nova 你好', 9)).toBeNull();
  });

  it('returns null for out-of-range cursor or missing @', () => {
    expect(extractMentionQuery('hello', 5)).toBeNull();
    expect(extractMentionQuery('@ab', 99)).toBeNull();
  });

  it('accepts the full-width ＠ from Chinese IMEs as a trigger', () => {
    expect(extractMentionQuery('＠', 1)).toEqual({ start: 0, end: 1, query: '' });
    expect(extractMentionQuery('你好 ＠Nov', 7)).toEqual({ start: 3, end: 7, query: 'Nov' });
    // 全角触发同样遵守邮箱规则
    expect(extractMentionQuery('user＠host', 9)).toBeNull();
  });
});

describe('useMentionSelector', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    const assistantStore = useAssistantStore();
    assistantStore.agents = [
      { id: 'a1', name: 'Nova', model: 'm' },
      { id: 'a2', name: 'Luna', model: 'm' },
    ] as any;
    assistantStore.groups = [
      { id: 'g1', name: '测试群', members: ['a1', 'a2'], mode: 'naturerandom' },
      { id: 'g2', name: '邀约群', members: ['a1'], mode: 'invite_only' },
    ] as any;
  });

  const selectGroup = (id: string) => {
    const sessionStore = useChatSessionStore();
    sessionStore.currentSelectedItem = { id, type: 'group', name: '群' };
  };

  it('opens on @ with all members plus synthetic @所有人 in naturerandom', () => {
    selectGroup('g1');
    const input = ref('');
    const selector = useMentionSelector(input);

    input.value = '@';
    selector.updateCursor(1);

    expect(selector.isOpen.value).toBe(true);
    const names = selector.filtered.value.map((m) => m.name);
    expect(names).toEqual(['所有人', 'Nova', 'Luna']);
    expect(selector.filtered.value[0].id).toBe(MENTION_ALL_ID);
  });

  it('filters members by case-insensitive substring', () => {
    selectGroup('g1');
    const input = ref('');
    const selector = useMentionSelector(input);

    input.value = '@nov';
    selector.updateCursor(4);

    expect(selector.filtered.value.map((m) => m.name)).toEqual(['Nova']);
  });

  it('does not inject @所有人 outside naturerandom', () => {
    selectGroup('g2');
    const input = ref('');
    const selector = useMentionSelector(input);

    input.value = '@';
    selector.updateCursor(1);

    expect(selector.groupMode.value).toBe('invite_only');
    expect(selector.filtered.value.map((m) => m.id)).toEqual(['a1']);
  });

  it('stays closed for non-group conversations and after dismiss', () => {
    const sessionStore = useChatSessionStore();
    sessionStore.currentSelectedItem = { id: 'a1', type: 'agent', name: 'Nova' };
    const input = ref('@');
    const selector = useMentionSelector(input);
    selector.updateCursor(1);
    expect(selector.isOpen.value).toBe(false);

    selectGroup('g1');
    expect(selector.isOpen.value).toBe(true);
    selector.dismiss();
    expect(selector.isOpen.value).toBe(false);
    // 输入再次变化后恢复可用
    input.value = '@N';
    selector.updateCursor(2);
    expect(selector.isOpen.value).toBe(true);
  });
});
