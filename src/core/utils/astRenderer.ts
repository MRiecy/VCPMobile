import { convertFileSrc } from "@tauri-apps/api/core";
import type { MarkdownNode, InlineNode } from "../types/chat";

const ACTIVE_URL_ATTRIBUTES = new Set([
  "action",
  "archive",
  "background",
  "cite",
  "codebase",
  "data",
  "formaction",
  "href",
  "longdesc",
  "manifest",
  "poster",
  "profile",
  "src",
  "usemap",
  "xlink:href",
]);

const ACTIVE_DOCUMENT_TAGS = new Set(["applet", "base", "embed", "object", "script"]);
const PASSIVE_DATA_MEDIA_TAGS = new Set(["audio", "img", "source", "track", "video"]);
const DEFAULT_IFRAME_SANDBOX = "allow-scripts allow-forms allow-modals allow-popups allow-downloads";
const TRUSTED_INTERNAL_HANDLER = /^(?:window\.)?__(?:vcpFixEmoticon|vcpShowEmoticon)\(\s*this\s*\)\s*;?$/i;

// Product-profile guard: preserve VCPChat-style rich HTML and local DOM-only interactions, while
// removing the small active surface that can directly reach the Tauri host or exfiltrate state.
// This is intentionally not presented as a general-purpose sanitizer: deliberately obfuscated
// JavaScript remains part of the accepted risk for trusted-circle assistant content.
const HOST_CAPABILITY_IN_HANDLER = /(?:__TAURI(?:_INTERNALS__)?|\binvoke\s*\(|\b(?:window|document|globalThis|self|parent|top|opener|frames)\b|\b(?:ownerDocument|defaultView|location|history)\b|\b(?:fetch|XMLHttpRequest|WebSocket|EventSource|sendBeacon)\b|\b(?:localStorage|sessionStorage|indexedDB|cookie)\b|\bnavigator\s*(?:\.|\[)|\b(?:eval|Function|constructor|setTimeout|setInterval|import)\b|\b(?:innerHTML|outerHTML|insertAdjacentHTML|document\.write)\b|\b(?:postMessage|submit|requestSubmit)\s*\()/i;

function compactScheme(value: string): string {
  return value
    .trim()
    .replace(/[\u0000-\u0020\u007f-\u009f\u00a0\u1680\u180e\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]/g, "")
    .toLowerCase();
}

/**
 * Returns null only for URL forms that can execute an active document or script. Normal network,
 * asset, blob and passive data-media URLs remain untouched for rich-message fidelity.
 */
export function filterTrustedRichHtmlUrl(
  value: string,
  tagName = "",
  attributeName = "href",
): string | null {
  const compact = compactScheme(value);
  if (/^(?:javascript|vbscript):/.test(compact)) return null;

  if (compact.startsWith("data:")) {
    const mime = compact.slice(5).split(/[;,]/, 1)[0];
    const tag = tagName.toLowerCase();
    const isPassiveMedia = PASSIVE_DATA_MEDIA_TAGS.has(tag)
      && /^(?:image|audio|video)\//.test(mime);
    const isActiveDocument = /^(?:text\/html|application\/xhtml\+xml|image\/svg\+xml|text\/xml|application\/xml)$/.test(mime);

    if (isActiveDocument && !isPassiveMedia) return null;
  }

  // srcset can contain multiple candidates; reject only candidates with an executable scheme.
  if (attributeName.toLowerCase() === "srcset") {
    const compactCandidates = value
      .split(/\s*,\s*/)
      .map((candidate) => compactScheme(candidate.split(/\s+/, 1)[0] || ""));
    if (compactCandidates.some((candidate) => /^(?:javascript|vbscript):/.test(candidate))) {
      return null;
    }
  }

  return value;
}

function handlerReferencesHostCapability(value: string): boolean {
  if (TRUSTED_INTERNAL_HANDLER.test(value.trim())) return false;
  return HOST_CAPABILITY_IN_HANDLER.test(value);
}

function hardenIframe(iframe: Element): void {
  const srcdoc = iframe.getAttribute("srcdoc");
  if (srcdoc !== null) {
    iframe.setAttribute("srcdoc", filterTrustedRichHtml(srcdoc));
  }

  const requestedSandbox = iframe.getAttribute("sandbox");
  if (requestedSandbox === null) {
    iframe.setAttribute("sandbox", DEFAULT_IFRAME_SANDBOX);
    return;
  }

  const safeTokens = requestedSandbox
    .split(/\s+/)
    .filter(Boolean)
    .filter((token) => {
      const normalized = token.toLowerCase();
      return normalized !== "allow-same-origin" && !normalized.startsWith("allow-top-navigation");
    });
  iframe.setAttribute("sandbox", Array.from(new Set(safeTokens)).join(" "));
}

function filterElementActiveContent(element: Element): void {
  const tagName = element.tagName.toLowerCase();
  if (ACTIVE_DOCUMENT_TAGS.has(tagName)) {
    element.remove();
    return;
  }

  if (
    tagName === "meta"
    && element.getAttribute("http-equiv")?.trim().toLowerCase() === "refresh"
  ) {
    element.remove();
    return;
  }

  for (const attribute of Array.from(element.attributes)) {
    const attributeName = attribute.name.toLowerCase();
    if (attributeName.startsWith("on") && handlerReferencesHostCapability(attribute.value)) {
      element.removeAttribute(attribute.name);
      continue;
    }

    if (attributeName === "ping") {
      element.removeAttribute(attribute.name);
      continue;
    }

    if (
      (ACTIVE_URL_ATTRIBUTES.has(attributeName) || attributeName === "srcset")
      && filterTrustedRichHtmlUrl(attribute.value, tagName, attributeName) === null
    ) {
      element.removeAttribute(attribute.name);
    }
  }

  if ((tagName === "a" || tagName === "area") && element.getAttribute("target") === "_blank") {
    const rel = new Set((element.getAttribute("rel") || "").split(/\s+/).filter(Boolean));
    rel.add("noopener");
    rel.add("noreferrer");
    element.setAttribute("rel", Array.from(rel).join(" "));
  }

  if (tagName === "iframe") hardenIframe(element);
  if (element instanceof HTMLTemplateElement) filterTrustedRichHtmlDom(element.content);
}

/** Applies the product-profile active-content guard to an existing detached or live subtree. */
export function filterTrustedRichHtmlDom(root: ParentNode): void {
  for (const element of Array.from(root.querySelectorAll("*"))) {
    filterElementActiveContent(element);
  }
}

/**
 * Keeps rich HTML intact while filtering direct host-capability entry points before main-DOM use.
 * Interactive full documents that need unrestricted JavaScript continue to use HtmlPreviewBlock.
 */
export function filterTrustedRichHtml(html: string): string {
  if (!html || typeof document === "undefined") return html;
  const template = document.createElement("template");
  template.innerHTML = html;
  filterTrustedRichHtmlDom(template.content);
  return template.innerHTML;
}

// HTML 缓存：避免重复遍历 AST 拼接相同内容
const htmlCache = new Map<string, string>();
const MAX_CACHE_SIZE = 500;

function getCacheKey(messageId: string, blockHash?: string | number): string | null {
  if (blockHash !== undefined && blockHash !== null) {
    return `${messageId}:${String(blockHash)}`;
  }
  return null;
}

/** 清理 AST HTML 缓存，用于重建/同步后强制重新渲染 */
export function clearHtmlCache(): void {
  htmlCache.clear();
}

/** 清理单条消息的 AST HTML 缓存，用于编辑后强制重新渲染 */
export function clearMessageCache(messageId: string): void {
  const prefix = `${messageId}:`;
  for (const key of htmlCache.keys()) {
    if (key.startsWith(prefix)) htmlCache.delete(key);
  }
}

/**
 * 将 Rust 预渲染的 AST 节点树转换为 HTML 字符串
 */
export function renderMarkdownNodes(
  nodes: MarkdownNode[], 
  messageId: string,
  blockHash?: string | number
): string {
  if (!nodes || nodes.length === 0) return '';
  const key = getCacheKey(messageId, blockHash);

  if (key) {
    const cached = htmlCache.get(key);
    if (cached !== undefined) return cached;
  }

  const html = filterTrustedRichHtml(nodes.map(node => renderNode(node, messageId)).join(''));

  // 无 hash 时不缓存，避免不同内容但节点数量相同的 legacy AST 串用 HTML
  if (!key) return html;

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
      return `<p>${node.children.map(renderInline).join('')}</p>`;
    
    case 'heading':
      const level = node.level || 1;
      return `<h${level}>${node.children.map(renderInline).join('')}</h${level}>`;
    
    case 'code_block': {
      if (node.lang === 'mermaid') {
        return `<div class="mermaid-placeholder">${escapeHtml(node.code)}</div>`;
      }
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
      return `<pre class="vcp-code-block vcp-scrollable"><code>${escapeHtml(node.code)}</code></pre>`;
    }
    
    case 'blockquote':
      return `<blockquote>${node.children.map((n) => renderNode(n, messageId)).join('')}</blockquote>`;
    
    case 'list':
      const tag = node.ordered ? 'ol' : 'ul';
      const itemsHtml = node.items.map(itemNodes =>
        `<li>${itemNodes.map(n => renderNode(n, messageId)).join('')}</li>`
      ).join('');
      return `<${tag}>${itemsHtml}</${tag}>`;
    
    case 'table':
      const headerHtml = `<tr>${node.header.map(cell => `<th>${cell.map(renderInline).join('')}</th>`).join('')}</tr>`;
      const bodyHtml = node.rows.map(row =>
        `<tr>${row.map(cell => `<td>${cell.map(renderInline).join('')}</td>`).join('')}</tr>`
      ).join('');
      const wrapper = node.wrapper_class || 'vcp-table-wrapper';
      return `<div class="${wrapper}"><table><thead>${headerHtml}</thead><tbody>${bodyHtml}</tbody></table></div>`;
    
    case 'thematic_break':
      return '<hr/>';
    

    
    case 'raw_html':
      return node.content;
    
    default:
      return '';
  }
}

function renderInline(node: InlineNode): string {
  switch (node.type) {
    case 'text':
      return escapeHtml(node.value);
    
    case 'strong':
      return `<strong>${node.children.map(renderInline).join('')}</strong>`;
    
    case 'emphasis':
      return `<em>${node.children.map(renderInline).join('')}</em>`;
    
    case 'strikethrough':
      return `<del>${node.children.map(renderInline).join('')}</del>`;
    
    case 'code':
      return `<code>${escapeHtml(node.value)}</code>`;
    
    case 'link': {
      const rawHref = node.needs_asset_conversion
        ? convertFileSrc(node.href)
        : node.href;
      const href = filterTrustedRichHtmlUrl(rawHref, 'a', 'href');
      const hrefAttribute = href === null ? '' : ` href="${escapeHtml(href)}"`;
      return `<a${hrefAttribute} title="${escapeHtml(node.title || '')}" target="_blank" rel="noopener noreferrer">${node.children.map(renderInline).join('')}</a>`;
    }
    
    case 'image': {
      const rawSrc = node.needs_asset_conversion
        ? convertFileSrc(node.src)
        : node.src;
      const src = filterTrustedRichHtmlUrl(rawSrc, 'img', 'src');
      const srcAttribute = src === null ? '' : ` src="${escapeHtml(src)}"`;
      return `<img${srcAttribute} alt="${escapeHtml(node.alt || '')}" title="${escapeHtml(node.title || '')}" loading="lazy" class="vcp-markdown-image" />`;
    }
    
    case 'break':
      return '<br/>';
    
    case 'inline_math': {
      const isDisplay = node.display_mode || false;
      const cls = isDisplay ? 'vcp-math-block no-swipe' : 'vcp-math-inline no-swipe';
      const tag = 'span';
      return `<${tag} class="${cls}" data-latex="${escapeHtml(node.content || '')}">${escapeHtml(node.content || '')}</${tag}>`;
    }
    
    case 'vcp_custom': {
      const cls = `vcp-custom-${node.kind}`;
      if (node.children && node.children.length > 0) {
        const innerContent = (node.children || []).map(renderInline).join('');
        return `<span class="${cls}">${innerContent}</span>`;
      }
      return `<span class="${cls}">${escapeHtml(node.value || '')}</span>`;
    }
    
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
