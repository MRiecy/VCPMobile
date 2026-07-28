const MESSAGE_HTML_TAGS = new Set([
  "a",
  "abbr",
  "article",
  "aside",
  "b",
  "bdi",
  "bdo",
  "blockquote",
  "br",
  "caption",
  "cite",
  "code",
  "col",
  "colgroup",
  "dd",
  "del",
  "details",
  "dfn",
  "div",
  "dl",
  "dt",
  "em",
  "figcaption",
  "figure",
  "footer",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "header",
  "hr",
  "i",
  "img",
  "ins",
  "kbd",
  "li",
  "main",
  "mark",
  "ol",
  "p",
  "pre",
  "q",
  "s",
  "samp",
  "section",
  "small",
  "span",
  "strong",
  "style",
  "sub",
  "summary",
  "sup",
  "table",
  "tbody",
  "td",
  "tfoot",
  "th",
  "thead",
  "time",
  "tr",
  "u",
  "ul",
  "var",
  "wbr",
]);

function extractTagName(fragment: string): string | null {
  const trimmed = fragment.trim();
  if (!trimmed.startsWith("<") || !trimmed.endsWith(">")) return null;

  let cursor = 1;
  while (cursor < trimmed.length && /\s/.test(trimmed[cursor])) cursor += 1;
  if (trimmed[cursor] === "/") cursor += 1;
  while (cursor < trimmed.length && /\s/.test(trimmed[cursor])) cursor += 1;

  const start = cursor;
  while (cursor < trimmed.length && /[a-zA-Z0-9-]/.test(trimmed[cursor])) {
    cursor += 1;
  }

  return cursor > start ? trimmed.slice(start, cursor).toLowerCase() : null;
}

/**
 * 仅将项目明确支持的 HTML 标签交给 WebView 解析。
 * 模型输出的伪标签（如 <reason>）必须按普通文本显示，避免内容被 DOM 吞掉。
 */
export function shouldRenderMessageHtml(fragment: string): boolean {
  const trimmed = fragment.trim();
  if (!trimmed) return false;
  if (trimmed.startsWith("<!--") && trimmed.endsWith("-->")) return true;

  const tagName = extractTagName(trimmed);
  return tagName !== null && MESSAGE_HTML_TAGS.has(tagName);
}

export function escapeMessageHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

export function renderMessageRawHtml(fragment: string): string {
  return shouldRenderMessageHtml(fragment)
    ? fragment
    : escapeMessageHtml(fragment);
}
