import { convertFileSrc } from "@tauri-apps/api/core";
import type { MarkdownNode, InlineNode } from "../types/chat";

// HTML 缓存：避免重复遍历 AST 拼接相同内容
const htmlCache = new Map<string, string>();
const MAX_CACHE_SIZE = 500;

function getCacheKey(nodes: MarkdownNode[], messageId: string): string {
  // 轻量内容指纹：JSON 前 200 字符 + 消息 ID
  return `${messageId}:${JSON.stringify(nodes).slice(0, 200)}`;
}

/** 清理 AST HTML 缓存，用于重建/同步后强制重新渲染 */
export function clearHtmlCache(): void {
  htmlCache.clear();
}

/**
 * 将 Rust 预渲染的 AST 节点树转换为 HTML 字符串
 */
export function renderMarkdownNodes(
  nodes: MarkdownNode[], 
  messageId: string
): string {
  if (!nodes || nodes.length === 0) return '';
  const key = getCacheKey(nodes, messageId);
  const cached = htmlCache.get(key);
  if (cached !== undefined) return cached;

  const html = nodes.map(node => renderNode(node, messageId)).join('');

  // 简单的 LRU 保护：超限时清空（实际命中模式是批量命中/失效）
  if (htmlCache.size >= MAX_CACHE_SIZE) {
    htmlCache.clear();
  }
  htmlCache.set(key, html);
  return html;
}

function renderNode(node: MarkdownNode, messageId: string): string {
  switch (node.type) {
    case 'paragraph':
      return `<p>${(node.children || []).map(renderInline).join('')}</p>`;
    
    case 'heading':
      const level = node.level || 1;
      return `<h${level}>${(node.children || []).map(renderInline).join('')}</h${level}>`;
    
    case 'code_block': {
      let html = node.highlighted_html;
      if (html) {
        // 兼容旧 AST：如果 highlighted_html 是 <pre><code> 包裹内层 <pre> 的嵌套结构，提取单层
        const nestedPreMatch = html.match(/<pre[^>]*>\s*<code>([\s\S]*?)<\/code>\s*<\/pre>/i);
        if (nestedPreMatch && nestedPreMatch[1].trim().startsWith('<pre')) {
          const innerMatch = nestedPreMatch[1].match(/<pre[^>]*>([\s\S]*?)<\/pre>/i);
          if (innerMatch) {
            html = `<pre class="vcp-code-block vcp-scrollable">${innerMatch[1]}</pre>`;
          }
        }
        return html;
      }
      return `<pre class="vcp-code-block vcp-scrollable"><code>${escapeHtml(node.code || '')}</code></pre>`;
    }
    
    case 'blockquote':
      return `<blockquote>${(node.children || []).map((n: any) => renderNode(n, messageId)).join('')}</blockquote>`;
    
    case 'list':
      const tag = node.ordered ? 'ol' : 'ul';
      const itemsHtml = (node.items || []).map(itemNodes => 
        `<li>${itemNodes.map(n => renderNode(n, messageId)).join('')}</li>`
      ).join('');
      return `<${tag}>${itemsHtml}</${tag}>`;
    
    case 'table':
      const headerHtml = `<tr>${(node.header || []).map(cell => `<th>${(cell as any).map(renderInline).join('')}</th>`).join('')}</tr>`;
      const bodyHtml = (node.rows || []).map(row =>
        `<tr>${row.map(cell => `<td>${(cell as any).map(renderInline).join('')}</td>`).join('')}</tr>`
      ).join('');
      const wrapper = node.wrapper_class || 'vcp-table-wrapper';
      return `<div class="${wrapper}"><table><thead>${headerHtml}</thead><tbody>${bodyHtml}</tbody></table></div>`;
    
    case 'thematic_break':
      return '<hr/>';
    
    case 'mermaid':
      return `<div class="mermaid-placeholder">${escapeHtml(node.code || '')}</div>`;
    
    case 'raw_html':
      return node.content || '';
    
    default:
      return '';
  }
}

function renderInline(node: InlineNode): string {
  switch (node.type) {
    case 'text':
      return escapeHtml(node.value || '');
    
    case 'strong':
      return `<strong>${(node.children || []).map(renderInline).join('')}</strong>`;
    
    case 'emphasis':
      return `<em>${(node.children || []).map(renderInline).join('')}</em>`;
    
    case 'code':
      return `<code>${escapeHtml(node.value || '')}</code>`;
    
    case 'link': {
      const href = node.needs_asset_conversion && node.href
        ? convertFileSrc(node.href)
        : escapeHtml(node.href || '');
      return `<a href="${href}" title="${escapeHtml(node.title || '')}" target="_blank" rel="noopener noreferrer">${(node.children || []).map(renderInline).join('')}</a>`;
    }
    
    case 'image': {
      const src = node.needs_asset_conversion && node.src
        ? convertFileSrc(node.src)
        : escapeHtml(node.src || '');
      return `<img src="${src}" alt="${escapeHtml(node.alt || '')}" title="${escapeHtml(node.title || '')}" loading="lazy" class="vcp-markdown-image" />`;
    }
    
    case 'line_break':
      return '<br/>';
    
    case 'soft_break':
      return '<br/>';
    
    case 'inline_math': {
      const isDisplay = node.display_mode || false;
      const cls = isDisplay ? 'vcp-math-block no-swipe' : 'vcp-math-inline no-swipe';
      const tag = isDisplay ? 'div' : 'span';
      if (node.svg) {
        return `<${tag} class="${cls}">${node.svg}</${tag}>`;
      }
      return `<${tag} class="${cls}" data-latex="${escapeHtml(node.content || '')}">${escapeHtml(node.content || '')}</${tag}>`;
    }
    
    case 'quoted_text':
      return `<span class="highlighted-quote">${escapeHtml(node.value || '')}</span>`;
    
    case 'highlight_tag':
      return `<span class="highlighted-tag">${escapeHtml(node.value || '')}</span>`;
    
    case 'alert_tag':
      return `<span class="highlighted-alert-tag">${escapeHtml(node.value || '')}</span>`;
    
    case 'raw_html_inline':
      return node.content || '';
    
    default:
      return '';
  }
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#039;');
}
