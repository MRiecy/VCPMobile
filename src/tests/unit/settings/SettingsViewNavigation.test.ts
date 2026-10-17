import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { defineAsyncComponent, defineComponent, h, onUnmounted, watch } from 'vue';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import SettingsView from '@/features/settings/SettingsView.vue';
import { settingsPages } from '@/features/settings/catalog';
import type { SettingsPageEntry } from '@/features/settings/contracts';
import { useModalHistory } from '@/core/composables/useModalHistory';

vi.mock('@/features/settings/catalog', () => ({ settingsPages: [] }));
vi.mock('@/core/stores/theme', () => ({
  useThemeStore: () => ({ availableThemes: [{}], fetchThemes: vi.fn() }),
}));

describe('SettingsView page loading and retained transitions', () => {
  let wrapper: VueWrapper | null;
  const activeChanges: boolean[] = [];
  const unmounted = vi.fn();
  const beforeLeave = vi.fn();
  const setFluidActive = vi.fn();

  const Page = defineComponent({
    props: { active: Boolean },
    emits: ['back'],
    setup(props, { expose, emit }) {
      expose({ beforeLeave });
      watch(() => props.active, (active) => activeChanges.push(active), { immediate: true });
      onUnmounted(unmounted);
      return () => h('div', { 'data-testid': 'page-body' }, [
        h('input', { 'aria-label': '端点', value: 'draft remains' }),
        h('button', { onClick: () => emit('back') }, '页面返回'),
      ]);
    },
  });

  const Background = defineComponent({
    setup(_, { expose }) {
      expose({ setActive: setFluidActive });
      return () => h('div', { 'data-testid': 'fluid-host' });
    },
  });

  function catalogEntry(loader: () => Promise<typeof Page>, frame: SettingsPageEntry['frame'] = 'list'): SettingsPageEntry {
    return {
      id: frame === 'immersive' ? 'about' : 'connection',
      title: frame === 'immersive' ? '关于' : '服务器连接',
      description: '测试页面', frame,
      component: defineAsyncComponent(loader),
    };
  }

  function mountShell(entry: SettingsPageEntry) {
    (settingsPages as SettingsPageEntry[]).splice(0, settingsPages.length, entry);
    wrapper = mount(SettingsView, {
      props: { isOpen: true },
      attachTo: document.body,
      global: { stubs: { FluidBackground: Background } },
    });
    return wrapper;
  }

  beforeEach(() => {
    wrapper = null;
    activeChanges.length = 0;
    beforeLeave.mockResolvedValue(true);
    window.history.replaceState({ vcpRoot: true, vcpMain: true }, '', '#/chat');
    vi.spyOn(window.history, 'back').mockImplementation(() => {
      window.history.replaceState({ vcpRoot: true, vcpMain: true }, '');
      window.dispatchEvent(new PopStateEvent('popstate', { state: window.history.state }));
    });
  });

  afterEach(() => wrapper?.unmount());

  it('retries a failed lazy chunk and then uses the loaded page leave hook', async () => {
    const loader = vi.fn().mockRejectedValueOnce(new Error('chunk unavailable')).mockResolvedValue(Page);
    const shell = mountShell(catalogEntry(loader));
    await shell.get('.settings-row').trigger('click');
    await flushPromises();
    expect(shell.text()).toContain('chunk unavailable');
    expect(shell.find('[data-testid="page-body"]').exists()).toBe(false);
    await shell.findAll('button').find((button) => button.text() === '重试')!.trigger('click');
    await flushPromises();
    expect(loader).toHaveBeenCalledTimes(2);
    expect(shell.find('[data-testid="page-body"]').exists()).toBe(true);
    await shell.get('button[aria-label="返回设置首页"]').trigger('click');
    await flushPromises();
    expect(beforeLeave).toHaveBeenCalledOnce();
    expect(activeChanges).toEqual([true, false]);
    await vi.waitFor(() => expect(unmounted).toHaveBeenCalledOnce());
    expect(useModalHistory().modalStackLength()).toBe(0);
  });

  it('deactivates before the outer leave finishes, without starting a save', async () => {
    const shell = mountShell(catalogEntry(async () => Page));
    await shell.get('.settings-row').trigger('click');
    await flushPromises();
    await shell.setProps({ isOpen: false });
    expect(activeChanges).toEqual([true, false]);
    expect(beforeLeave).not.toHaveBeenCalled();
    expect(unmounted).not.toHaveBeenCalled();
    expect(useModalHistory().modalStackLength()).toBe(0);
    await vi.waitFor(() => expect(unmounted).toHaveBeenCalledOnce());
    expect(shell.find('[data-testid="page-body"]').exists()).toBe(false);
  });

  it('can leave an unresolved chunk and does not mount it when it arrives later', async () => {
    let resolve!: (component: typeof Page) => void;
    const shell = mountShell(catalogEntry(() => new Promise((accept) => { resolve = accept; })));
    await shell.get('.settings-row').trigger('click');
    expect(shell.text()).toContain('正在加载设置');
    await shell.get('button[aria-label="返回设置首页"]').trigger('click');
    // Resolve during the retained exit picture, before after-leave can unmount it.
    resolve(Page);
    await flushPromises();
    expect(activeChanges).toEqual([]);
    expect(shell.find('[data-testid="page-body"]').exists()).toBe(false);
    await vi.waitFor(() => expect(shell.find('[data-settings-visit]').exists()).toBe(false));
    expect(useModalHistory().modalStackLength()).toBe(0);
  });

  it('retains the about background host while stopping its work on hide', async () => {
    const shell = mountShell(catalogEntry(async () => Page, 'immersive'));
    const background = shell.get('[data-testid="fluid-host"]').element;
    await shell.get('.settings-row').trigger('click');
    await flushPromises();
    expect(setFluidActive).toHaveBeenLastCalledWith(true);
    await shell.setProps({ isOpen: false });
    expect(setFluidActive).toHaveBeenLastCalledWith(false);
    await vi.waitFor(() => expect(unmounted).toHaveBeenCalledOnce());
    expect(shell.get('[data-testid="fluid-host"]').element).toBe(background);
  });
});
