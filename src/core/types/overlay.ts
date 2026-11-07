export interface OverlayActionItem {
  id?: string;
  label: string;
  icon?: any; // @lucide/vue component
  danger?: boolean;
  disabled?: boolean;
  selected?: boolean;
  /** 次级说明（菜单项副标题）。 */
  description?: string;
  /** 在该项之前插入分隔线，用于把「恢复/重建」类动作与选项列表分开。 */
  showDivider?: boolean;
  handler: () => void;
}

export interface ContextMenuConfig {
  title: string;
  /** 标题下方的补充说明（例如操作边界提示）。 */
  note?: string;
  actions: OverlayActionItem[];
  headerAction?: OverlayActionItem;
}

export interface PromptConfig {
  title: string;
  initialValue: string;
  placeholder: string;
  inputMode?: 'text' | 'numeric';
  validate?: (value: string) => string | null;
  onConfirm: (val: string) => void;
}

export interface EditorConfig {
  initialValue: string;
  onSave: (newContent: string) => void | Promise<void>;
}

export interface ConfirmConfig {
  title: string;
  message: string;
  isDanger?: boolean;
  onlyConfirm?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}
