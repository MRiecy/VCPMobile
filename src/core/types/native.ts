export const androidStartupPermissionIds = ['notification', 'battery', 'backgroundAllowed', 'notificationListener'] as const;
export type AndroidStartupPermission = typeof androidStartupPermissionIds[number];

// null means Android cannot evaluate this item on the current device/profile.
export interface PermissionStatusDto extends Record<AndroidStartupPermission, boolean | null> {
  storage: boolean;
  microphone: boolean;
  camera: boolean;
  location: boolean;
}

export function hasMissingStartupPermission(status: PermissionStatusDto): boolean {
  return androidStartupPermissionIds.some(id => status[id] === false);
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
