// useConfigBackup: 「服务器连接」页配置备份（导出/导入）
// 导出范围：用户身份 + 服务器连接 + 分布式节点（白名单字段）
// 头像不在导出范围：头像属于数据同步职责，重装后由同步带回
// 评估与决策记录见 plan/Settings_Import_Export_Evaluation_2026-08-18.md
import { invoke } from "@tauri-apps/api/core";
import type { AppSettings } from "../../core/stores/settings";
import type { DownloadsSaveResultDto } from "../../core/types/native";

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
  // 分布式节点
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

/**
 * 解析并校验备份文件内容。失败时抛出带中文说明的 Error。
 * 只放行白名单字段，未知字段一律丢弃（防止垃圾键经 extra flatten 灌入设置）。
 */
export function parseBackupFile(text: string): ParsedConfigBackup {
  if (text.length > MAX_BACKUP_FILE_BYTES) {
    throw new Error("备份文件过大，已拒绝解析");
  }

  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    throw new Error("备份文件不是合法的 JSON");
  }
  if (typeof raw !== "object" || raw === null) {
    throw new Error("备份文件格式不正确");
  }

  const file = raw as Record<string, unknown>;
  if (file.app !== "vcp-mobile" || file.kind !== CONFIG_BACKUP_KIND) {
    throw new Error("这不是 VCP Mobile 的配置备份文件");
  }
  if (file.version !== CONFIG_BACKUP_VERSION) {
    throw new Error(`不支持的备份版本: ${String(file.version)}`);
  }
  if (typeof file.settings !== "object" || file.settings === null) {
    throw new Error("备份文件缺少配置内容");
  }

  const rawSettings = file.settings as Record<string, unknown>;
  const settings: Record<string, string | boolean> = {};
  for (const key of BACKUP_SETTINGS_KEYS) {
    const value = rawSettings[key];
    if (value === undefined || value === null) continue;
    if (key === "chatEndpointMode") {
      if (typeof value !== "string" || !CHAT_ENDPOINT_MODES.has(value)) {
        throw new Error(`备份字段 ${key} 类型不正确`);
      }
      settings[key] = value;
    } else if (BOOLEAN_BACKUP_KEYS.has(key)) {
      if (typeof value !== "boolean") {
        throw new Error(`备份字段 ${key} 类型不正确`);
      }
      settings[key] = value;
    } else {
      if (typeof value !== "string") {
        throw new Error(`备份字段 ${key} 类型不正确`);
      }
      settings[key] = value;
    }
  }

  // 仅导入时识别旧布尔开关；新枚举优先，旧字段绝不进入输出 patch。
  if (settings.chatEndpointMode === undefined && rawSettings.enableVcpToolInjection !== undefined) {
    if (typeof rawSettings.enableVcpToolInjection !== "boolean") {
      throw new Error("备份字段 enableVcpToolInjection 类型不正确");
    }
    settings.chatEndpointMode = rawSettings.enableVcpToolInjection ? "vcpTools" : "standard";
  }

  return {
    settings: settings as Partial<AppSettings>,
    exportedAt: typeof file.exportedAt === "string" ? file.exportedAt : "",
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
  const payload = buildBackupPayload(settings);
  const contentBase64 = bytesToBase64(new TextEncoder().encode(JSON.stringify(payload, null, 2)));
  const fileName = buildBackupFileName();

  try {
    return await invokeSaveToDownloads(fileName, contentBase64);
  } catch (e: any) {
    const message = String(e);
    if (!message.includes("储存空间权限")) throw e;
    // API 26–28：引导用户授予储存空间权限后重试一次
    await invoke("plugin:vcp-mobile|request_android_permission", { pType: "storage" });
    return await invokeSaveToDownloads(fileName, contentBase64);
  }
}

/** 读取并解析导入文件（纯 IO + 校验，不写入任何状态） */
export async function readConfigBackupFile(file: File): Promise<ParsedConfigBackup> {
  if (file.size > MAX_BACKUP_FILE_BYTES) {
    throw new Error("备份文件过大（超过 4MB），已拒绝导入");
  }
  const text = await file.text();
  return parseBackupFile(text);
}
