/**
 * forumTypes.ts — VCP 论坛类型与纯函数层。
 *
 * 上游契约（plan/vcpmobile-more-tools-research/09 篇）：
 * - 一帖一 Markdown 文件；GET /posts 返回 PostMeta[]（无分页）；
 * - timestamp 是本地时区 ISO 变体（冒号被替换成 `-`）；
 * - 帖子正文 = 元信息头 + 主帖 + `## 评论区` 硬分隔 + `### 楼层 #N` 楼层；
 * - 置顶靠标题手写 `[置顶]` 约定；无作者权限模型。
 */
import { renderSafeMarkdown } from '../../core/utils/safeMarkdown';

// ---------- PostMeta（列表项） ----------

export interface PostMeta {
  uid: string;
  board: string;
  title: string;
  author: string;
  /** 归一化后的发布时间（epoch ms；解析失败为 0）。 */
  timestampMs: number;
  lastReplyBy: string | null;
  lastReplyAt: string | null;
  /** 楼层总数（上游补丁字段；旧服务器/轻量模式为 null）。 */
  replyCount: number | null;
  mtimeMs: number;
  pinned: boolean;
}

/**
 * 论坛时间戳归一化：`2026-03-21T00-43-00.160`（冒号→`-`）→ epoch ms。
 * 同时容错标准 ISO 与带 Z 的形式。解析失败返回 0。
 */
export function parseForumTime(raw: unknown): number {
  if (typeof raw !== 'string' || !raw) return 0;
  const direct = Date.parse(raw);
  if (Number.isFinite(direct)) return direct;
  // 冒号替换变体：T 后的 hh-mm-ss 还原为 hh:mm:ss
  const restored = raw.replace(/T(\d{2})-(\d{2})-(\d{2})/, 'T$1:$2:$3');
  const parsed = Date.parse(restored);
  return Number.isFinite(parsed) ? parsed : 0;
}

export function isPinnedTitle(title: string): boolean {
  return title.includes('[置顶]');
}

export function normalizePostMeta(raw: unknown): PostMeta | null {
  if (!raw || typeof raw !== 'object') return null;
  const record = raw as Record<string, unknown>;
  const uid = String(record.uid ?? '');
  if (!uid) return null;
  const title = String(record.title ?? '未命名帖子');
  return {
    uid,
    board: String(record.board ?? '未分类'),
    title,
    author: String(record.author ?? '未知'),
    timestampMs: parseForumTime(record.timestamp),
    lastReplyBy: typeof record.lastReplyBy === 'string' ? record.lastReplyBy : null,
    lastReplyAt: typeof record.lastReplyAt === 'string' ? record.lastReplyAt : null,
    replyCount: typeof record.replyCount === 'number' && Number.isFinite(record.replyCount)
      ? record.replyCount
      : null,
    mtimeMs: Number(record.mtimeMs) || 0,
    pinned: isPinnedTitle(title),
  };
}

/** 列表归一化 + 排序：置顶优先，其后按 mtimeMs 降序。 */
export function normalizePostList(raw: unknown): PostMeta[] {
  const list = Array.isArray(raw) ? raw : [];
  return list
    .map(normalizePostMeta)
    .filter((post): post is PostMeta => post !== null)
    .sort((a, b) => Number(b.pinned) - Number(a.pinned) || b.mtimeMs - a.mtimeMs);
}

// ---------- 帖子正文解析 ----------

export interface PostFloor {
  index: number;
  author: string;
  timeMs: number;
  body: string;
}

export interface ParsedPost {
  /** 主帖正文（元信息头已剥离）。 */
  mainBody: string;
  floors: PostFloor[];
}

const COMMENT_SPLIT = /\n\s*-{3,}\s*\n\s*##\s*评论区\s*\n\s*-{3,}\s*\n/;
// 允许行首或字符串起始（COMMENT_SPLIT 会吞掉分隔后的前置换行）
const FLOOR_SPLIT = /(?:^|\n)\s*-{3,}\s*\n\s*###\s*楼层\s*#(\d+)\s*\n/;
const FLOOR_AUTHOR = /\*\*回复者:\*\*\s*(.+)/;
const FLOOR_TIME = /\*\*时间:\*\*\s*(.+)/;

/** 剥掉主帖的元信息头（# 标题 + **作者:** 等行 + 首个 --- 之前的内容）。 */
function stripMetaHeader(main: string): string {
  const separator = main.search(/\n\s*-{3,}\s*\n/);
  if (separator === -1) return main.trim();
  return main.slice(separator).replace(/^\n\s*-{3,}\s*\n/, '').trim();
}

/** 解析整篇帖子 Markdown 为主帖 + 楼层列表（对格式变体宽容，失败时主帖兜底全文）。 */
export function parsePostContent(content: string): ParsedPost {
  if (!content) return { mainBody: '', floors: [] };

  const parts = content.split(COMMENT_SPLIT);
  const mainBody = stripMetaHeader(parts[0]);
  if (parts.length < 2) return { mainBody, floors: [] };

  const commentArea = parts.slice(1).join('\n');
  const segments = commentArea.split(FLOOR_SPLIT);
  // split 结果：[前言, 楼层号1, 内容1, 楼层号2, 内容2, ...]
  const floors: PostFloor[] = [];
  for (let i = 1; i + 1 < segments.length; i += 2) {
    const index = Number(segments[i]);
    const raw = segments[i + 1] ?? '';
    const authorMatch = raw.match(FLOOR_AUTHOR);
    const timeMatch = raw.match(FLOOR_TIME);
    // 去掉楼层元信息行，保留正文
    const body = raw
      .replace(FLOOR_AUTHOR, '')
      .replace(FLOOR_TIME, '')
      .replace(/^\s*\n/, '')
      .trim();
    floors.push({
      index: Number.isFinite(index) ? index : floors.length + 1,
      author: authorMatch ? authorMatch[1].trim() : '未知',
      timeMs: timeMatch ? parseForumTime(timeMatch[1].trim()) : 0,
      body,
    });
  }
  return { mainBody, floors };
}

// ---------- 渲染 ----------

/**
 * 论坛正文渲染唯一 v-html 边界（共享安全管线：marked → filterTrustedRichHtml）。
 */
export function renderForumMarkdown(content: string): string {
  return renderSafeMarkdown(content);
}

// ---------- 展示工具 ----------

/** 相对时间：刚刚 / N分钟前 / N小时前 / 昨天 / M月d日 / yyyy年M月d日。 */
export function relativeTime(timeMs: number): string {
  if (!timeMs) return '—';
  const diff = Date.now() - timeMs;
  if (diff < 60_000) return '刚刚';
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;

  const date = new Date(timeMs);
  const now = new Date();
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  if (timeMs >= startOfToday - 86_400_000 && timeMs < startOfToday) return '昨天';
  if (date.getFullYear() === now.getFullYear()) {
    return `${date.getMonth() + 1}月${date.getDate()}日`;
  }
  return `${date.getFullYear()}年${date.getMonth() + 1}月${date.getDate()}日`;
}

/** 署名 → 稳定 HSL 色相（头像底色用；共享散列见 core/utils/nameHue）。 */
export { nameHue as authorHue } from '../../core/utils/nameHue';
