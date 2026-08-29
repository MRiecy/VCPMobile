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

export function stopStreamService(agentName: string): Promise<void> {
  return invoke('plugin:vcp-mobile|stop_streaming_service', { agentName });
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
  thumbnailPath?: string | null;
}

export function pickFile(): Promise<PickedFile> {
  return invoke<PickedFile>('plugin:vcp-mobile|pick_file');
}

export function openFileNative(path: string): Promise<void> {
  return invoke('plugin:vcp-mobile|open_file_native', { path, action: 'view' });
}

export function shareFileNative(path: string): Promise<void> {
  return invoke('plugin:vcp-mobile|open_file_native', { path, action: 'share' });
}

export interface ApkSignatureVerification {
  apkSha256: string | null;
  selfSha256: string | null;
  matched: boolean;
}

export function verifyApkSignature(path: string): Promise<ApkSignatureVerification> {
  return invoke('plugin:vcp-mobile|verify_apk_signature', { path });
}

export function canInstallPackages(): Promise<boolean> {
  return invoke('plugin:vcp-mobile|can_install_packages');
}

export function openUnknownSourcesSettings(): Promise<void> {
  return invoke('plugin:vcp-mobile|open_unknown_sources_settings');
}

export function acquireOtaKeepalive(): Promise<void> {
  return invoke('plugin:vcp-mobile|acquire_ota_keepalive');
}

export function releaseOtaKeepalive(): Promise<void> {
  return invoke('plugin:vcp-mobile|release_ota_keepalive');
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

export interface DownloadsSaveResult {
  uri: string;
  displayName: string;
  mimeType: string;
  size: number;
}

export function saveToDownloads(fileName: string, contentBase64: string, mimeType?: string): Promise<DownloadsSaveResult> {
  return invoke<DownloadsSaveResult>('plugin:vcp-mobile|save_to_downloads', { fileName, contentBase64, mimeType });
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
