export interface MessageShell {
  avatarColor: string;
  displayName: string;
  isUser: boolean;
}

type AstHash = number;

export type MarkdownNode =
  | { type: "paragraph"; children: InlineNode[]; hash?: AstHash }
  | { type: "heading"; level: number; children: InlineNode[]; hash?: AstHash }
  | {
      type: "code_block";
      lang: string | null;
      code: string;
      highlighted_html: string | null;
      theme: string | null;
      hash?: AstHash;
    }
  | { type: "blockquote"; children: MarkdownNode[]; hash?: AstHash }
  | { type: "list"; ordered: boolean; items: MarkdownNode[][]; hash?: AstHash }
  | {
      type: "table";
      header: InlineNode[][];
      rows: InlineNode[][][];
      wrapper_class: string | null;
      hash?: AstHash;
    }
  | { type: "thematic_break" }
  | { type: "raw_html"; content: string; hash?: AstHash };

export type InlineNode =
  | { type: "text"; value: string }
  | { type: "strong"; children: InlineNode[]; hash?: AstHash }
  | { type: "emphasis"; children: InlineNode[]; hash?: AstHash }
  | { type: "strikethrough"; children: InlineNode[]; hash?: AstHash }
  | { type: "code"; value: string }
  | {
      type: "link";
      href: string;
      title: string | null;
      children: InlineNode[];
      needs_asset_conversion: boolean;
      hash?: AstHash;
    }
  | {
      type: "image";
      src: string;
      alt: string;
      title: string | null;
      needs_asset_conversion: boolean;
      hash?: AstHash;
    }
  | { type: "break" }
  | { type: "inline_math"; content: string; display_mode: boolean; hash?: AstHash }
  | {
      type: "vcp_custom";
      kind: string;
      value?: string;
      children?: InlineNode[];
      hash?: AstHash;
    }
  | { type: "raw_html_inline"; content: string; hash?: AstHash };

export interface ToolCallSummaryItem {
  tool_name: string;
  status: string;
}

export interface ToolResultDetail {
  key: string;
  value: string;
}

interface RenderBlockFields {
  content?: string;
  nodes?: MarkdownNode[];
  tool_name?: string;
  is_complete?: boolean;
  status?: string;
  details?: ToolResultDetail[];
  footer?: string;
  maid?: string;
  valet?: string;
  date?: string;
  file_name?: string;
  folder?: string;
  target?: string;
  replace?: string;
  target_nodes?: MarkdownNode[];
  replace_nodes?: MarkdownNode[];
  theme?: string;
  role?: string;
  is_end?: boolean;
  highlighted_content?: string;
  items?: ToolCallSummaryItem[];
  raw_content?: string;
  hash?: string | number;
}

/** 持久化块与流式稳定块的统一渲染形态。每个变体保留其真实必填字段。 */
export type ContentBlock =
  | (RenderBlockFields & { type: "markdown" })
  | (RenderBlockFields & {
      type: "tool-use";
      tool_name: string;
      content: string;
      // StreamBlock 的 tool-use 没有此字段，持久化终态会补齐。
      is_complete?: boolean;
    })
  | (RenderBlockFields & {
      type: "tool-result";
      tool_name: string;
      status: string;
      details: ToolResultDetail[];
      footer: string;
    })
  | (RenderBlockFields & {
      type: "diary";
      maid: string;
      date: string;
      content: string;
    })
  | (RenderBlockFields & {
      type: "diary-update";
      maid: string;
      target: string;
      replace: string;
    })
  | (RenderBlockFields & {
      type: "thought";
      theme: string;
      content: string;
      is_complete: boolean;
    })
  | (RenderBlockFields & { type: "button-click"; content: string })
  | (RenderBlockFields & { type: "html-preview"; content: string })
  | (RenderBlockFields & {
      type: "role-divider";
      role: string;
      is_end: boolean;
    })
  | (RenderBlockFields & { type: "style"; content: string })
  | (RenderBlockFields & {
      type: "tool-call-summary";
      items: ToolCallSummaryItem[];
      raw_content: string;
    });

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
  status?: "loading" | "processing" | "done" | "ready" | "desktop_only";
  attachmentOrder?: number; // 消息内附件关系的稳定顺序键，不进入同步 DTO
  internalPath?: string; // 手机本地物理路径，仅供前端通过 convertFileSrc 转换为安全 URL
  extractedText?: string;
  imageFrames?: string[];
  thumbnailPath?: string;
  createdAt?: number;
}

/** register_local_file / store_file 返回的完整本地 CAS 注册结果。 */
export interface AttachmentData {
  id: string;
  name: string;
  internalFileName: string;
  internalPath: string;
  type: string;
  size: number;
  hash: string;
  createdAt: number;
  extractedText: string | null;
  thumbnailPath: string | null;
}

export interface AttachmentRegisterProgressDto {
  progress: number;
  stableId: string;
}

/**
 * ChatMessage 接口定义，严格对齐 Rust 端的 MessageSyncDTO / ChatMessage
 */
export interface ChatMessage {
  id: string;
  role: string;
  name?: string | null;
  content?: string; // 原文，现在变为按需懒加载的可选字段
  blocks?: ContentBlock[]; // 预编译的 AST 数据块，前端直接渲染
  shell?: MessageShell; // 预计算的外壳属性
  timestamp: number;
  updatedAt?: number;

  isThinking?: boolean;
  agentId?: string;
  groupId?: string;
  isGroupMessage?: boolean;
  finishReason?: string;
  attachments?: Attachment[];
  topicId?: string;
  content_hash?: string;

