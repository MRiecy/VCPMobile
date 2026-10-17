import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { effectScope, ref, type EffectScope } from 'vue';
import { flushPromises } from '@vue/test-utils';
import { useModalHistory } from '@/core/composables/useModalHistory';
import { SETTINGS_SUBPAGE_MODAL_ID, useSettingsNavigation } from '@/features/settings/useSettingsNavigation';

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((accept, fail) => { resolve = accept; reject = fail; });
  return { promise, resolve, reject };
}

describe('settings page navigation with the real modal history', () => {
  let scope: EffectScope;
  const modals = useModalHistory();
  const isOpen = ref(true);
  let nav: ReturnType<typeof useSettingsNavigation>;
  const close = vi.fn();

  beforeEach(() => {
    window.history.replaceState({ vcpRoot: true, vcpMain: true }, '', '#/chat');
    // Deliver internal history.back synchronously so each test observes a settled
    // browser event, while running the real ModalHistory close/popstate logic.
    vi.spyOn(window.history, 'back').mockImplementation(() => {
      window.history.replaceState({ vcpRoot: true, vcpMain: true }, '');
      window.dispatchEvent(new PopStateEvent('popstate', { state: window.history.state }));
    });
    isOpen.value = true;
    scope = effectScope();
    scope.run(() => {
      nav = useSettingsNavigation({ isOpen, surface: () => null, close });
    });
    expect(modals.modalStackLength()).toBe(0);
  });

  afterEach(() => {
    modals.unregisterModal('settings-test-child');
    scope.stop();
  });

  it('waits once for repeated hardware back, keeps a failed edit open, and retries', async () => {
    const first = deferred<boolean>();
    const beforeLeave = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValueOnce(true);
    nav.openPage('connection');
    const visit = nav.activeVisit.value!;
    visit.setHandle({ beforeLeave });

    modals.closeTopModal();
    modals.closeTopModal();
    await Promise.resolve();
    expect(beforeLeave).toHaveBeenCalledTimes(1);
    expect(modals.modalStackLength()).toBe(1);
    expect(nav.activeVisit.value).toBe(visit);

    first.resolve(false);
    await flushPromises();
    expect(nav.activeVisit.value).toBe(visit);
    expect(nav.leaving.value).toBe(false);
    expect(modals.modalStackLength()).toBe(1);

    modals.closeTopModal();
    await flushPromises();
    expect(beforeLeave).toHaveBeenCalledTimes(2);
    expect(nav.activeVisit.value).toBeNull();
    expect(nav.displayedVisit.value).toBe(visit);
    expect(modals.modalStackLength()).toBe(0);
    nav.finishPageLeave(visit.serial);
    expect(nav.displayedVisit.value).toBeNull();
    expect(close).not.toHaveBeenCalled();
  });

  it('restores the browser entry while an asynchronous leave is pending', async () => {
    const saving = deferred<boolean>();
    nav.openPage('identity');
    nav.activeVisit.value!.setHandle({ beforeLeave: () => saving.promise });

    window.history.replaceState({ vcpRoot: true, vcpMain: true }, '');
    window.dispatchEvent(new PopStateEvent('popstate', { state: window.history.state }));
    expect(window.history.state.vcpModalId).toBe(SETTINGS_SUBPAGE_MODAL_ID);
    expect(modals.modalStackLength()).toBe(1);

    saving.resolve(true);
    await flushPromises();
    expect(modals.modalStackLength()).toBe(0);
    expect(window.history.state.vcpModalId).toBeUndefined();
  });

  it('does not restore a phantom browser entry for an immediate preference page', () => {
    nav.openPage('rendering');
    nav.activeVisit.value!.setHandle({});
    window.history.replaceState({ vcpRoot: true, vcpMain: true }, '');
    window.dispatchEvent(new PopStateEvent('popstate', { state: window.history.state }));
    expect(nav.activeVisit.value).toBeNull();
    expect(modals.modalStackLength()).toBe(0);
    expect(window.history.state.vcpModalId).toBeUndefined();
  });

  it('invalidates pending saves and transition callbacks when hidden and reopened', async () => {
    const saving = deferred<boolean>();
    nav.openPage('connection');
    const previous = nav.activeVisit.value!;
    previous.setHandle({ beforeLeave: () => saving.promise });
    nav.requestLeave();
    await Promise.resolve();
    isOpen.value = false;
    expect(nav.activeVisit.value).toBeNull();
    expect(nav.displayedVisit.value).toBe(previous);
    expect(modals.modalStackLength()).toBe(0);

    isOpen.value = true;
    nav.openPage('theme');
    const current = nav.activeVisit.value!;
    saving.resolve(true);
    await flushPromises();
    nav.finishPageLeave(previous.serial);
    expect(nav.activeVisit.value).toBe(current);
    expect(nav.displayedVisit.value).toBe(current);
    expect(modals.modalStackLength()).toBe(1);
  });

  it('cancels an unresolved page without letting its late ref own the next visit', () => {
    nav.openPage('connection');
    const previous = nav.activeVisit.value!;
    expect(nav.loading.value).toBe(true);
    expect(nav.requestLeave()).toBe(true);
    nav.openPage('rendering');
    const beforeLeave = vi.fn();
    previous.setHandle({ beforeLeave });
    expect(nav.requestLeave()).toBe(true);
    expect(beforeLeave).not.toHaveBeenCalled();
    expect(modals.modalStackLength()).toBe(0);
  });

  it('keeps an exposed page instance when its leave hook rejects', async () => {
    const beforeLeave = vi.fn().mockRejectedValueOnce(new Error('disk full')).mockResolvedValueOnce(true);
    nav.openPage('connection');
    const visit = nav.activeVisit.value!;
    visit.setHandle({ beforeLeave });
    expect(nav.requestLeave()).toBe(false);
    await flushPromises();
    expect(nav.navigationError.value).toContain('disk full');
    expect(nav.pageError.value).toBe('');
    expect(nav.activeVisit.value).toBe(visit);
    expect(nav.requestLeave()).toBe(false);
    await flushPromises();
    expect(beforeLeave).toHaveBeenCalledTimes(2);
    expect(nav.activeVisit.value).toBeNull();
  });

  it('lets an inner selector consume back before attempting the page save', async () => {
    const beforeLeave = vi.fn().mockResolvedValue(true);
    const closeSelector = vi.fn();
    nav.openPage('advanced');
    nav.activeVisit.value!.setHandle({ beforeLeave });
    modals.registerModal('settings-test-child', closeSelector);
    modals.closeTopModal();
    await flushPromises();
    expect(closeSelector).toHaveBeenCalledOnce();
    expect(beforeLeave).not.toHaveBeenCalled();
    expect(nav.activeVisit.value?.id).toBe('advanced');
    expect(modals.modalStackLength()).toBe(1);
  });
});
