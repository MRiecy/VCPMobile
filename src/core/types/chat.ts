import type { ContentBlock } from "../composables/useContentProcessor";

export type { ContentBlock };

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
  timestamp: number;

  isThinking?: boolean;
  agentId?: string;
  groupId?: string;
  isGroupMessage?: boolean;
  finishReason?: string;
  attachments?: Attachment[];

  // 以下为纯前端运行时 UI 状态 (Ephemeral)，绝不进行持久化
  displayedContent?: string; // 用于兼容旧版渲染器的全量文本
  stableContent?: string;    // Aurora: 稳定区 HTML/Markdown
  tailContent?: string;      // Aurora: 尾随区 Markdown (高频变动)
  processedContent?: string; // 缓存 Rust 返回的 AST 或文本，避免重复解析
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
}
