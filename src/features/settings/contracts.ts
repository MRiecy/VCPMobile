import type { Component } from 'vue';

export type SettingsPageId =
  | 'identity'
  | 'connection'
  | 'theme'
  | 'rendering'
  | 'advanced'
  | 'guide'
  | 'about';

export interface SettingsPageEntry {
  id: SettingsPageId;
  title: string;
  description: string;
  component: Component;
  frame: 'list' | 'viewport' | 'immersive';
}

export interface SettingsPageHandle {
  beforeLeave?: () => Promise<boolean>;
}

export interface SettingsPageProps {
  active: boolean;
}
