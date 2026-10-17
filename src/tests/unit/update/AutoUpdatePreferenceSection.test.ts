import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { disposePinia, getActivePinia } from 'pinia';
import { flushPromises } from '@vue/test-utils';
import { nextTick } from 'vue';
import UpdateSection from '@/features/settings/components/UpdateSection.vue';
import SettingsSwitch from '@/components/settings/SettingsSwitch.vue';
import { useNotificationStore } from '@/core/stores/notification';
import { useUpdateStore } from '@/core/stores/update';
import { mockInvoke } from '@/tests/mocks/tauri';
import { mountWithPinia } from '@/tests/utils/mount';

vi.mock('@/core/utils/platform', () => ({
  nativePlatform: 'android', isAndroid: true, isIOS: false,
}));
vi.mock('@tauri-apps/api/app', () => ({ getVersion: vi.fn(async () => '1.1.6') }));

beforeEach(() => {
  localStorage.clear();
  mockInvoke('get_update_status', () => ({
    state: 'idle', info: null, downloaded: 0, total: null, error: null,
  }));
});
afterEach(() => {
  const pinia = getActivePinia();
  if (pinia) disposePinia(pinia);
  vi.unstubAllGlobals();
});

describe('automatic update preference section', () => {
  it('uses the shared preference and retains its last saved display after a storage failure', async () => {
    const wrapper = mountWithPinia(UpdateSection);
    try {
      await flushPromises();
      const control = wrapper.getComponent(SettingsSwitch);
      expect(control.props('modelValue')).toBe(true);
      useUpdateStore().setAutoCheckEnabled(false);
      await nextTick();
      expect(control.props('modelValue')).toBe(false);

      vi.stubGlobal('localStorage', {
        getItem: localStorage.getItem.bind(localStorage),
        setItem: () => { throw new Error('quota exceeded'); },
      });
      await control.trigger('click');
      expect(control.props('modelValue')).toBe(false);
      expect(useUpdateStore().autoCheckEnabled).toBe(false);
      expect(localStorage.getItem('vcp_auto_update_check')).toBe('off');
      const notices = useNotificationStore().activeToasts;
      expect(notices).toHaveLength(1);
      expect(notices[0].type).toBe('error');
      expect(notices[0].message).toContain('未能保存');
    } finally {
      wrapper.unmount();
    }
  });
});
