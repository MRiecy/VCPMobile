/**
 * @提及 解析工具：与后端发言策略（group_speaking_policy.rs）同款的
 * 「`@名字` 不区分大小写子串匹配」规则，供 invite_only 模式发送后自动邀约。
 */

export interface MentionableMember {
  id: string;
  name: string;
}

/**
 * 解析消息文本中被 @ 的成员 id。
 *
 * - 命中规则：`@` + 成员名（小写子串匹配），与策略层一致；
 * - 顺序：按名字在文中首次出现的位置从前到后（发言接力顺序）；
 * - 去重：同一成员多次提及只邀约一次；
 * - 空名字成员不参与匹配。
 */
export function extractMentionedMemberIds(
  content: string,
  members: MentionableMember[],
): string[] {
  const lower = content.toLowerCase();
  const seen = new Set<string>();
  return members
    .filter((m) => m.name.trim().length > 0)
    .map((m) => ({ id: m.id, index: lower.indexOf(`@${m.name.toLowerCase()}`) }))
    .filter((hit) => hit.index !== -1)
    .sort((a, b) => a.index - b.index)
    .filter((hit) => {
      if (seen.has(hit.id)) return false;
      seen.add(hit.id);
      return true;
    })
    .map((hit) => hit.id);
}
