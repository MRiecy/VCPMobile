import { invoke } from '@tauri-apps/api/core';

// ==================================================================
// Screen
// ==================================================================

export function setKeepScreenOn(): Promise<void> {
  return invoke('plugin:vcp-mobile|set_keep_screen_on');
}

export function clearKeepScreenOn(): Promise<void> {
  return invoke('plugin:vcp-mobile|clear_keep_screen_on');
}

// ==================================================================
// Stream Service
// ==================================================================

export function startStreamService(agentName: string): Promise<void> {
  return invoke('plugin:vcp-mobile|start_streaming_service', { agentName });
}

export function stopStreamService(): Promise<void> {
  return invoke('plugin:vcp-mobile|stop_streaming_service');
}

// ==================================================================
// Native File Picker
// ==================================================================

export interface PickedFile {
  path: string;
  name: string;
  mime: string;
  size: number;
  hash: string;
  thumbnailPath?: string;
}

export function pickFile(): Promise<PickedFile> {
  return invoke<PickedFile>('plugin:vcp-mobile|pick_file');
}
