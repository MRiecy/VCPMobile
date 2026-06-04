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

export function openFileNative(path: string): Promise<void> {
  return invoke('plugin:vcp-mobile|open_file_native', { path });
}

export interface GallerySaveResult {
  uri: string;
  displayName: string;
  mimeType: string;
  size: number;
}

export function saveImageToGallery(sourceUrl: string, fileName?: string): Promise<GallerySaveResult> {
  return invoke<GallerySaveResult>('plugin:vcp-mobile|save_image_to_gallery', { sourceUrl, fileName });
}

export function saveImageFromPath(imagePath: string, fileName?: string): Promise<GallerySaveResult> {
  return invoke<GallerySaveResult>('plugin:vcp-mobile|save_image_from_path', { imagePath, fileName });
}

export function writeTempFile(bytes: Uint8Array, fileName: string): Promise<string> {
  return invoke<string>('plugin:vcp-mobile|write_temp_file', { bytes: Array.from(bytes), fileName });
}

export interface RootAccessStatus {
  isRoot: boolean;
}

export function checkRootAccess(): Promise<RootAccessStatus> {
  return invoke<RootAccessStatus>('plugin:vcp-mobile|check_root_access');
}

export interface RootCommandResult {
  success: boolean;
  output: string;
}

export function runRootCommand(command: string): Promise<RootCommandResult> {
  return invoke<RootCommandResult>('plugin:vcp-mobile|run_root_command', { command });
}

export interface LaunchRootManagerResult {
  success: boolean;
  manager?: string;
  message?: string;
}

export function launchRootManager(): Promise<LaunchRootManagerResult> {
  return invoke<LaunchRootManagerResult>('plugin:vcp-mobile|launch_root_manager');
}

