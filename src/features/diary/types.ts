export interface DiaryNoteKey {
  folder: string;
  file: string;
}

export interface DiaryFolderList {
  folders: string[];
}

export interface DiaryNoteSummary extends DiaryNoteKey {
  lastModified: string;
  preview: string;
}

export interface DiaryDocument {
  key: DiaryNoteKey;
  content: string;
  contentHash: string;
}

export interface DiarySearchResponse {
  notes: DiaryNoteSummary[];
  total: number;
  limited: boolean;
}

export interface DiarySemanticHit {
  key: DiaryNoteKey;
  preview: string;
  score?: number | null;
}

export interface DiarySemanticResponse {
  hits: DiarySemanticHit[];
  indexMayBeCatchingUp: boolean;
}

export interface DiarySaveOutcome {
  contentHash: string;
  verified: boolean;
}

export interface DiaryRenameOutcome {
  key: DiaryNoteKey;
  contentHash: string;
  status: "renamed" | "copied_source_retained";
}

export interface DiaryCreateRequest {
  maid: string;
  date: string;
  folder?: string;
  fileNameSuffix?: string;
  tag?: string;
  content: string;
}

export interface DiaryCreateOutcome {
  key: DiaryNoteKey;
  indexStatus: "queued";
}

export interface DiaryBatchError {
  key: DiaryNoteKey;
  message: string;
}

export interface DiaryBatchOutcome {
  succeeded: DiaryNoteKey[];
  errors: DiaryBatchError[];
}

export interface DiaryComposerDraft {
  maid: string;
  date: string;
  folder: string;
  fileNameSuffix: string;
  tag: string;
  content: string;
}

export type DiaryFolderCategory = "diary" | "cluster";

export type DiaryScreen = "list" | "reader" | "editor" | "preview" | "composer";
export type DiarySearchMode = "none" | "text" | "semantic";
export type DiarySearchScope = "folder" | "all";
export type DiarySaveState =
  | "idle"
  | "dirty"
  | "saving"
  | "saved"
  | "error"
  | "conflict"
  | "uncertain";

export interface DiaryUiError {
  code: string;
  message: string;
}

export interface DiaryFilePresentation {
  title: string;
  originalFile: string;
  date?: string;
  time?: string;
  extension?: string;
  structured: boolean;
}

const ERROR_PREFIX = /^(DIARY_[A-Z_]+):\s*(.*)$/s;
const STRUCTURED_FILE = /^(\d{4}-\d{2}-\d{2})-(\d{2})_(\d{2})_(\d{2})(?:-(.+?))?\.(txt|md)$/i;

export function parseDiaryError(error: unknown): DiaryUiError {
  const raw = error instanceof Error ? error.message : String(error ?? "");
  const match = raw.match(ERROR_PREFIX);
  if (match) {
    return {
      code: match[1],
      message: match[2] || "日记操作失败",
    };
  }
  return {
    code: "DIARY_UNKNOWN",
    message: raw || "日记操作失败",
  };
}

export function noteKeyId(key: DiaryNoteKey): string {
  return `${key.folder}\u0000${key.file}`;
}

export function sameNoteKey(left: DiaryNoteKey | null, right: DiaryNoteKey | null): boolean {
  return Boolean(left && right && left.folder === right.folder && left.file === right.file);
}

export function parseDiaryFileName(file: string): DiaryFilePresentation {
  const match = file.match(STRUCTURED_FILE);
  if (!match) {
    const extension = file.includes(".") ? file.split(".").pop()?.toUpperCase() : undefined;
    return {
      title: file,
      originalFile: file,
      extension,
      structured: false,
    };
  }

  const [, date, hour, minute, second, rawTitle, extension] = match;
  return {
    title: rawTitle?.trim() || "DailyNote",
    originalFile: file,
    date,
    time: `${hour}:${minute}:${second}`,
    extension: extension.toUpperCase(),
    structured: true,
  };
}

export function formatDiaryTimestamp(value: string): string {
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) return value || "时间未知";
  return new Intl.DateTimeFormat("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  }).format(timestamp);
}

export function semanticHitToSummary(hit: DiarySemanticHit): DiaryNoteSummary {
  return {
    folder: hit.key.folder,
    file: hit.key.file,
    preview: hit.preview,
    lastModified: "",
  };
}

export function diaryFolderCategory(folder: string): DiaryFolderCategory {
  return folder.endsWith("簇") ? "cluster" : "diary";
}

export function isValidDiaryFileName(file: string): boolean {
  const value = file.trim();
  return Boolean(
    value &&
      value !== "." &&
      value !== ".." &&
      !value.includes("/") &&
      !value.includes("\\") &&
      !/[\u0000-\u001f\u007f]/.test(value),
  );
}
