import { describe, expect, it } from 'vitest';
import {
  extractMentionedMemberIds,
  splitMentionSegments,
} from '../../../core/utils/mention';

describe('extractMentionedMemberIds', () => {
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

});
