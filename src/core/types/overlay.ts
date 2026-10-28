export interface OverlayActionItem {
  id?: string;
  label: string;
  icon?: any; // @lucide/vue component
  danger?: boolean;
  disabled?: boolean;
  selected?: boolean;
  handler: () => void;
}

export interface ContextMenuConfig {
  title: string;
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
