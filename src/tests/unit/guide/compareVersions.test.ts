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

  it('returns 0 for equal versions', () => {
    expect(compareVersions('1.1.4', '1.1.4')).toBe(0);
  });

  it('orders multi-segment versions correctly', () => {
    expect(compareVersions('1.2.3', '1.2.10')).toBe(-1);
    expect(compareVersions('2.0.0', '1.99.99')).toBe(1);
    expect(compareVersions('0.10.0', '0.9.9')).toBe(1);
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

  it('returns the same reference when order is empty', () => {
    expect(sortByOrder(items, [])).toBe(items);
  });

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

  it('returns items unchanged when none of the ids appear in order', () => {
    const result = sortByOrder(items, ['x', 'y']);
    expect(result.map((i) => i.id)).toEqual(['a', 'b', 'c']);
  });
});
