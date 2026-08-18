import { describe, expect, it } from 'vitest';
import {
  extractMentionedMemberIds,
  splitMentionSegments,
} from '../../../core/utils/mention';

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

  it('matches the full-width ＠ produced by Chinese IMEs (device regression)', () => {
    // 血训：中文输入法全角 ＠ 曾导致 invite_only 自动邀约完全失效
    expect(extractMentionedMemberIds('＠Nova 你好', members)).toEqual(['a1']);
    expect(extractMentionedMemberIds('大家好＠小诺', members)).toEqual(['a3']);
    expect(extractMentionedMemberIds('@Nova 和 ＠Luna', members)).toEqual(['a1', 'a2']);
  });

  it('prefers the longest name when names overlap at the same position', () => {
    const overlapping = [
      { id: 'a1', name: '小诺' },
      { id: 'a2', name: '小诺亚' },
    ];
    expect(extractMentionedMemberIds('@小诺亚 你好', overlapping)).toEqual(['a2']);
  });
});

describe('splitMentionSegments', () => {
  it('splits text into mention/plain segments that rejoin losslessly', () => {
    const names = ['Nova', '小诺'];
    const segments = splitMentionSegments('你好 @Nova 看＠小诺 的消息', names);
    expect(segments).toEqual([
      { text: '你好 ', mention: false },
      { text: '@Nova', mention: true },
      { text: ' 看', mention: false },
      { text: '＠小诺', mention: true },
      { text: ' 的消息', mention: false },
    ]);
    expect(segments.map((s) => s.text).join('')).toBe('你好 @Nova 看＠小诺 的消息');
  });

  it('returns a single plain segment when nothing matches or text is empty', () => {
    expect(splitMentionSegments('大家好', ['Nova'])).toEqual([
      { text: '大家好', mention: false },
    ]);
    expect(splitMentionSegments('', ['Nova'])).toEqual([]);
  });
});
