import { defineStore } from 'pinia';
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';

// 注入规则的完整数据结构，与后端 Rust TarvenRule 严格契合
export interface TarvenRule {
  id: string;
  name: string;
  ruleType: 'system_suffix' | 'user_suffix' | 'context_inject';
  isEnabled: boolean;
  content: string;
  scope: 'global' | 'agent' | 'group';
  wrap: boolean;
  
  // context_inject 专用
  role?: 'user' | 'assistant';
  depth?: number;
  
  // system_suffix / user_suffix 专用
  position?: 'prepend' | 'append';
  
  sortOrder: number;
}

export const useTarvenStore = defineStore('tarven', () => {
  const rules = ref<TarvenRule[]>([]);
  const isSelectorOpen = ref(false);

  // 1. 从 SQLite 获取所有规则
  const fetchRules = async () => {
    try {
      const res = await invoke<TarvenRule[]>('get_tarven_rules');
      rules.value = res || [];
    } catch (e) {
      console.error('Failed to get tarven rules:', e);
    }
  };

  // 2. 保存单个规则 (新建或更新)
  const saveRule = async (rule: TarvenRule) => {
    try {
      await invoke('save_tarven_rule', { rule });
      await fetchRules(); // 刷新列表
    } catch (e) {
      console.error('Failed to save tarven rule:', e);
      throw e;
    }
  };

  // 3. 删除单个规则
  const deleteRule = async (id: string) => {
    try {
      await invoke('delete_tarven_rule', { id });
      await fetchRules();
    } catch (e) {
      console.error('Failed to delete tarven rule:', e);
    }
  };

  // 4. 快速切换单条规则的启用状态
  const toggleRule = async (id: string) => {
    const target = rules.value.find(r => r.id === id);
    if (target) {
      try {
        const nextState = !target.isEnabled;
        target.isEnabled = nextState;
        await invoke('toggle_rule_enabled', { id, enabled: nextState });
      } catch (e) {
        console.error('Failed to toggle rule state:', e);
        // 回滚前端状态
        target.isEnabled = !target.isEnabled;
      }
    }
  };

  // 5. 保存拖拽重排后的顺序
  const saveOrder = async (orderedIds: string[]) => {
    try {
      await invoke('reorder_rules', { ruleIds: orderedIds });
      await fetchRules();
    } catch (e) {
      console.error('Failed to save rules reorder:', e);
    }
  };

  // 6. 调用后端进行 WYSIWYG 规则注入效果预览
  const previewInjection = async (previewRules: TarvenRule[], mockMessages?: any[]) => {
    try {
      return await invoke<any[]>('preview_tarven_injection', {
        rules: previewRules,
        mockMessages
      });
    } catch (e) {
      console.error('Failed to preview tarven injection:', e);
      throw e;
    }
  };

  return {
    rules,
    isSelectorOpen,
    fetchRules,
    saveRule,
    deleteRule,
    toggleRule,
    saveOrder,
    previewInjection,
  };
});
