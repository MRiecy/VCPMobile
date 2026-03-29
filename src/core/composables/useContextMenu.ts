import { ref } from 'vue';
import type { ActionItem } from '../../components/ui/BottomSheet.vue';

// 全局状态单例
const isOpen = ref(false);
const currentTitle = ref('');
const currentActions = ref<ActionItem[]>([]);

export const useContextMenu = () => {
  /**
   * 打开底部操作菜单
   * @param actions 菜单项配置数组
   * @param title 可选：菜单标题
   */
  const openMenu = (actions: ActionItem[], title?: string) => {
    currentActions.value = actions;
    currentTitle.value = title || '';
    isOpen.value = true;
  };

  /**
   * 强制关闭菜单
   */
  const closeMenu = () => {
    isOpen.value = false;
  };

  return {
    isOpen,
    currentTitle,
    currentActions,
    openMenu,
    closeMenu
  };
};