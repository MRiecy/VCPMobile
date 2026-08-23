/**
 * 全局消息搜索（Global Search）共享类型定义。
 * 与 src-tauri/src/vcp_modules/persistence/db_manager.rs 的 DTO 保持契约一致。
 */

/** 单条搜索结果（对应 Rust FtsSearchResult，camelCase 序列化） */
export interface FtsSearchResultItem {
  msgId: string;
  topicId: string;
  role: string;
  timestamp: number;
  topicTitle: string;
  ownerId: string;
  ownerType: 'agent' | 'group';
  /** FTS5 snippet() 命中摘要，含 <mark> 高亮标记（渲染时必须按标记切分，禁止直接 v-html） */
  snippet: string;
}

/** FTS 索引覆盖率状态（对应 Rust FtsIndexStatus） */
export interface FtsIndexStatus {
  totalMessages: number;
  indexedMessages: number;
  rebuilding: boolean;
}

/** 搜索范围：全部 / 当前话题 / 指定助手或群组 */
export type SearchScope = 'all' | 'topic' | 'owner';

/** 消息协议类型过滤，不表示具体 Agent 身份 */
export type RoleFilter = 'all' | 'user' | 'assistant' | 'system';

/** 时间范围过滤 */
export type TimeFilter = 'all' | 'today' | 'week' | 'month';

/** 排序：时间倒序（默认）/ bm25 相关度 */
export type SortMode = 'time' | 'rank';

export const SEARCH_PAGE_SIZE = 50;

/** 最小触发字符数（trigram 对 <3 字查询退化全扫，双字词合法故下限取 2） */
export const SEARCH_MIN_CHARS = 2;

export const ROLE_LABELS: Record<RoleFilter, string> = {
  all: '全部类型',
  user: '用户消息',
  assistant: 'AI 回复',
  system: '系统消息',
};

export const TIME_LABELS: Record<TimeFilter, string> = {
  all: '全部时间',
  today: '今天',
  week: '近 7 天',
  month: '近 30 天',
};
