export interface OverlayActionItem {
  label: string;
  icon?: any; // lucide-vue-next component
  danger?: boolean;
  disabled?: boolean;
  handler: () => void;
}

export interface ContextMenuConfig {
  title: string;
  actions: OverlayActionItem[];
}

export interface PromptConfig {
  title: string;
  initialValue: string;
  placeholder: string;
  onConfirm: (val: string) => void;
}

export interface ConfirmOptions {
  title: string;
  message: string;
  confirmText?: string;
  cancelText?: string;
  isDanger?: boolean;
  onlyConfirm?: boolean;
}

export interface ConfirmConfig extends ConfirmOptions {
  confirmText: string;
  cancelText: string;
  isDanger: boolean;
  onlyConfirm: boolean;
}

export interface EditorConfig {
  initialValue: string;
  onSave: (newContent: string) => void;
}
