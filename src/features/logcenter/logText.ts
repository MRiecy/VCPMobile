/**
 * logText.ts — 日志中心纯函数层（无框架依赖，便于 L4 单测）。
 *
 * 上游契约见 plan/vcpmobile-more-tools-research/01：
 * - 后端按字节 offset 切片，半行拼接（trailingFragment）逻辑移植自
 *   VCPToolBox AdminPanel-Vue 的 useServerLogViewer.ts（修复了 VCPChat
 *   桌面版跨轮询断行缺陷）；
 * - 级别判定融合 log.js 的正则覆盖度（含 FATAL/WARNING）。
 */

/** 剥除 ANSI 转义序列（日志可能含彩色控制码，防止污染布局）。 */
export function stripAnsi(text: string): string {
  // eslint-disable-next-line no-control-regex
  return text.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, '');
}

export interface LogChunk {
  /** 可展示的行（含末尾半行——半行始终可见，下次增量原位补全）。 */
  lines: string[];
  /** 末尾不完整的半行（无换行符结尾时为非空）。 */
  trailing: string;
}

/**
 * 把一段日志文本切分为行，分离末尾半行。
 * `carry` 为上一次切分的 trailing（半行拼接核心）。
 */
export function splitLogChunk(content: string, carry = ''): LogChunk {
  const normalized = content.replace(/\r\n/g, '\n');
  const combined = `${carry}${normalized}`;
  const segments = combined.split('\n');
  const endsWithNewline = combined.endsWith('\n');

  if (endsWithNewline && segments[segments.length - 1] === '') {
    segments.pop();
  }

  const trailing = endsWithNewline ? '' : (segments.pop() ?? '');
  const lines = trailing ? [...segments, trailing] : segments;
  return { lines, trailing };
}

export type LogLevel = 'error' | 'warn' | 'info' | 'debug' | 'normal';

const LEVEL_PATTERN = /\[(LOG|INFO|WARN|WARNING|ERROR|FATAL|DEBUG)\]/i;

/** 判定日志行级别（用于 2px accent bar 着色）。 */
export function levelOf(line: string): LogLevel {
  const match = LEVEL_PATTERN.exec(line);
  if (!match) return 'normal';
  const tag = match[1].toUpperCase();
  if (tag === 'ERROR' || tag === 'FATAL') return 'error';
  if (tag === 'WARN' || tag === 'WARNING') return 'warn';
  if (tag === 'LOG' || tag === 'INFO') return 'info';
  return 'debug';
}

/** 行数限制合法区间（移动端默认 500，见 05 篇决策 2）。 */
export const LINE_LIMIT_MIN = 50;
export const LINE_LIMIT_MAX = 5000;
export const LINE_LIMIT_DEFAULT = 500;

export function clampLineLimit(raw: number): number {
  if (!Number.isFinite(raw)) return LINE_LIMIT_DEFAULT;
  return Math.min(LINE_LIMIT_MAX, Math.max(LINE_LIMIT_MIN, Math.trunc(raw)));
}

/**
 * 按关键词把一行文本切分为「普通段/命中段」交替数组，
 * 供模板以文本节点 + <mark> 渲染（规避 v-html，天然免疫 XSS）。
 */
export function splitByKeyword(
  line: string,
  keyword: string,
): { text: string; hit: boolean }[] {
  const needle = keyword.trim().toLowerCase();
  if (!needle) return [{ text: line, hit: false }];

  const lower = line.toLowerCase();
  const parts: { text: string; hit: boolean }[] = [];
  let cursor = 0;
  for (;;) {
    const found = lower.indexOf(needle, cursor);
    if (found === -1) break;
    if (found > cursor) parts.push({ text: line.slice(cursor, found), hit: false });
    parts.push({ text: line.slice(found, found + needle.length), hit: true });
    cursor = found + needle.length;
  }
  if (cursor < line.length) parts.push({ text: line.slice(cursor), hit: false });
  return parts.length ? parts : [{ text: line, hit: false }];
}
