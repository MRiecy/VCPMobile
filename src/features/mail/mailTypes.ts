/**
 * mailTypes.ts — clawEmail 邮箱类型与纯函数层。
 *
 * 上游契约（plan/vcpmobile-more-tools-research/10 篇）：
 * - 响应包裹 `{status:'success', ...}`；from/to 形态不稳定（string/array/object）；
 * - date 未做时区/格式归一；read/unread 可能双双 undefined（状态未知）；
 * - 寻址：mailbox=mailN（子邮箱优先）或 user=完整地址（公共邮箱）。
 */
import { renderSafeMarkdown } from '../../core/utils/safeMarkdown';

// ---------- 邮箱账户 ----------

export interface MailboxInfo {
  /** 槽位：'public' 或 'mail1'..'mail4'。 */
  mailbox: string;
  /** 完整邮箱地址。 */
  user: string;
  label: string;
  /** 子邮箱绑定的 Agent 名（公共邮箱为 null）。 */
  agentName: string | null;
  enabled: boolean;
  cachedCount: number;
}

export interface WsState {
  user: string;
  connected: boolean;
  lastError: string | null;
}

export function normalizeMailboxes(raw: unknown): MailboxInfo[] {
  if (!Array.isArray(raw)) return [];
  return raw
    .map((item) => {
      if (!item || typeof item !== 'object') return null;
      const record = item as Record<string, unknown>;
      const user = String(record.user ?? '');
      const mailbox = String(record.mailbox ?? '');
      if (!user || !mailbox) return null;
      return {
        mailbox,
        user,
        label: String(record.label ?? user),
        agentName: typeof record.agentName === 'string' ? record.agentName : null,
        enabled: record.enabled !== false,
        cachedCount: Number(record.cachedCount) || 0,
      } as MailboxInfo;
    })
    .filter((entry): entry is MailboxInfo => entry !== null);
}

export function normalizeWsStates(raw: unknown): WsState[] {
  if (!Array.isArray(raw)) return [];
  return raw
    .map((item) => {
      if (!item || typeof item !== 'object') return null;
      const record = item as Record<string, unknown>;
      const user = String(record.user ?? '');
      if (!user) return null;
      return {
        user,
        connected: record.connected === true,
        lastError: typeof record.lastError === 'string' ? record.lastError : null,
      } as WsState;
    })
    .filter((entry): entry is WsState => entry !== null);
}

/** 邮箱的寻址参数：子邮箱传 mailbox 槽位（优先），公共邮箱传 user 地址。 */
export function addressingOf(
  info: MailboxInfo,
): { mailbox?: string; user?: string } {
  if (info.mailbox.startsWith('mail')) return { mailbox: info.mailbox };
  return { user: info.user };
}

// ---------- 邮件摘要 ----------

export type ReadState = 'read' | 'unread' | 'unknown';

export interface MailSummary {
  mailId: string;
  user: string;
  subject: string;
  fromText: string;
  toText: string;
  dateMs: number;
  readState: ReadState;
  hasAttachments: boolean;
  preview: string;
}

/** from/to 形态不稳定：string | string[] | object → 展示字符串。 */
export function mailPartyText(raw: unknown): string {
  if (raw == null) return '';
  if (typeof raw === 'string') return raw;
  if (Array.isArray(raw)) {
    return raw.map(mailPartyText).filter(Boolean).join(', ');
  }
  if (typeof raw === 'object') {
    const record = raw as Record<string, unknown>;
    if (typeof record.name === 'string' && typeof record.address === 'string') {
      return `${record.name} <${record.address}>`;
    }
    if (typeof record.address === 'string') return record.address;
    if (typeof record.name === 'string') return record.name;
    try {
      return JSON.stringify(raw);
    } catch {
      return String(raw);
    }
  }
  return String(raw);
}

/** date 宽容解析：ISO / RFC2822 / 时间戳数字串。 */
export function parseMailDate(raw: unknown): number {
  if (typeof raw === 'number' && Number.isFinite(raw)) return raw;
  if (typeof raw !== 'string' || !raw) return 0;
  const direct = Date.parse(raw);
  if (Number.isFinite(direct)) return direct;
  const asNumber = Number(raw);
  return Number.isFinite(asNumber) && asNumber > 1_000_000_000_000 ? asNumber : 0;
}

