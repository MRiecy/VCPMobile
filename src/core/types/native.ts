export interface PermissionStatusDto {
  notification: boolean;
  storage: boolean;
  battery: boolean;
  microphone: boolean;
  camera: boolean;
  location: boolean;
}

export interface ListenerPermissionDto {
  enabled: boolean;
}

export interface DiskSpaceDto {
  freeBytes: number;
  freeGb: number;
  totalBytes: number;
  totalGb: number;
}

export interface BatteryStatusDto {
  level: number;
  isPowerSaveMode: boolean;
  status: string | null;
  temperature: number | null;
}

export interface NativeFileStartDetail {
  name: string;
  size: number;
  mime: string;
}

export interface NativeFileProgressDetail extends NativeFileStartDetail {
  loaded: number;
  total: number;
  progress: number;
}

export interface DownloadsSaveResultDto {
  uri: string;
  displayName: string;
  mimeType: string;
  size: number;
}
