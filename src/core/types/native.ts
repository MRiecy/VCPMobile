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
  volume?: string;
}

export interface BatteryStatusDto {
  level: number | null;
  isPowerSaveMode: boolean;
  status: string | null;
  temperature: number | null;
  unavailable?: Record<string, string>;
}

export interface NativeFileStartDetail {
  operationId?: string;
  selectionIndex?: number;
  selectionCount?: number;
  name: string;
  size: number;
  mime: string;
}

export interface NativeFileProgressDetail {
  operationId?: string;
  selectionIndex?: number;
  selectionCount?: number;
  name: string;
  mime: string;
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
