import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { nextTick, defineComponent } from 'vue';
import { mount } from '@vue/test-utils';
import SlidePage from '../../components/ui/SlidePage.vue';

describe('modal focus ownership', () => {
  let history: typeof import('../../core/composables/useModalHistory');
  beforeEach(async () => {
    vi.resetModules();
    window.history.replaceState({}, '', '#/');
    history = await import('../../core/composables/useModalHistory');
    vi.spyOn(window.history, 'back').mockImplementation(() => {});
  });
  afterEach(() => { history.cleanupModalHistory(); });

  it('keeps Tab in the top surface and restores its connected opener once', async () => {
    const opener = document.createElement('button');
    const surface = document.createElement('div');
    const first = document.createElement('input');
    const last = document.createElement('button');
    surface.append(first, last);
    document.body.append(opener, surface);
    opener.focus();
    const modal = history.useModalHistory();
    const close = vi.fn(() => { surface.remove(); });
    modal.registerModal('test', close, () => surface);
    // A component binds its view to the Store's existing session, not a second entry.
    modal.registerModal('test', vi.fn(), () => surface);
    expect(modal.modalStackLength()).toBe(1);
    last.focus();
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', cancelable: true }));
    expect(document.activeElement).toBe(first);
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', cancelable: true }));
    await nextTick();
    expect(close).toHaveBeenCalledTimes(1);
    expect(modal.modalStackLength()).toBe(0);
    expect(document.activeElement).toBe(opener);
  });

  it('leaves newly selected focus alone and respects a running operation', async () => {
    const opener = document.createElement('button');
    const surface = document.createElement('div');
    const input = document.createElement('input');
    const next = document.createElement('button');
    surface.append(input);
    document.body.append(opener, surface, next);
    opener.focus();
    const modal = history.useModalHistory();
    let saving = true;
    const close = vi.fn();
    modal.registerModal('saving', close, () => surface, () => !saving);
    input.focus();
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', cancelable: true }));
    expect(close).not.toHaveBeenCalled();
    expect(modal.modalStackLength()).toBe(1);
    saving = false;
    modal.unregisterModal('saving');
    next.focus();
    await nextTick();
    expect(document.activeElement).toBe(next);
  });

  it('checks notification navigation without closing the drawer or starting a settings save', () => {
    const modal = history.useModalHistory();
    const saveBeforeClosing = vi.fn(() => false);
    const closeDrawer = vi.fn();
    modal.registerModal('settings', saveBeforeClosing, undefined, saveBeforeClosing);
    modal.registerModal('drawer', closeDrawer);
    expect(modal.canCloseAllModals()).toBe(false);
    expect(saveBeforeClosing).not.toHaveBeenCalled();
    expect(closeDrawer).not.toHaveBeenCalled();
    expect(modal.modalStackLength()).toBe(2);
  });

  it('binds a Store-owned page identity through its feature and SlidePage boundary', async () => {
    const Page = defineComponent({ components: { SlidePage },
      template: '<SlidePage :is-open="true"><button class="first">One</button><button class="last">Two</button></SlidePage>',
    });
    const wrapper = mount(Page, { attrs: { 'data-vcp-modal-id': 'Page:settings:' }, attachTo: document.body });
    expect(wrapper.get('.vcp-viewport-surface').attributes('data-vcp-modal-id')).toBe('Page:settings:');
    const modal = history.useModalHistory();
    modal.registerModal('Page:settings:', () => {});
    (wrapper.get('.last').element as HTMLElement).focus();
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', cancelable: true }));
    expect(document.activeElement).toBe(wrapper.get('.first').element);
    modal.unregisterModal('Page:settings:');
    wrapper.unmount();
    await nextTick();
  });
});
