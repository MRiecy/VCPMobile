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

  it('keeps Android permission requests and its existing storage check in the shared guide', async () => {
    native.android = true;
    mockInvoke('plugin:vcp-mobile|check_all_permissions', () => ({
      notification: false, storage: false, battery: false,
      microphone: false, camera: false, location: false,
    }));
    mockInvoke('plugin:vcp-mobile|check_notification_listener_permission', () => ({ enabled: false }));
    mockInvoke('plugin:vcp-mobile|get_free_disk_space', () => ({ freeGb: 2, totalGb: 64 }));
    mockInvoke('plugin:vcp-mobile|request_android_permission', () => undefined);
    wrapper = mount(PermissionGate);
    await flushPromises();

    const requests = wrapper.findAll('button').filter(button => button.text() === '去授权');
    expect(requests).toHaveLength(2);
    await requests[0].trigger('click');
    await requests[1].trigger('click');
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|request_android_permission', { pType: 'notification' });
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|request_android_permission', { pType: 'battery' });
    expect(wrapper.find('.vcp-permission-gate').exists()).toBe(true);
    expect(wrapper.findAll('button').some(button => button.text().includes('存储空间不足') && button.attributes('disabled') !== undefined)).toBe(true);
    expect(bootstrap).not.toHaveBeenCalled();
  });

  it('offers one optional iOS notification item, reports refusal and still lets the user continue', async () => {
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
    await wrapper.findAll('button').find(button => button.text().includes('继续进入应用'))!.trigger('click');
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
