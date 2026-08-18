import { computed, ref, watch, type Ref } from 'vue';
import { useAssistantStore } from '../../../core/stores/assistant';
import { useChatSessionStore } from '../../../core/stores/chatSessionStore';

export interface MentionMember {
  id: string;
  name: string;
}

export interface MentionTarget {
  /** '@' 在文本中的下标 */
  start: number;
  /** 光标位置（token 终点） */
  end: number;
  /** @ 之后、光标之前的查询串 */
  query: string;
}

/** 合成项「@所有人」的保留 id（仅 naturerandom 模式注入） */
export const MENTION_ALL_ID = '__mention_all__';

/**
 * 从文本与光标位置提取进行中的 @提及 token。纯函数，便于测试。
 *
 * 规则：
 * - 触发符为半角 `@` 或中文输入法全角 `＠`（U+FF20）；
 * - 触发符必须位于行首或前置为空白（防止 user@host 邮箱误触发）；
 * - 触发符到光标之间不含空白（含空白视为提及已结束）。
 */
export function extractMentionQuery(text: string, cursor: number): MentionTarget | null {
  if (cursor < 0 || cursor > text.length) return null;
  const before = text.slice(0, cursor);
  const atIndex = Math.max(before.lastIndexOf('@'), before.lastIndexOf('＠'));
  if (atIndex === -1) return null;
  if (atIndex > 0 && !/\s/.test(before[atIndex - 1])) return null;
  const token = before.slice(atIndex + 1);
  if (/\s/.test(token)) return null;
  return { start: atIndex, end: cursor, query: token };
}

/**
 * 群聊 @提及选择器状态。
 *
 * - 仅群聊会话激活；
 * - naturerandom 模式注入合成首项「@所有人」；
 * - 显式关闭（dismiss）后保持关闭，直到输入内容再次变化。
 */
export function useMentionSelector(input: Ref<string>) {
  const assistantStore = useAssistantStore();
  const sessionStore = useChatSessionStore();

  const cursor = ref(0);
  const dismissed = ref(false);

  // sync flush：输入变化当拍即解除关闭锁定，保证选择器状态确定性
  watch(
    input,
    () => {
      dismissed.value = false;
    },
    { flush: 'sync' },
  );

  const currentGroup = computed(() => {
    const item = sessionStore.currentSelectedItem;
    if (!item || item.type !== 'group') return null;
    return assistantStore.groups.find((g) => g.id === item.id) ?? null;
  });

  const groupMode = computed(() => currentGroup.value?.mode ?? null);

  const members = computed<MentionMember[]>(() => {
    const group = currentGroup.value;
    if (!group) return [];
    return group.members
      .map((id) => assistantStore.agents.find((a) => a.id === id))
      .filter((a): a is NonNullable<typeof a> => !!a)
      .map((a) => ({ id: a.id, name: a.name }));
  });

  const target = computed<MentionTarget | null>(() => {
    if (!currentGroup.value) return null;
    return extractMentionQuery(input.value, cursor.value);
  });

  const filtered = computed<MentionMember[]>(() => {
    const query = target.value?.query.toLowerCase() ?? '';
    const matched = members.value.filter(
      (m) => !query || m.name.toLowerCase().includes(query),
    );
    if (
      groupMode.value === 'naturerandom' &&
      (!query || '所有人'.includes(query))
    ) {
      return [{ id: MENTION_ALL_ID, name: '所有人' }, ...matched];
    }
    return matched;
  });

  const isOpen = computed(
    () => target.value !== null && !dismissed.value && filtered.value.length > 0,
  );

  const updateCursor = (pos: number) => {
    cursor.value = pos;
  };

  const dismiss = () => {
    dismissed.value = true;
  };

  return {
    isOpen,
    target,
    filtered,
    members,
    groupMode,
    updateCursor,
    dismiss,
  };
}
