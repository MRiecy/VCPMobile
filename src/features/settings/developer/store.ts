import { defineStore } from 'pinia';
import { readonly, ref } from 'vue';
import { readLocalPreference, writeLocalPreference } from '../../../core/preferences/localPreference';

const STORAGE_KEY = 'vcp-developer-mode';

/** Device-local access to explicit diagnostic tools; never part of config backups. */
export const useDeveloperSettingsStore = defineStore('developerSettings', () => {
  const stored = readLocalPreference(STORAGE_KEY);
  const enabled = ref(stored.ok && stored.raw === 'true');
  if (!stored.ok) {
    console.warn('[DeveloperSettings] Failed to read preference:', stored.error);
  } else if (stored.raw !== null && stored.raw !== 'true' && stored.raw !== 'false') {
    const repaired = writeLocalPreference(STORAGE_KEY, 'false');
    if (!repaired.ok) console.warn('[DeveloperSettings] Failed to repair preference:', repaired.error);
  }

  function setEnabled(value: boolean) {
    const result = writeLocalPreference(STORAGE_KEY, String(value));
    if (result.ok) enabled.value = value;
    return result;
  }

  return { enabled: readonly(enabled), setEnabled };
});