export function normalizeMailSummary(raw: unknown): MailSummary | null {
  if (!raw || typeof raw !== 'object') return null;
  const record = raw as Record<string, unknown>;
  const mailId = String(record.mailId ?? record.id ?? '');
  if (!mailId) return null;

  const read = typeof record.read === 'boolean' ? record.read : undefined;
  const unread = typeof record.unread === 'boolean' ? record.unread : undefined;
  const readState: ReadState =
    read !== undefined ? (read ? 'read' : 'unread')
    : unread !== undefined ? (unread ? 'unread' : 'read')
    : 'unknown';

  return {
    mailId,
    user: String(record.user ?? ''),
    subject: String(record.subject ?? '(无主题)'),
    fromText: mailPartyText(record.from),
    toText: mailPartyText(record.to),
    dateMs: parseMailDate(record.date),
    readState,
    hasAttachments: record.hasAttachments === true,
    preview: String(record.preview ?? ''),
  };
}

export function normalizeMailList(raw: unknown): MailSummary[] {
  if (!Array.isArray(raw)) return [];
  return raw
    .map(normalizeMailSummary)
    .filter((mail): mail is MailSummary => mail !== null);
}

// ---------- 详情 ----------

/** 从详情响应提取可渲染 Markdown（响应的 markdown 字段即正文+附件解析文本）。 */
export function extractDetailMarkdown(raw: unknown): string {
  if (!raw || typeof raw !== 'object') return '';
  const record = raw as Record<string, unknown>;
  return typeof record.markdown === 'string' ? record.markdown : '';
}

// ---------- 文件夹（V1.1） ----------

export interface FolderInfo {
  id: string;
  name: string;
  unreadCount: number | null;
}

export function normalizeFolders(raw: unknown): FolderInfo[] {
  if (!Array.isArray(raw)) return [];
  return raw
    .map((item) => {
      if (!item || typeof item !== 'object') return null;
      const record = item as Record<string, unknown>;
      const id = String(record.id ?? record.fid ?? '');
      if (!id) return null;
      const unread = Number(record.unreadCount);
      return {
        id,
        name: String(record.name ?? id),
        unreadCount: Number.isFinite(unread) ? unread : null,
      } as FolderInfo;
    })
    .filter((entry): entry is FolderInfo => entry !== null);
}

// ---------- 附件元数据（V1.1） ----------

export interface AttachmentMeta {
  /** 下载优先使用 partId，回退 attachmentId。 */
  partId: string;
  filename: string;
  contentType: string;
  size: number | null;
  /** cid 非空 = HTML 内嵌图。 */
  inline: boolean;
}

export function normalizeAttachments(raw: unknown): AttachmentMeta[] {
  if (!Array.isArray(raw)) return [];
  return raw
    .map((item) => {
      if (!item || typeof item !== 'object') return null;
      const record = item as Record<string, unknown>;
      const partId = String(
        record.partId ?? record.attachmentId ?? record.id ?? record.cid ?? '',
      );
      if (!partId) return null;
      const size = Number(record.size);
      return {
        partId,
        filename: String(record.filename ?? record.name ?? `${partId}.bin`),
        contentType: String(record.contentType ?? 'application/octet-stream'),
        size: Number.isFinite(size) ? size : null,
        inline: !!record.cid,
      } as AttachmentMeta;
    })
    .filter((entry): entry is AttachmentMeta => entry !== null);
}

/** 详情归一化（V1.1：markdown + 附件元数据；V1.2：html/text 结构化正文）。 */
export interface MailDetail {
  markdown: string;
  /** 原始 HTML 正文（补丁服务器的结构化字段；无 HTML 邮件时为 null）。 */
  html: string | null;
  /** 纯文本正文（同上）。 */
  text: string | null;
  attachments: AttachmentMeta[];
}

export function normalizeDetail(raw: unknown): MailDetail {
  const record = raw && typeof raw === 'object' ? (raw as Record<string, unknown>) : {};
  return {
    markdown: extractDetailMarkdown(record),
    html: typeof record.html === 'string' && record.html.trim() ? record.html : null,
    text: typeof record.text === 'string' && record.text.trim() ? record.text : null,
    attachments: normalizeAttachments(record.attachments),
  };
}

/**
 * 详情正文提取（唯一入口）。
 *
 * 优先级：
 * 1. `html` —— 补丁服务器的结构化 HTML 正文，保真渲染（经 filterTrustedRichHtml）；
 * 2. `text` —— 纯文本正文，按预格式化文本展示；
 * 3. AI 导向的 markdown 大杂烩（旧服务器兜底）——剥离「基本信息/正文预览/
 *    附件/可执行后续动作」等 AI 段落，只保留正文区。
 */
