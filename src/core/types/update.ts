/**
 * OTA 更新系统共享类型。与 Rust `vcp_modules/updater/update_manager.rs`
 * 的 serde camelCase 序列化契约一一对应。
 */

export type UpdateState =
  | 'idle'
  | 'checking'
  | 'available'
  | 'downloading'
  | 'verifying'
  | 'readyToInstall'
  | 'installing'
  | 'failed';

export type UpdateErrorStage = 'check' | 'download' | 'verify' | 'install';

export interface UpdateError {
  stage: UpdateErrorStage;
  message: string;
  retryable: boolean;
}

export interface UpdateInfo {
  hasUpdate: boolean;
  currentVersion: string;
  latestVersion: string;
  releasePageUrl: string | null;
  releaseNotes: string | null;
  apkSize: number | null;
  apkSha256: string | null;
}

export interface UpdateStatus {
  state: UpdateState;
  info: UpdateInfo | null;
  downloaded: number;
  total: number | null;
  error: UpdateError | null;
}

/** Rust 侧状态广播事件名（`UPDATE_STATUS_EVENT`）。 */
export const UPDATE_STATUS_EVENT = 'vcp-update://status';
