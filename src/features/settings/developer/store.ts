import { defineStore } from 'pinia';
import { readonly, ref } from 'vue';
import { readLocalPreference, writeLocalPreference } from '../../../core/preferences/localPreference';

const STORAGE_KEY = 'vcp-developer-mode';

/** Device-local access to explicit diagnostic tools; never part of config backups. */
export const useDeveloperSettingsStore = defineStore('developerSettings', () => {
  const stored = readLocalPreference(STORAGE_KEY);
  const enabled = ref(stored.ok && stored.raw === 'true');

  function setEnabled(value: boolean) {
    const result = writeLocalPreference(STORAGE_KEY, String(value));
    if (result.ok) enabled.value = value;
    return result;
  }

  return { enabled: readonly(enabled), setEnabled };
});
