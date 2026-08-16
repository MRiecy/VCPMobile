import { describe, expect, it } from 'vitest';
import chatViewSource from '@/features/chat/ChatView.vue?raw';
import inputEnhancerSource from '@/features/chat/InputEnhancer.vue?raw';
import agentListSource from '@/features/agent/AgentList.vue?raw';
import diaryNoteListSource from '@/features/diary/components/DiaryNoteList.vue?raw';
import '@/features/guide/guides';
import { allGuides } from '@/features/guide/registry';

const guideById = (id: string) => {
  const guide = allGuides().find((g) => g.id === id);
  if (!guide) throw new Error(`guide ${id} not registered`);
  return guide;
};

describe('guide anchor / definition contract', () => {
  it('keeps business component anchors aligned with guide definitions', () => {
    expect(chatViewSource).toContain(`v-guide="'chat-theme-button'"`);
    expect(inputEnhancerSource).toContain(`v-guide="'chat-plus-button'"`);
    expect(agentListSource).toContain(`v-guide="'agent-row-' + agent.id"`);
    expect(diaryNoteListSource).toContain(`v-guide="'diary-note-row'"`);

    const theme = guideById('theme-longpress');
    expect(theme.steps.map((s) => s.target)).toEqual(['chat-theme-button', 'chat-theme-button']);

    const plus = guideById('plus-longpress');
    expect(plus.steps.map((s) => s.target)).toEqual(['chat-plus-button', 'chat-plus-button']);

    const diary = guideById('diary-longpress');
    expect(diary.steps.map((s) => s.target)).toEqual(['diary-note-row', 'diary-note-row']);
  });

  it('locks the sidebar guide to the first agent row via a dynamic target', () => {
    const sidebar = guideById('sidebar-gestures');
    expect(sidebar.steps).toHaveLength(2);
    for (const step of sidebar.steps) {
      expect(typeof step.target).toBe('function');
    }
    expect(sidebar.steps[0].demo).toBe('swipe-right');
    expect(sidebar.steps[1].demo).toBe('drag-vertical');
    expect(sidebar.trigger?.requires).toBeUndefined();
    expect(sidebar.trigger?.predicates?.map((p) => p.name)).toEqual([
      'left-sidebar-visible',
      'agents-count-ge-2',
      'first-agent-row-mounted',
    ]);
  });

  it('keeps the trigger specs from the approved research (03 文档)', () => {
    const theme = guideById('theme-longpress');
    expect(theme.trigger?.requires).toEqual(['sidebar-gestures']);
    expect(theme.trigger?.predicates?.map((p) => p.name)).toEqual([
      'topic-loaded',
      'non-system-messages-ge-4',
      'title-not-default',
      'drawers-closed',
    ]);
    expect(theme.steps[0].demo).toBe('press-hold');
    expect(theme.steps[0].demoHint).toEqual(['气泡', '统一', '杂志']);
    expect(theme.steps[1].demo).toBeUndefined();

    const plus = guideById('plus-longpress');
    expect(plus.trigger?.requires).toBeUndefined();
    expect(plus.trigger?.predicates?.map((p) => p.name)).toEqual(['input-unlocked', 'drawers-closed']);

    const diary = guideById('diary-longpress');
    expect(diary.trigger?.requires).toBeUndefined();
    expect(diary.trigger?.predicates?.map((p) => p.name)).toEqual([
      'diary-center-open',
      'displayed-notes-ge-1',
    ]);
  });

  it('keeps every guide at 1–2 steps with the two-button model reachable', () => {
    for (const guide of allGuides()) {
      expect(guide.steps.length, `${guide.id} step count`).toBeGreaterThanOrEqual(1);
      expect(guide.steps.length, `${guide.id} step count`).toBeLessThanOrEqual(2);
      for (const step of guide.steps) {
        expect(step.title.length).toBeGreaterThan(0);
        expect(step.content.length).toBeGreaterThan(0);
      }
    }
  });

  it('registers exactly the four approved guides with no introducedIn shipped', () => {
    expect(allGuides().map((g) => g.id).sort()).toEqual([
      'diary-longpress',
      'plus-longpress',
      'sidebar-gestures',
      'theme-longpress',
    ]);
    for (const guide of allGuides()) {
      expect(guide.introducedIn).toBeUndefined();
    }
  });
});
