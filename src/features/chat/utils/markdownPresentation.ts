import type { ContentBlock, MarkdownNode } from '../../../core/types/chat';

interface PresentationGroup {
  id: string;
  blocks: ContentBlock[];
}

/** Hashes narrow candidates only: Rust u64 hashes can lose precision in JS. */
function sameAstValue(left: unknown, right: unknown): boolean {
  if (left === right) return true;
  if (left === null || right === null || typeof left !== 'object' || typeof right !== 'object') {
    return false;
  }
  if (Array.isArray(left) || Array.isArray(right)) {
    return Array.isArray(left) && Array.isArray(right)
      && left.length === right.length
      && left.every((value, index) => sameAstValue(value, right[index]));
  }
  const leftRecord = left as Record<string, unknown>;
  const rightRecord = right as Record<string, unknown>;
  const leftKeys = Object.keys(leftRecord).filter(key => key !== 'hash');
  const rightKeys = Object.keys(rightRecord).filter(key => key !== 'hash');
  return leftKeys.length === rightKeys.length && leftKeys.every(key =>
    Object.prototype.hasOwnProperty.call(rightRecord, key) && sameAstValue(leftRecord[key], rightRecord[key]),
  );
}

function candidateKey(node: MarkdownNode): string {
  return `${node.type}:${'hash' in node ? node.hash ?? '' : ''}`;
}

/**
 * Message-local presentation identity, independent of transport block boundaries.
 * Only the previous projection is retained; AST nodes and large strings are shared.
 */
export function createMarkdownPresentation(onCreate?: (source: ContentBlock, projected: ContentBlock) => void) {
  let owner = '';
  let nextIdentity = 0;
  let previous = new Map<string, ContentBlock[]>();
  const keys = new WeakMap<ContentBlock, string>();
  const rawHtmlNodes = new WeakMap<object, boolean>();

  function containsRawHtml(value: unknown): boolean {
    if (value === null || typeof value !== 'object') return false;
    const cached = rawHtmlNodes.get(value);
    if (cached !== undefined) return cached;
    const record = value as Record<string, unknown>;
    const result = record.type === 'raw_html' || record.type === 'raw_html_inline'
      || Object.values(record).some(containsRawHtml);
    rawHtmlNodes.set(value, result);
    return result;
  }

  return {
    getKey: (block: ContentBlock): string | undefined => keys.get(block),
    project<T extends PresentationGroup>(messageId: string, groups: T[], enabled = true): T[] {
      if (owner !== messageId || !enabled) {
        previous = new Map();
        owner = messageId;
      }
      if (!enabled) return groups;
      const current = new Map<string, ContentBlock[]>();
      const projected = groups.map(group => {
        const candidates = new Map<string, ContentBlock[]>();
        for (const block of previous.get(group.id) ?? []) {
          const key = candidateKey(block.nodes![0]);
          const bucket = candidates.get(key);
          if (bucket) bucket.push(block);
          else candidates.set(key, [block]);
        }
        const occurrences = new Map<string, number>();
        const currentMarkdown: ContentBlock[] = [];
        const blocks: ContentBlock[] = [];
        for (const source of group.blocks) {
          // Raw tags may open in one node and close in another. Keep their full
          // existing context, including inline tags and standalone style inputs.
          if (source.type !== 'markdown' || !source.nodes?.length || containsRawHtml(source.nodes)) {
            blocks.push(source);
            continue;
          }
          for (const node of source.nodes) {
            const key = candidateKey(node);
            const occurrence = occurrences.get(key) ?? 0;
            occurrences.set(key, occurrence + 1);
            const candidate = candidates.get(key)?.[occurrence];
            // Compare the same occurrence only. Collisions can lose reuse, but
            // cannot reuse stale content or cause quadratic candidate searches.
            const block = candidate && sameAstValue(candidate.nodes![0], node)
              ? candidate
              : { type: 'markdown' as const, nodes: [node] };
            if (!keys.has(block)) {
              keys.set(block, `${messageId}-markdown-view-${nextIdentity++}`);
              onCreate?.(source, block);
            }
            blocks.push(block);
            currentMarkdown.push(block);
          }
        }
        current.set(group.id, currentMarkdown);
        return { ...group, blocks };
      });
      previous = current;
      return projected;
    },
  };
}
