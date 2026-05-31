export interface MessageShell {
  avatarColor: string;
  bubbleBorderColor: string;
  bubbleBoxShadow: string;
  displayName: string;
  isUser: boolean;
}

export type MarkdownNode = {
  type: "paragraph" | "heading" | "code_block" | "blockquote" | "list" | "table" | "thematic_break" | "raw_html" | "mermaid";
  children?: InlineNode[];
  level?: number;
  lang?: string;
  code?: string;
  highlighted_html?: string;
  theme?: string;
  ordered?: boolean;
  items?: MarkdownNode[][];
  header?: InlineNode[][];
  rows?: InlineNode[][][];
  wrapper_class?: string;
  content?: string;
  encoded?: string;
  hash?: string | number;
};

export type InlineNode = {
  type: "text" | "strong" | "emphasis" | "strikethrough" | "code" | "link" | "image" | "line_break" | "soft_break" | "inline_math" | "quoted_text" | "highlight_tag" | "alert_tag" | "raw_html_inline";
  value?: string;
  children?: InlineNode[];
  href?: string;
  src?: string;
  alt?: string;
  title?: string;
  needs_asset_conversion?: boolean;
  content?: string;
  display_mode?: boolean;
  hash?: string | number;
};

export interface ContentBlock {
  type:
  | "markdown"
  | "tool-use"
  | "tool-result"
  | "diary"
  | "thought"
  | "button-click"
  | "html-preview"
  | "role-divider"
  | "style"
  | "math";
  content?: string;
  nodes?: MarkdownNode[]; // For type: "markdown", "diary", "thought"
  tool_name?: string;
  is_complete?: boolean;
  status?: string;
  details?: Array<{ key: string; value: string }>;
  footer?: string;
  maid?: string;
  date?: string;
  theme?: string;
  role?: string;
  is_end?: boolean;
  display_mode?: boolean;
  highlighted_content?: string;
  hash?: string | number;
}

/**
 * Attachment 接口定义，严格对齐 Rust 端的 AttachmentSyncDTO / Attachment (仅保留核心字段)
 */
export interface Attachment {
  id?: string; // 纯前端 UI 稳定性标识 (Stable Key)
  type: string;
  name: string;
  size: number;
  progress?: number; // 0-100 的真实上传进度
  src: string; // 物理存储路径：真理之源。用于后续超栈文件追踪，或跨端同步时的原始路径参考
  resolvedSrc?: string; // Webview 可用的 asset:// 路径 (运行时动态生成，不进行持久化)
  hash?: string;
  status?: string;
  internalPath?: string; // 手机本地物理路径，仅供前端通过 convertFileSrc 转换为安全 URL
  extractedText?: string;
  imageFrames?: string[];
  thumbnailPath?: string;
  createdAt?: number;
}

/**
 * ChatMessage 接口定义，严格对齐 Rust 端的 MessageSyncDTO / ChatMessage
 */
export interface ChatMessage {
  id: string;
  role: string;
  name?: string;
  content?: string; // 原文，现在变为按需懒加载的可选字段
  blocks?: ContentBlock[]; // 预编译的 AST 数据块，前端直接渲染
  shell?: MessageShell; // 预计算的外壳属性
  timestamp: number;

  isThinking?: boolean;
  agentId?: string;
  groupId?: string;
  isGroupMessage?: boolean;
  finishReason?: string;
  attachments?: Attachment[];
  topicId?: string;
  topic_id?: string; // 兼容两种写法

  // 以下为纯前端运行时 UI 状态 (Ephemeral)，绝不进行持久化
  tailContent?: string;      // Aurora: 尾随区 Markdown (高频变动)
  tailBlock?: ContentBlock;
}

/**
 * HistoryChunk 接口定义，用于 Channel 流式加载
 */
export interface HistoryChunk {
  message: ChatMessage;
  index: number;
  is_last: boolean;
}

/**
 * TopicDelta 接口定义
 */
export interface TopicDelta {
  added: ChatMessage[];
  updated: ChatMessage[];
  deleted_ids: string[];
  sync_skipped?: boolean;
}

/**
 * TopicFingerprint 接口定义
 */
export interface TopicFingerprint {
  topic_id: string;
  mtime: number;
  size: number;
  msg_count: number;
}

/**
 * 流式增量块定义，由 Rust 流式块解析器推送
 * 与 ContentBlock 类似但精简，用于流式期间的增量渲染
 */
export interface StreamBlock {
  type: "markdown" | "thought" | "tool-use" | "tool-result" | "diary" | "html-preview" | "role-divider" | "style" | "button-click";
  content?: string;
  nodes?: MarkdownNode[];
  theme?: string;
  is_complete?: boolean;
  tool_name?: string;
  status?: string;
  details?: Array<{ key: string; value: string }>;
  footer?: string;
  maid?: string;
  date?: string;
  role?: string;
  is_end?: boolean;
  highlighted_content?: string;
  hash?: string;
}

/**
 * Aurora 语义沉淀更新，由 Rust 流式管道推送
 */
export interface AuroraUpdate {
  stableBlocks?: StreamBlock[];
  stableChanged?: boolean;
  tailBlock?: StreamBlock;
  tail?: string;
  tailChanged?: boolean;
  content?: string;
}

/**
 * StreamEvent 接口定义，用于 Rust Channel 事件分发
 */
export interface StreamEvent {
  type: string;
  chunk?: any;
  messageId?: string;
  message_id?: string;
  context?: any;
  finishReason?: string;
  error?: string;
  aurora?: AuroraUpdate;
  blocks?: ContentBlock[];
}