  // 以下为纯前端运行时 UI 状态 (Ephemeral)，绝不进行持久化
  tailContent?: string;      // Aurora: 尾随区 Markdown (高频变动)
  tailBlock?: StreamBlock;
  tailFrame?: TailFrame;
  tailSnapshot?: MarkdownNode[];
  isReconnecting?: boolean;  // 🆕 流接续重连中状态
}

export type AstMutation =
  | { op: "add"; id: string; parent: string; node: MarkdownNode }
  | { op: "add_inline"; id: string; parent: string; node: InlineNode }
  | { op: "add_list_item"; id: string; parent: string; children: MarkdownNode[] }
  | { op: "text"; id: string; value: string }
  | { op: "append"; id: string; chunk: string }
  | { op: "prop"; id: string; key: string; value: string }
  | { op: "replace"; id: string; node: MarkdownNode }
  | { op: "replace_inline"; id: string; node: InlineNode }
  | { op: "remove"; id: string };

export interface TailFrame {
  streamId: number;
  epoch: number;
  revision: number;
  frameSeq: number;
  reset?: boolean;
  snapshot?: MarkdownNode[];
  mutations?: AstMutation[];
}

/**
 * 流式增量块定义，由 Rust 流式块解析器推送
 * 与 ContentBlock 类似但精简，用于流式期间的增量渲染
 */
interface StreamBlockFields {
  content?: string;
  nodes?: MarkdownNode[];
  /** Aurora tail 的前端运行时渲染模式；稳定块与持久化数据不携带。 */
  render_mode?: TailRenderMode;
  theme?: string;
  is_complete?: boolean;
  tool_name?: string;
  status?: string;
  details?: ToolResultDetail[];
  footer?: string;
  maid?: string;
  valet?: string;
  date?: string;
  file_name?: string;
  folder?: string;
  target?: string;
  replace?: string;
  target_nodes?: MarkdownNode[];
  replace_nodes?: MarkdownNode[];
  role?: string;
  is_end?: boolean;
  highlighted_content?: string;
  items?: ToolCallSummaryItem[];
  raw_content?: string;
  hash: string;
}

export type StreamBlock =
  | (StreamBlockFields & { type: "markdown"; content: string })
  | (StreamBlockFields & {
      type: "thought";
      theme: string;
      content: string;
      is_complete: boolean;
    })
  | (StreamBlockFields & {
      type: "tool-use";
      tool_name: string;
      content: string;
    })
  | (StreamBlockFields & {
      type: "tool-result";
      tool_name: string;
      status: string;
      details: ToolResultDetail[];
      footer: string;
    })
  | (StreamBlockFields & {
      type: "diary";
      maid: string;
      date: string;
      content: string;
    })
  | (StreamBlockFields & {
      type: "diary-update";
      maid: string;
      target: string;
      replace: string;
    })
  | (StreamBlockFields & { type: "html-preview"; content: string })
  | (StreamBlockFields & {
      type: "role-divider";
      role: string;
      is_end: boolean;
    })
  | (StreamBlockFields & { type: "style"; content: string })
  | (StreamBlockFields & { type: "button-click"; content: string })
  | (StreamBlockFields & {
      type: "tool-call-summary";
      items: ToolCallSummaryItem[];
      raw_content: string;
    });

export type TailRenderMode = "ast" | "plain";

export interface StableAppend {
  baseCount: number;
  blocks: StreamBlock[];
}

export type TailTextOp =
  | {
      op: "append";
      baseHash?: string;
      content: string;
      hash: string;
      mode: TailRenderMode;
    }
  | {
      op: "replace";
      content: string;
      hash: string;
      mode: TailRenderMode;
    }
  | { op: "clear" };

/**
 * Aurora 语义沉淀更新，由 Rust 流式管道推送
 */
export interface AuroraUpdate {
  kind: "delta" | "snapshot";
  streamId?: number;
  stableBlocks?: StreamBlock[];
  stableAppend?: StableAppend;
  tailBlock?: StreamBlock;
  tailMode?: TailRenderMode;
  tailOp?: TailTextOp;
  tailFrame?: TailFrame;
  content?: string;
  chunk?: string;
}

export interface AuroraRecoverySnapshot {
  stableBlocks: StreamBlock[];
  tailBlock?: StreamBlock;
  tailMode?: TailRenderMode;
  tailSnapshot: MarkdownNode[];
}

export interface StreamContextDto {
  topicId?: string;
  isGroupMessage?: boolean;
  groupId?: string;
  agentId?: string;
  ownerId?: string;
  ownerType?: "agent" | "group";
  agentName?: string;
  [key: string]: unknown;
}

/** Rust `StreamEvent` 的完整 Channel 载荷；Option 字段会被序列化为 null。 */
export interface StreamEventDto {
  type: "thinking" | "aurora" | "end" | "error" | "reconnecting" | "data";
  chunk: unknown | null;
  messageId: string;
  context: StreamContextDto | null;
  finishReason: string | null;
  error: string | null;
  content?: string | null;
  aurora: AuroraUpdate | null;
  blocks: ContentBlock[] | null;
  timestamp: number | null;
}

export interface ActiveGenerationDto {
  msgId: string;
  topicId: string;
  ownerId: string;
  ownerType: string;
  createdAt: number;
  agentId: string | null;
  agentName: string | null;
}

export type RecoveryResultDto =
  | { status: "already_running" }
  | { status: "completed"; content: string; finishReason?: string | null }
  | { status: "failed" | "not_found"; content: string };

export type GroupChatResultDto =
  | { status: "completed" }
  | { status: "no_ai_response"; reason: "invite_only" | "no_speakers" }
  | {
      status: "no_ai_response";
      reason: "mode_not_implemented";
      mode: string;
    };

export interface RegenerateTopicResultDto {
  generation: unknown;
  msgCount: number;
}
