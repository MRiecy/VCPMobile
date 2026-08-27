import { describe, expect, it } from 'vitest';
import { compareVersions } from '@/features/guide/stores/guideStore';
import { sortByOrder } from '@/features/agent/agentOrder';

describe('compareVersions', () => {
  it('compares segments numerically so 1.9 < 1.10', () => {
    expect(compareVersions('1.9', '1.10')).toBe(-1);
    expect(compareVersions('1.10', '1.9')).toBe(1);
  });

  it('treats missing segments as zero', () => {
    expect(compareVersions('1.10', '1.10.0')).toBe(0);
    expect(compareVersions('1.10.0', '1.10')).toBe(0);
    expect(compareVersions('1.9', '1.9.1')).toBe(-1);
  });

  it('tolerates non-numeric segments by treating them as zero', () => {
    expect(compareVersions('1.x.2', '1.0.2')).toBe(0);
  });
});

describe('sortByOrder (shared agent/group ordering contract)', () => {
  const items = [
    { id: 'a' },
    { id: 'b' },
    { id: 'c' },
  ];

  it('sorts by explicit order with unknown ids last', () => {
    const result = sortByOrder(items, ['c', 'a']);
    expect(result.map((i) => i.id)).toEqual(['c', 'a', 'b']);
  });

  it('keeps unknown ids in original relative order', () => {
    const result = sortByOrder(items, ['c']);
    expect(result.map((i) => i.id)).toEqual(['c', 'a', 'b']);
  });

  it('does not mutate the input array', () => {
    const snapshot = items.map((i) => i.id);
    sortByOrder(items, ['b', 'a']);
    expect(items.map((i) => i.id)).toEqual(snapshot);
  });

});