export type MailBody =
  | { kind: 'html'; html: string }
  | { kind: 'text'; text: string }
  | { kind: 'markdown'; markdown: string };

/** 从 AI 导向 markdown 中提取正文段落（旧服务器兼容兜底）。 */
function extractBodyFromAiMarkdown(markdown: string): string {
  // 「## HTML 正文转 Markdown」或「## 纯文本正文」段落到下一个二级标题之间
  const section = markdown.match(
    /##\s*(?:HTML 正文转 Markdown|纯文本正文)\s*\n([\s\S]*?)(?=\n##\s|$)/,
  );
  if (section?.[1]?.trim()) return section[1].trim();
  // 再兜底「正文预览」段落
  const preview = markdown.match(/##\s*正文预览\s*\n([\s\S]*?)(?=\n##\s|$)/);
  if (preview?.[1]?.trim()) return preview[1].trim();
  return markdown;
}

export function mailBodyOf(detail: MailDetail): MailBody | null {
  if (detail.html) return { kind: 'html', html: detail.html };
  if (detail.text) return { kind: 'text', text: detail.text };
  if (detail.markdown) {
    return { kind: 'markdown', markdown: extractBodyFromAiMarkdown(detail.markdown) };
  }
  return null;
}

// ---------- 文件夹语义化 ----------

/**
 * 已知系统文件夹 → 中文名。clawEmail 云端（WuKongIM）文件夹名为英文约定，
 * 原样展示对中文用户不友好；未知自定义文件夹保留原名。
 */
const KNOWN_FOLDER_NAMES: Record<string, string> = {
  inbox: '收件箱',
  sent: '已发送',
  'sent items': '已发送',
  'sent messages': '已发送',
  drafts: '草稿箱',
  draft: '草稿箱',
  trash: '垃圾箱',
  deleted: '垃圾箱',
  'deleted items': '垃圾箱',
  junk: '垃圾邮件',
  spam: '垃圾邮件',
  archive: '归档',
};

export function folderDisplayName(name: string): string {
  return KNOWN_FOLDER_NAMES[name.trim().toLowerCase()] ?? name;
}

/** 是否收件箱（客户端已有固定的「收件箱」入口，服务器列表里的要去重）。 */
export function isInboxFolder(name: string): boolean {
  return KNOWN_FOLDER_NAMES[name.trim().toLowerCase()] === '收件箱';
}

/** 系统文件夹的规范排序权重（未知名排最后，按名称字典序）。 */
function folderSortWeight(name: string): number {
  const key = name.trim().toLowerCase();
  const order = ['sent', 'sent items', 'sent messages', 'drafts', 'draft', 'archive', 'junk', 'spam', 'trash', 'deleted', 'deleted items'];
  const index = order.indexOf(key);
  return index === -1 ? 100 : index;
}

/** 文件夹列表整理：去收件箱（已有固定入口）+ 系统文件夹按用途排序。 */
export function organizeFolders(folders: FolderInfo[]): FolderInfo[] {
  return folders
    .filter((folder) => !isInboxFolder(folder.name))
    .sort((a, b) => {
      const diff = folderSortWeight(a.name) - folderSortWeight(b.name);
      return diff !== 0 ? diff : a.name.localeCompare(b.name);
    });
}

/** 附件大小展示。 */
export function attachmentSizeLabel(size: number | null): string {
  if (size === null || !Number.isFinite(size)) return '—';
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}

/** 邮件正文渲染唯一 v-html 边界（共享安全管线）。 */
export function renderMailMarkdown(content: string): string {
  return renderSafeMarkdown(content);
}

// ---------- 展示工具 ----------

/** 邮件列表时间：今天显示时分，今年显示月日，更早显示年月日。 */
export function mailTimeLabel(timeMs: number): string {
  if (!timeMs) return '—';
  const date = new Date(timeMs);
  const now = new Date();
  const pad = (n: number) => String(n).padStart(2, '0');
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  if (timeMs >= startOfToday) return `${pad(date.getHours())}:${pad(date.getMinutes())}`;
  if (date.getFullYear() === now.getFullYear()) return `${date.getMonth() + 1}月${date.getDate()}日`;
  return `${date.getFullYear()}年${date.getMonth() + 1}月${date.getDate()}日`;
}
