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
  return invoke('plugin:vcp-mobile|start_stream_service', { agentName });
}

export function stopStreamService(): Promise<void> {
  return invoke('plugin:vcp-mobile|stop_stream_service');
}
