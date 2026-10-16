// useConfigBackup: 「服务器连接」页配置备份（导出/导入）
// 导出范围：用户身份 + 应用连接（含插件中心连接，白名单字段）
// 头像不在导出范围：头像属于数据同步职责，重装后由同步带回
// 评估与决策记录见 plan/Settings_Import_Export_Evaluation_2026-08-18.md
import { invoke } from "@tauri-apps/api/core";
import { pickFile, deleteTempFile } from "tauri-plugin-vcp-mobile";
import type { AppSettings } from "../../core/stores/settings";
import type { DownloadsSaveResultDto } from "../../core/types/native";
import { isAndroid, isIOS } from "../../core/utils/platform";

export const CONFIG_BACKUP_KIND = "vcp-mobile-settings-backup";
export const CONFIG_BACKUP_VERSION = 1;
export const CONFIG_BACKUP_EXTENSION = ".vcpcfg";
/** 导入文件大小上限（纯配置 JSON 正常仅数 KB） */
export const MAX_BACKUP_FILE_BYTES = 1024 * 1024;

/** 导出白名单：仅这些字段进入备份文件；extra 透传字段与本地偏好被显式排除 */
export const BACKUP_SETTINGS_KEYS = [
  // 用户身份
  "userName",
  "adminUsername",
  "adminPassword",
  // 核心连接
  "vcpServerUrl",
  "chatEndpointMode",
  "vcpApiKey",
  "vcpLogUrl",
  "vcpLogKey",
  // 数据同步
  "syncHttpUrl",
  "syncServerUrl",
  "syncToken",
  "fileKey",
  // 插件中心连接
  "distributedEnabled",
  "distributedWsUrl",
  "distributedVcpKey",
  "distributedDeviceName",
] as const;

export type BackupSettingsKey = (typeof BACKUP_SETTINGS_KEYS)[number];

/** 布尔字段；白名单内其余字段均为字符串 */
const BOOLEAN_BACKUP_KEYS: ReadonlySet<string> = new Set(["distributedEnabled"]);
const CHAT_ENDPOINT_MODES = new Set(["standard", "vcpTools", "raw"]);

export interface ConfigBackupFile {
  app: "vcp-mobile";
  kind: typeof CONFIG_BACKUP_KIND;
  version: number;
  exportedAt: string;
  settings: Partial<AppSettings>;
}

export interface ParsedConfigBackup {
  settings: Partial<AppSettings>;
  exportedAt: string;
}

// ------------------------------------------------------------------
// 纯函数（可单测）
// ------------------------------------------------------------------

/** 按白名单从完整设置中提取备份字段 */
export function pickBackupSettings(settings: AppSettings): Partial<AppSettings> {
  const picked: Record<string, string | boolean> = {};
  for (const key of BACKUP_SETTINGS_KEYS) {
    const value = settings[key];
    if (key === "chatEndpointMode") {
      picked[key] = typeof value === "string" && CHAT_ENDPOINT_MODES.has(value)
        ? value
        : "standard";
    } else if (BOOLEAN_BACKUP_KEYS.has(key)) {
      picked[key] = value === true;
    } else if (typeof value === "string") {
      picked[key] = value;
    } else {
      picked[key] = "";
    }
  }
  return picked as Partial<AppSettings>;
}

/** 生成导出文件名：vcp-mobile-config-YYYYMMDD-HHmm.vcpcfg */
export function buildBackupFileName(now: Date = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  const stamp = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}`;
  return `vcp-mobile-config-${stamp}${CONFIG_BACKUP_EXTENSION}`;
}

/** 组装备份文件 payload */
export function buildBackupPayload(
  settings: AppSettings,
  now: Date = new Date(),
): ConfigBackupFile {
  return {
    app: "vcp-mobile",
    kind: CONFIG_BACKUP_KIND,
    version: CONFIG_BACKUP_VERSION,
    exportedAt: now.toISOString(),
    settings: pickBackupSettings(settings),
  };
}


// ------------------------------------------------------------------
// Base64 工具（分块避免栈溢出）
// ------------------------------------------------------------------

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

// ------------------------------------------------------------------
// IO（依赖 Tauri IPC，组件内调用）
// ------------------------------------------------------------------

async function invokeSaveToDownloads(
  fileName: string,
  contentBase64: string,
): Promise<DownloadsSaveResultDto> {
  return await invoke<DownloadsSaveResultDto>("plugin:vcp-mobile|save_to_downloads", {
    fileName,
    contentBase64,
    mimeType: "application/json",
  });
}

/**
 * 导出当前配置到系统下载目录。
 * API 26–28 首次导出时若缺少储存空间权限，会先发起权限申请并重试一次。
 * @returns 保存结果（含展示文件名）
 */
export async function exportConfigBackup(
  settings: AppSettings,
): Promise<DownloadsSaveResultDto> {
  if (!isAndroid && !isIOS) {
    throw new Error('当前预览环境不提供原生配置文件导出');
  }
  const payload = buildBackupPayload(settings);
  const bytes = new TextEncoder().encode(JSON.stringify(payload, null, 2));
  if (bytes.byteLength > MAX_BACKUP_FILE_BYTES) {
    throw new Error('配置备份超过 1 MiB，请检查连接配置字段');
  }
  const contentBase64 = bytesToBase64(bytes);
  const fileName = buildBackupFileName();

  try {
    return await invokeSaveToDownloads(fileName, contentBase64);
  } catch (e: any) {
    const message = String(e);
    if (!isAndroid || !message.includes("储存空间权限")) throw e;
    // API 26–28：引导用户授予储存空间权限后重试一次
    await invoke("plugin:vcp-mobile|request_android_permission", { pType: "storage" });
    return await invokeSaveToDownloads(fileName, contentBase64);
  }
}

/** Native staging is released after Rust has returned the validated patch. */
export async function pickConfigBackup(operationId: string): Promise<ParsedConfigBackup> {
  if (!isAndroid && !isIOS) {
    throw new Error('当前预览环境不提供原生配置文件导入');
  }
  const picked = await pickFile('config', { operationId, maxBytes: MAX_BACKUP_FILE_BYTES });
  try {
    return await invoke<ParsedConfigBackup>('parse_config_backup', { path: picked.path });
  } finally {
    await deleteTempFile(picked.path).catch(console.warn);
  }
}
