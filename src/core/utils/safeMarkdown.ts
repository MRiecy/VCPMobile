//! 共享的安全 Markdown 渲染边界：marked（GFM + breaks）→ filterTrustedRichHtml。
//!
//! 所有"服务端来的半信任富文本"（论坛帖子、邮件正文等）的 v-html 输入
//! 必须经此唯一入口；纯文本字段（文件名、错误信息、元数据）继续用 Vue 文本绑定。

import { Marked } from 'marked';
import { filterTrustedRichHtml } from './astRenderer';

const sharedMarked = new Marked({ gfm: true, breaks: true });

/**
 * Markdown → 受护栏 HTML。filterTrustedRichHtml 过滤 script/活动文档/
 * javascript: URL/宿主能力调用，与聊天富 HTML 同一产品安全基线。
 */
export function renderSafeMarkdown(content: string): string {
  try {
    const html = sharedMarked.parse(content) as string;
    return filterTrustedRichHtml(html);
  } catch (error) {
    console.error('[SafeMarkdown] render failed', error);
    return '<p>内容渲染失败。</p>';
  }
}
