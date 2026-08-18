import { describe, expect, it } from 'vitest';
import { extractMentionedMemberIds } from '../../../core/utils/mention';

const members = [
  { id: 'a1', name: 'Nova' },
  { id: 'a2', name: 'Luna' },
  { id: 'a3', name: '小诺' },
];

describe('extractMentionedMemberIds', () => {
  it('matches @name case-insensitively (policy parity)', () => {
    expect(extractMentionedMemberIds('@nova 你好', members)).toEqual(['a1']);
    expect(extractMentionedMemberIds('@NOVA 你好', members)).toEqual(['a1']);
    expect(extractMentionedMemberIds('@小诺 在吗', members)).toEqual(['a3']);
  });

  it('orders by first appearance and dedupes repeated mentions', () => {
    expect(extractMentionedMemberIds('@Luna 和 @Nova 还有 @luna', members)).toEqual([
      'a2',
      'a1',
    ]);
  });

  it('returns empty when nobody is mentioned', () => {
    expect(extractMentionedMemberIds('大家好', members)).toEqual([]);
    expect(extractMentionedMemberIds('@不存在的人 你好', members)).toEqual([]);
  });

  it('ignores members with blank names', () => {
    const withBlank = [...members, { id: 'a4', name: '  ' }];
    expect(extractMentionedMemberIds('@ 你好', withBlank)).toEqual([]);
  });
});
