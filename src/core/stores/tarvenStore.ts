import { defineStore } from 'pinia';
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';

export interface TarvenRule {
  id: string;
  name: string;
  content: string;
  enabled: boolean;
  icon?: string;
}

export const useTarvenStore = defineStore('tarven', () => {
  const rules = ref<TarvenRule[]>([]);
  const isSelectorOpen = ref(false);

  const fetchRules = async () => {
    try {
      const res = await invoke<TarvenRule[]>('get_tarven_rules');
      rules.value = res || [];
    } catch (e) {
      console.error('Failed to get tarven rules:', e);
    }
  };

  const saveRules = async () => {
    try {
      await invoke('save_tarven_rules', { rules: rules.value });
    } catch (e) {
      console.error('Failed to save tarven rules:', e);
    }
  };

  const addRule = async (name: string, content: string, icon?: string) => {
    const newRule: TarvenRule = {
      id: `rule_${Date.now()}_${Math.random().toString(36).substring(2, 7)}`,
      name,
      content,
      enabled: true,
      icon,
    };
    rules.value.push(newRule);
    await saveRules();
  };

  const updateRule = async (id: string, updates: Partial<Omit<TarvenRule, 'id'>>) => {
    const idx = rules.value.findIndex(r => r.id === id);
    if (idx !== -1) {
      rules.value[idx] = { ...rules.value[idx], ...updates };
      await saveRules();
    }
  };

  const deleteRule = async (id: string) => {
    rules.value = rules.value.filter(r => r.id !== id);
    await saveRules();
  };

  const toggleRule = async (id: string) => {
    const idx = rules.value.findIndex(r => r.id === id);
    if (idx !== -1) {
      rules.value[idx].enabled = !rules.value[idx].enabled;
      await saveRules();
    }
  };

  return {
    rules,
    isSelectorOpen,
    fetchRules,
    addRule,
    updateRule,
    deleteRule,
    toggleRule,
  };
});
