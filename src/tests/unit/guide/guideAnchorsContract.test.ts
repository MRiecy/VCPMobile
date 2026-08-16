import { describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import chatViewSource from '@/features/chat/ChatView.vue?raw';
import inputEnhancerSource from '@/features/chat/InputEnhancer.vue?raw';
import agentListSource from '@/features/agent/AgentList.vue?raw';
import diaryNoteListSource from '@/features/diary/components/DiaryNoteList.vue?raw';
import contextMenuSheetSource from '@/components/ui/ContextMenuSheet.vue?raw';
import tarvenSelectorSource from '@/features/chat/components/TarvenSelector.vue?raw';
import { useOverlayStore } from '@/core/stores/overlay';
import { useLayoutStore } from '@/core/stores/layout';
import { sidebarTab } from '@/features/agent/sidebarTab';
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
    expect(inputEnhancerSource).toContain(`v-guide="'chat-attach-menu'"`);
    expect(agentListSource).toContain(`v-guide="'agent-row-' + agent.id"`);
    expect(agentListSource).toContain(`v-guide="'agent-row-settings-' + agent.id"`);
    expect(diaryNoteListSource).toContain(`v-guide="'diary-note-row'"`);
    expect(contextMenuSheetSource).toContain(`v-guide="'context-menu-sheet'"`);
    expect(tarvenSelectorSource).toContain(`v-guide="'tarven-selector'"`);

    const theme = guideById('theme-longpress');
    expect(theme.steps.map((s) => s.target)).toEqual([
      'chat-theme-button',
      'context-menu-sheet',
    ]);

    const plus = guideById('plus-longpress');
    expect(plus.steps.map((s) => s.target)).toEqual([
      'chat-plus-button',
      'chat-attach-menu',
      'chat-plus-button',
      'tarven-selector',
    ]);

    const diary = guideById('diary-longpress');
    expect(diary.steps.map((s) => s.target)).toEqual(['diary-note-row', 'diary-note-row']);
  });

  it('locks the sidebar guide to the first agent row via a dynamic target', () => {
    const sidebar = guideById('sidebar-gestures');
    expect(sidebar.steps).toHaveLength(3);
    for (const step of sidebar.steps) {
      expect(typeof step.target).toBe('function');
    }
    expect(sidebar.steps[0].demo).toBe('swipe-right');
    expect(sidebar.steps[1].demo).toBeUndefined();
    expect(sidebar.steps[2].demo).toBe('drag-vertical');
    // 真实业务：右滑步骤由 perform 驱动真实行滑开，undo 收尾
    expect(typeof sidebar.steps[0].perform).toBe('function');
    expect(typeof sidebar.steps[0].undo).toBe('function');
    expect(sidebar.trigger?.requires).toBeUndefined();
    expect(sidebar.trigger?.predicates?.map((p) => p.name)).toEqual([
      'workspace-not-occluded',
      'left-sidebar-visible',
      'agents-count-ge-2',
      'first-agent-row-mounted',
    ]);
  });

  it('keeps the trigger specs from the approved research (03 文档)', () => {
    const theme = guideById('theme-longpress');
    expect(theme.trigger?.requires).toEqual(['sidebar-gestures']);
    expect(theme.trigger?.predicates?.map((p) => p.name)).toEqual([
      'workspace-not-occluded',
      'topic-loaded',
      'non-system-messages-ge-4',
      'title-not-default',
      'drawers-closed',
    ]);
    expect(theme.steps[0].demo).toBe('press-hold');
    expect(theme.steps[0].perform).toBeDefined();
    expect(theme.steps[0].undo).toBeDefined();
    expect(theme.steps[1].demo).toBeUndefined();
    expect(theme.steps[1].undo).toBeUndefined(); // undo 与 perform 同步骤配对

    const plus = guideById('plus-longpress');
    expect(plus.trigger?.requires).toBeUndefined();
    expect(plus.trigger?.predicates?.map((p) => p.name)).toEqual([
      'workspace-not-occluded',
      'input-unlocked',
      'drawers-closed',
    ]);

    const diary = guideById('diary-longpress');
    expect(diary.trigger?.requires).toBeUndefined();
    expect(diary.trigger?.predicates?.map((p) => p.name)).toEqual([
      'diary-center-open',
      'displayed-notes-ge-1',
    ]);
  });

  it('hardens the diary guide for slow first-open (load + virtual rows + entrance)', () => {
    const diary = guideById('diary-longpress');
    for (const step of diary.steps) {
      expect(step.waitTimeoutMs).toBe(6000);
      expect(typeof step.waitFor).toBe('function');
    }
  });

  it('keeps perform on gesture steps, waitFor on result steps, and no perform on last steps', () => {
    const theme = guideById('theme-longpress');
    expect(theme.steps[0].perform).toBeDefined();
    expect(theme.steps[1].perform).toBeUndefined();
    expect(theme.steps[1].waitFor).toBeDefined();

    const plus = guideById('plus-longpress');
    expect(plus.steps[0].perform).toBeDefined();
    expect(plus.steps[1].perform).toBeUndefined();
    expect(plus.steps[1].waitFor).toBeDefined();
    expect(plus.steps[2].perform).toBeDefined();
    expect(plus.steps[3].perform).toBeUndefined();
    expect(plus.steps[3].waitFor).toBeDefined();
    expect(plus.steps[3].waitTimeoutMs).toBe(6000);

    const sidebar = guideById('sidebar-gestures');
    expect(sidebar.steps[0].perform).toBeDefined();
    expect(sidebar.steps[1].perform).toBeDefined(); // 收行，为拖动演示复位场景
    expect(sidebar.steps[2].perform).toBeUndefined();

    // 全局不变量：末步不配置 perform（「我知道了」无触发时机）
    for (const guide of allGuides()) {
      const last = guide.steps[guide.steps.length - 1];
      expect(last.perform, `${guide.id} last step`).toBeUndefined();
    }
  });

  it('prepares the right environment for manual replay', () => {
    setActivePinia(createPinia());
    const overlayStore = useOverlayStore();
    const layoutStore = useLayoutStore();

    // theme/plus：清页面栈 + 双关抽屉
    layoutStore.setRightDrawer(true);
    guideById('theme-longpress').prepare?.();
    expect(overlayStore.pageStack).toEqual([]);
    expect(layoutStore.leftDrawerOpen).toBe(false);
    expect(layoutStore.rightDrawerOpen).toBe(false);

    layoutStore.setRightDrawer(true);
    guideById('plus-longpress').prepare?.();
    expect(overlayStore.pageStack).toEqual([]);
    expect(layoutStore.rightDrawerOpen).toBe(false);

    // sidebar：清页面栈 + 切助理 Tab + 开左抽屉
    sidebarTab.value = 'topics';
    guideById('sidebar-gestures').prepare?.();
    expect(sidebarTab.value).toBe('agents');
    expect(layoutStore.leftDrawerOpen).toBe(true);
    layoutStore.setLeftDrawer(false);

    // diary：清页面栈 + 打开日记中心
    guideById('diary-longpress').prepare?.();
    expect(overlayStore.pageStack.map((p) => p.type)).toEqual(['diaryCenter']);
  });

  it('keeps every guide at 1–4 steps with the two-button model reachable', () => {
    for (const guide of allGuides()) {
      expect(guide.steps.length, `${guide.id} step count`).toBeGreaterThanOrEqual(1);
      expect(guide.steps.length, `${guide.id} step count`).toBeLessThanOrEqual(4);
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
