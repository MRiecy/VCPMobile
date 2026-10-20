import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { defineAsyncComponent, defineComponent, h, onUnmounted, watch, type Component } from 'vue';
import { createPinia, setActivePinia } from 'pinia';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import SettingsView from '@/features/settings/SettingsView.vue';
import { settingsPages } from '@/features/settings/catalog';
import type { SettingsPageEntry } from '@/features/settings/contracts';
import { useModalHistory } from '@/core/composables/useModalHistory';
import { useDeveloperSettingsStore } from '@/features/settings/developer/store';
import { useSettingsStore } from '@/core/stores/settings';
import { makeConfigSnapshot, makeSettings } from '../../helpers/config';
import { mockInvoke } from '../../mocks/tauri';

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

  const About = defineComponent({
    props: { active: Boolean },
    emits: ['back', 'open-developer'],
    setup(props, { emit }) {
      return () => h('div', { 'data-testid': 'about-body' }, [
        h('button', { disabled: !props.active, onClick: () => emit('open-developer') }, '开发者入口'),
        h('button', { disabled: !props.active, onClick: () => emit('back') }, '关于返回'),
      ]);
    },
  });
  const Developer = defineComponent({
    props: { active: Boolean },
    setup: (props) => () => h('div', { 'data-testid': 'developer-body', 'data-active': props.active }, '诊断工具'),
  });
  function developerEntries(loader: () => Promise<Component> = async () => Developer): SettingsPageEntry[] {
    return [
      { id: 'about', title: '关于', description: '版本与更新', frame: 'immersive', component: defineAsyncComponent(async () => About) },
      { id: 'developer', title: '开发者选项', description: '调试工具', frame: 'list', parent: 'about', component: defineAsyncComponent(loader) },
    ];
  }

  function catalogEntry(loader: () => Promise<Component>, frame: SettingsPageEntry['frame'] = 'list'): SettingsPageEntry {
    return {
      id: frame === 'immersive' ? 'about' : 'connection',
      title: frame === 'immersive' ? '关于' : '服务器连接',
      description: '测试页面', frame,
      component: defineAsyncComponent(loader),
    };
  }

  function mountShell(...entries: SettingsPageEntry[]) {
    (settingsPages as SettingsPageEntry[]).splice(0, settingsPages.length, ...entries);
    wrapper = mount(SettingsView, {
      props: { isOpen: true },
      attachTo: document.body,
      global: { stubs: { FluidBackground: Background } },
    });
    return wrapper;
  }

  beforeEach(() => {
    localStorage.clear();
    setActivePinia(createPinia());
    wrapper = null;
    activeChanges.length = 0;
    beforeLeave.mockResolvedValue(true);
    window.history.replaceState({ vcpRoot: true, vcpMain: true }, '', '#/chat');
    vi.spyOn(window.history, 'back').mockImplementation(() => {
      window.history.replaceState({ vcpRoot: true, vcpMain: true }, '');
      window.dispatchEvent(new PopStateEvent('popstate', { state: window.history.state }));
    });
  });

  afterEach(() => { wrapper?.unmount(); localStorage.clear(); });

  it('offers a home reload after a failed read and clears the error after success', async () => {
    mockInvoke('read_settings', () => { throw new Error('storage unavailable'); });
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    await expect(useSettingsStore().fetchSettings()).rejects.toThrow('storage unavailable');
    const shell = mountShell();
    expect(shell.text()).toContain('storage unavailable');
    mockInvoke('read_settings', () => makeConfigSnapshot(makeSettings({ 'server.adminUsername': 'admin' })));
    await shell.findAll('button').find(button => button.text() === '重新加载')!.trigger('click');
    await flushPromises();
    expect(shell.text()).not.toContain('storage unavailable');
    expect(useSettingsStore().settings?.server.adminUsername).toBe('admin');
    expect(shell.find('[role="alert"]').exists()).toBe(false);
  });

  it('keeps developer tools out of the root list and enters them only from an unlocked about visit', async () => {
    const developer = useDeveloperSettingsStore();
    const shell = mountShell(...developerEntries());
    expect(shell.findAll('.settings-row')).toHaveLength(1);
    expect(shell.get('.settings-row').text()).toContain('关于');
    await shell.get('.settings-row').trigger('click');
    await flushPromises();
    await shell.get('[data-testid="about-body"] button').trigger('click');
    await flushPromises();
    expect(shell.find('[data-testid="developer-body"]').exists()).toBe(false);
    expect(developer.setEnabled(true).ok).toBe(true);
    await shell.get('[data-testid="about-body"] button').trigger('click');
    await flushPromises();
    expect(shell.get('[data-testid="developer-body"]').attributes('data-active')).toBe('true');
    expect(shell.findAll('.settings-row')).toHaveLength(1);
    expect(useModalHistory().modalStackLength()).toBe(1);
    await shell.get('button[aria-label="返回关于"]').trigger('click');
    await flushPromises();
    expect(shell.find('[data-testid="about-body"]').exists()).toBe(true);
    expect(useModalHistory().modalStackLength()).toBe(1);
    // The previous About surface can remain mounted during its leave transition.
    await vi.waitFor(() => expect(shell.findAll('[data-testid="about-body"]')).toHaveLength(1));
    await shell.findAll('[data-testid="about-body"] button:not(:disabled)').find((button) => button.text() === '关于返回')!.trigger('click');
    await flushPromises();
    expect(useModalHistory().modalStackLength()).toBe(0);
  });

  it('ignores a late developer chunk after returning to about and then hiding settings', async () => {
    let resolve!: (component: Component) => void;
    useDeveloperSettingsStore().setEnabled(true);
    const shell = mountShell(...developerEntries(() => new Promise((accept) => { resolve = accept; })));
    await shell.get('.settings-row').trigger('click');
    await flushPromises();
    await shell.get('[data-testid="about-body"] button').trigger('click');
    expect(shell.find('button[aria-label="返回关于"]').exists()).toBe(true);
    await shell.get('button[aria-label="返回关于"]').trigger('click');
    resolve(Developer);
    await flushPromises();
    expect(shell.find('[data-testid="developer-body"]').exists()).toBe(false);
    expect(shell.find('[data-testid="about-body"]').exists()).toBe(true);
    await shell.setProps({ isOpen: false });
    expect(useModalHistory().modalStackLength()).toBe(0);
    expect(setFluidActive).toHaveBeenLastCalledWith(false);
    await shell.setProps({ isOpen: true });
    expect(shell.find('[data-testid="developer-body"]').exists()).toBe(false);
    await vi.waitFor(() => expect(shell.find('[data-testid="about-body"]').exists()).toBe(false));
  });

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
