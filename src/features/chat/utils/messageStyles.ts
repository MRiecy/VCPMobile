import type { ContentBlock } from '../../../core/types/chat';

/** Existing scoped-style inputs: explicit style blocks and isolated Markdown style nodes. */
export function standaloneStyleSource(block: ContentBlock): string | null {
  if (block.type !== 'markdown' || block.nodes?.length !== 1) return null;
  const node = block.nodes[0];
  return node.type === 'raw_html' && node.content.trimStart().toLowerCase().startsWith('<style')
    ? node.content
    : null;
}

/** The fallback HTML and its CSS are projected together, before either is committed. */
export function separateMessageStyles(html: string): { html: string; css: string } {
  const styles: string[] = [];
  const body = html.replace(/<style\b[^>]*>([\s\S]*?)(?:<\/style>|$)/gi, (_, css: string) => {
    if (css.trim()) styles.push(css.trim());
    return '';
  });
  return { html: body, css: styles.join('\n') };
}

export function stableMessageStyles(blocks: ContentBlock[] | undefined): string {
  const styles: string[] = [];
  for (const block of blocks ?? []) {
    if (block.type === 'style') {
      if (block.content?.trim()) styles.push(block.content);
    } else {
      const source = standaloneStyleSource(block);
      if (source !== null) {
        const css = separateMessageStyles(source).css;
        if (css) styles.push(css);
      }
    }
  }
  return styles.join('\n');
}
