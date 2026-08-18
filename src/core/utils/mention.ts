/**
 * @提及 解析工具：与后端发言策略（group_speaking_policy.rs）同款的
 * 「`@名字` 不区分大小写紧邻匹配」规则，供 invite_only 模式发送后自动邀约、
 * 以及输入框 @提及 高亮（标签化）使用。
 *
 * 兼容性说明：策略层只识别半角 `@`；前端额外容忍中文输入法产生的全角 `＠`
 * （U+FF20），属于纯前端的宽松超集——邀约由前端显式驱动，不改变后端语义。
 */

export interface MentionableMember {
  id: string;
  name: string;
}

export interface MentionSegment {
  text: string;
  mention: boolean;
}

interface MentionHit {
  /** 触发符（@ 或 ＠）的下标 */
  start: number;
  /** 名字之后的下标（不含尾随空白） */
  end: number;
  /** 命中的原始大小写名字 */
  name: string;
}

/**
 * 扫描文本中的 @提及 命中区间。纯函数。
 *
 * - 触发符：`@` 或全角 `＠`；
 * - 名字必须紧邻触发符之后（与策略层 `@name` 子串语义等价）；
 * - 同一位置多个名字前缀重叠时取最长者（如 "Ab" 与 "Abc"）；
 * - 命中区间互不重叠，命中后从区间末尾继续扫描；
 * - 返回按出现位置升序。
 */
export function scanMentionHits(content: string, names: string[]): MentionHit[] {
  if (!content) return [];
  const candidates = names
    .map((n) => n.trim())
    .filter((n) => n.length > 0)
    .map((n) => ({ name: n, lower: n.toLowerCase() }))
    .sort((a, b) => b.lower.length - a.lower.length);
  if (candidates.length === 0) return [];

  const lower = content.toLowerCase();
  const hits: MentionHit[] = [];
  let i = 0;
  while (i < content.length) {
    const ch = content[i];
    if (ch !== "@" && ch !== "＠") {
      i++;
      continue;
    }
    const rest = lower.slice(i + 1);
    const hit = candidates.find((c) => rest.startsWith(c.lower));
    if (hit) {
      hits.push({ start: i, end: i + 1 + hit.name.length, name: hit.name });
      i += 1 + hit.name.length;
    } else {
      i++;
    }
  }
  return hits;
}

/**
 * 解析消息文本中被 @ 的成员 id。
 *
 * - 命中规则：`@`/`＠` + 成员名（不区分大小写紧邻匹配），策略层语义的宽松超集；
 * - 顺序：按名字在文中首次出现的位置从前到后（发言接力顺序）；
 * - 去重：同一成员多次提及只邀约一次；重名成员取先定义者；
 * - 空名字成员不参与匹配。
 */
export function extractMentionedMemberIds(
  content: string,
  members: MentionableMember[],
): string[] {
  const idByLowerName = new Map<string, string>();
  for (const m of members) {
    const key = m.name.trim().toLowerCase();
    if (key && !idByLowerName.has(key)) idByLowerName.set(key, m.id);
  }
  const hits = scanMentionHits(content, [...idByLowerName.keys()]);
  const seen = new Set<string>();
  const ids: string[] = [];
  for (const hit of hits) {
    const id = idByLowerName.get(hit.name.toLowerCase());
    if (id && !seen.has(id)) {
      seen.add(id);
      ids.push(id);
    }
  }
  return ids;
}

/**
 * 将文本切分为「提及 / 普通」分段，供输入框高亮背板渲染。
 *
 * 分段拼接后与原文完全一致（不改变任何字符），提及段可用于橙色系
 * 标签化样式；无命中时返回单个普通段。
 */
export function splitMentionSegments(
  content: string,
  names: string[],
): MentionSegment[] {
  if (!content) return [];
  const hits = scanMentionHits(content, names);
  if (hits.length === 0) return [{ text: content, mention: false }];

  const segments: MentionSegment[] = [];
  let cursor = 0;
  for (const hit of hits) {
    if (hit.start > cursor) {
      segments.push({ text: content.slice(cursor, hit.start), mention: false });
    }
    segments.push({ text: content.slice(hit.start, hit.end), mention: true });
    cursor = hit.end;
  }
  if (cursor < content.length) {
    segments.push({ text: content.slice(cursor), mention: false });
  }
  return segments;
}
