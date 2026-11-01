import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import { ref } from 'vue';
import PermissionGate from '../../../components/layout/PermissionGate.vue';
import { invokeMock, mockInvoke } from '../../mocks/tauri';

vi.mock('/vcpmobile.svg', () => ({ default: '/vcpmobile.svg' }));
const native = vi.hoisted(() => ({ android: false, ios: false }));
const bootstrap = vi.hoisted(() => vi.fn(async (_force: boolean) => {}));
const continueFromPermissionGuide = vi.hoisted(() => vi.fn(async () => {}));
const phase = ref('active');
vi.mock('../../../core/utils/platform', () => ({
  get isAndroid() { return native.android; },
  get isIOS() { return native.ios; },
}));
vi.mock('../../../core/stores/appLifecycle', () => ({
  useAppLifecycleStore: () => ({ bootstrap, continueFromPermissionGuide, get lifecycleSnapshot() { return { phase: phase.value }; } }),
}));

describe('shared permission guide', () => {
  let wrapper: VueWrapper | undefined;
  const currentButtons = () => wrapper!.findAll('button').filter(button => !button.element.closest('[inert]'));
  const settingButtons = () => currentButtons().filter(button => /^去(授权|设置)$/.test(button.text()));
  const next = () => currentButtons().find(button => button.text() === '下一步' && !button.element.disabled);

  beforeEach(() => {
    vi.useFakeTimers();
    native.android = false;
    native.ios = false;
    phase.value = 'active';
    bootstrap.mockClear();
    continueFromPermissionGuide.mockClear();
  });

  afterEach(() => {
    wrapper?.unmount();
    wrapper = undefined;
    vi.useRealTimers();
  });

  it('opens each Android setting but keeps denied permissions blocking when the user returns without changes', async () => {
    native.android = true;
    let basicGranted = false;
    mockInvoke('plugin:vcp-mobile|check_all_permissions', () => ({
      notification: basicGranted, battery: false, backgroundAllowed: basicGranted, notificationListener: false,
    }));
    mockInvoke('plugin:vcp-mobile|request_power_management_permission', () => true);
    mockInvoke('plugin:vcp-mobile|get_free_disk_space', () => ({ freeGb: 2, totalGb: 64 }));
    wrapper = mount(PermissionGate);
    await flushPromises();

    expect(settingButtons()).toHaveLength(2);
    for (const button of settingButtons()) {
      await button.trigger('click');
      document.dispatchEvent(new Event('visibilitychange'));
      await flushPromises();
    }
    expect(next()).toBeUndefined();
    basicGranted = true;
    document.dispatchEvent(new Event('visibilitychange'));
    await flushPromises();
    await next()!.trigger('click');
    expect(settingButtons()).toHaveLength(2);
    for (const button of settingButtons()) {
      await button.trigger('click');
      document.dispatchEvent(new Event('visibilitychange'));
      await flushPromises();
    }
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|request_android_permission', { pType: 'notificationSettings' });
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|request_android_permission', { pType: 'battery' });
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|request_power_management_permission');
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|request_notification_listener_permission');
    expect(wrapper.find('input[type="checkbox"]').exists()).toBe(false);
    expect(settingButtons()).toHaveLength(2);
    expect(next()).toBeUndefined();
    expect(bootstrap).not.toHaveBeenCalled();
  });

  it('hides an undetectable item, advances on a real grant and returns to the permission page on revocation', async () => {
    native.android = true;
    let notificationListener = false;
    const snapshot = () => ({
      notification: true, battery: null, backgroundAllowed: true, notificationListener,
    });
    mockInvoke('plugin:vcp-mobile|check_all_permissions', snapshot);
    let freeGb = 2;
    mockInvoke('plugin:vcp-mobile|get_free_disk_space', () => ({ freeGb, totalGb: 64 }));
    wrapper = mount(PermissionGate);
    await flushPromises();
    expect(wrapper.text()).not.toContain('忽略电池优化');
    await next()!.trigger('click');
    expect(settingButtons()).toHaveLength(1);
    await settingButtons()[0].trigger('click');
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|request_notification_listener_permission');
    expect(next()).toBeUndefined();

    notificationListener = true;
    document.dispatchEvent(new Event('visibilitychange'));
    await flushPromises();
    await next()!.trigger('click');
    expect(currentButtons().some(button => button.text().includes('存储空间不足') && button.element.disabled)).toBe(true);

    notificationListener = false;
    window.dispatchEvent(new CustomEvent('vcp-permission-change', { detail: snapshot() }));
    await flushPromises();
    expect(settingButtons()).toHaveLength(1);
    expect(next()).toBeUndefined();
    notificationListener = true;
    freeGb = 12;
    document.dispatchEvent(new Event('visibilitychange'));
    await flushPromises();
    await next()!.trigger('click');
    const enter = wrapper.findAll('button').find(button => button.text() === '激活并进入应用' && !button.element.disabled)!;
    await enter.trigger('click');
    expect(continueFromPermissionGuide).toHaveBeenCalledOnce();
  });

  it('skips an empty permission page and ignores an older native read that arrives after returning', async () => {
    native.android = true;
    const unavailable = { notification: null, battery: null, backgroundAllowed: null, notificationListener: null };
    let finishFirstRead!: () => void;
    mockInvoke('plugin:vcp-mobile|check_all_permissions', () => new Promise(resolve => {
      finishFirstRead = () => resolve({ ...unavailable, notification: false });
    }));
    mockInvoke('plugin:vcp-mobile|get_free_disk_space', () => ({ freeGb: 12, totalGb: 64 }));
    wrapper = mount(PermissionGate);
    await flushPromises();
    expect(invokeMock).not.toHaveBeenCalledWith('plugin:vcp-mobile|get_free_disk_space');
    mockInvoke('plugin:vcp-mobile|check_all_permissions', () => unavailable);
    document.dispatchEvent(new Event('visibilitychange'));
    await flushPromises();
    finishFirstRead();
    await flushPromises();
    expect(wrapper.findAll('button').some(button => /^去(授权|设置)$/.test(button.text()))).toBe(false);
    const enter = wrapper.findAll('button').find(button => button.text() === '激活并进入应用')!;
    expect(enter.element.closest('[inert]')).toBeNull();
    await enter.trigger('click');
    expect(continueFromPermissionGuide).toHaveBeenCalledOnce();
  });

  it('requires iOS notifications, reports refusal and only offers continuation after authorization', async () => {
    native.ios = true;
    mockInvoke('plugin:vcp-mobile|get_notification_permission', () => ({ authorization: 'notDetermined' }));
    mockInvoke('plugin:vcp-mobile|request_notification_permission', () => ({ authorization: 'denied' }));
    wrapper = mount(PermissionGate);
    await flushPromises();
    expect(wrapper.text()).not.toContain('品牌厂商后台管理');
    expect(wrapper.text()).not.toContain('5.0 GB');
    const request = wrapper.findAll('button').find(button => button.text() === '开启通知')!;
    await request.trigger('click');
    await flushPromises();
    expect(wrapper.text()).toContain('系统通知已关闭');
    expect(wrapper.findAll('button').some(button => button.text() === '系统设置')).toBe(true);
    expect(currentButtons().some(button => button.text().includes('继续进入应用'))).toBe(false);
    expect(continueFromPermissionGuide).not.toHaveBeenCalled();
    mockInvoke('plugin:vcp-mobile|get_notification_permission', () => ({ authorization: 'authorized' }));
    document.dispatchEvent(new Event('visibilitychange'));
    await flushPromises();
    await currentButtons().find(button => button.text() === '继续进入应用')!.trigger('click');
    expect(continueFromPermissionGuide).toHaveBeenCalledOnce();
    expect(invokeMock.mock.calls.some(([command]) => command.includes('android_permission') || command.includes('disk_space'))).toBe(false);
  });

  it('refreshes after returning from Settings even if an earlier native read is still pending', async () => {
    native.ios = true;
    let resolveInitial!: (value: { authorization: 'denied' }) => void;
    let reads = 0;
    mockInvoke('plugin:vcp-mobile|get_notification_permission', () => {
      reads++;
      return reads === 1 ? new Promise(resolve => { resolveInitial = resolve; }) : { authorization: 'authorized' };
    });
    wrapper = mount(PermissionGate);
    await flushPromises();
    phase.value = 'background';
    await flushPromises();
    phase.value = 'active';
    document.dispatchEvent(new Event('visibilitychange'));
    await flushPromises();
    expect(reads).toBe(1);
    resolveInitial({ authorization: 'denied' });
    await flushPromises();
    expect(reads).toBe(2);
    expect(wrapper.text()).toContain('已允许通知');
  });

  it('does not treat opening Settings as a grant and reads the real state on return', async () => {
    native.ios = true;
    mockInvoke('plugin:vcp-mobile|get_notification_permission', () => ({ authorization: 'denied' }));
    mockInvoke('plugin:vcp-mobile|open_notification_settings', () => undefined);
    wrapper = mount(PermissionGate);
    await flushPromises();
    await wrapper.findAll('button').find(button => button.text() === '系统设置')!.trigger('click');
    await flushPromises();
    expect(wrapper.text()).toContain('系统通知已关闭');
    mockInvoke('plugin:vcp-mobile|get_notification_permission', () => ({ authorization: 'authorized' }));
    phase.value = 'background';
    await flushPromises();
    phase.value = 'active';
    await flushPromises();
    expect(wrapper.text()).toContain('已允许通知');
    expect(invokeMock).not.toHaveBeenCalledWith('plugin:vcp-mobile|request_notification_permission');
  });

  it('does not perform a queued permission refresh or retain listeners after unmount', async () => {
    native.ios = true;
    let resolveInitial!: (value: { authorization: 'denied' }) => void;
    mockInvoke('plugin:vcp-mobile|get_notification_permission', () => new Promise(resolve => { resolveInitial = resolve; }));
    wrapper = mount(PermissionGate);
    document.dispatchEvent(new Event('visibilitychange'));
    wrapper.unmount();
    wrapper = undefined;
    resolveInitial({ authorization: 'denied' });
    await flushPromises();
    document.dispatchEvent(new Event('visibilitychange'));
    await flushPromises();
    expect(invokeMock.mock.calls.filter(([command]) => command === 'plugin:vcp-mobile|get_notification_permission')).toHaveLength(1);
  });
});
