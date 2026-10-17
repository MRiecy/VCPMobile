import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useSettingsStore } from '../../../core/stores/settings';
import type { SettingsEditableKey } from './fieldSets';
import { useSettingsEditSession } from './useSettingsEditSession';

/** The editor belongs to one page visit, even while the settings shell stays mounted. */
export function useSettingsPageSession<const Keys extends readonly SettingsEditableKey[]>(
  keys: Keys,
  isActive: () => boolean,
) {
  const store = useSettingsStore();
  const session = useSettingsEditSession(keys);
  const loading = ref(false);
  const loadError = ref<string | null>(null);
  let disposed = false;

  const dispose = () => {
    if (disposed) return;
    disposed = true;
    session.dispose();
  };
  const reload = async () => {
    if (disposed || !isActive() || loading.value) return;
    loading.value = true;
    loadError.value = null;
    try {
      await store.fetchSettings();
    } catch (error) {
      if (!disposed) loadError.value = String(error);
    } finally {
      if (!disposed) loading.value = false;
    }
  };

  watch(isActive, (active) => { if (!active) dispose(); }, { flush: 'sync' });
  onMounted(() => { void reload(); });
  onBeforeUnmount(dispose);

  return {
    session,
    loading,
    error: computed(() => session.error.value || loadError.value),
    reload,
  };
}
