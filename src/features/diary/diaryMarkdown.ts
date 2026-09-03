import { Marked } from "marked";
import { withCodeBlockClass } from "../../core/utils/astRenderer";

const diaryMarked = new Marked({
  gfm: true,
  breaks: true,
});

/**
 * Diary bodies are a trusted server-side content class by product contract.
 * Keep this as the sole v-html input boundary; filenames, errors and search
 * metadata must continue to use Vue text bindings.
 */
export function renderDiaryMarkdown(content: string): string {
  try {
    const safeReferences = content.replace(/^(\s*)\[([^\]]+)\]:/gm, "$1\\[$2\\]:");
    return withCodeBlockClass(diaryMarked.parse(safeReferences) as string);
  } catch (error) {
    console.error("[DiaryCenter] Markdown render failed", error);
    return "<p>正文渲染失败，请切换到编辑态查看原文。</p>";
  }
}

const HIGHLIGHT_EXCLUDED_TAGS = new Set([
  "CODE",
  "MARK",
  "NOSCRIPT",
  "PRE",
  "SCRIPT",
  "STYLE",
  "TEXTAREA",
]);

function canHighlightTextNode(node: Text): boolean {
  let parent = node.parentElement;
  while (parent) {
    if (HIGHLIGHT_EXCLUDED_TAGS.has(parent.tagName)) return false;
    parent = parent.parentElement;
  }
  return true;
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** Adds visual-only literal matches without altering trusted body markup. */
export function highlightDiarySearchText(html: string, term: string): string {
  const needle = term.trim();
  if (!needle || typeof document === "undefined") return html;

  const template = document.createElement("template");
  template.innerHTML = html;
  const walker = document.createTreeWalker(template.content, NodeFilter.SHOW_TEXT);
  const nodes: Text[] = [];
  let current = walker.nextNode();
  while (current) {
    if (current instanceof Text && canHighlightTextNode(current)) nodes.push(current);
    current = walker.nextNode();
  }

  const matcher = new RegExp(escapeRegExp(needle), "giu");
  for (const node of nodes) {
    const value = node.nodeValue ?? "";
    const matches = [...value.matchAll(matcher)];
    if (matches.length === 0) continue;

    const fragment = document.createDocumentFragment();
    let cursor = 0;
    for (const match of matches) {
      const matchIndex = match.index;
      const matchedText = match[0];
      fragment.append(value.slice(cursor, matchIndex));
      const mark = document.createElement("mark");
      mark.className = "diary-search-mark";
      mark.textContent = matchedText;
      fragment.append(mark);
      cursor = matchIndex + matchedText.length;
    }
    fragment.append(value.slice(cursor));
    node.replaceWith(fragment);
  }

  return template.innerHTML;
}

export function renderDiaryMarkdownWithHighlight(content: string, term = ""): string {
  return highlightDiarySearchText(renderDiaryMarkdown(content), term);
}
