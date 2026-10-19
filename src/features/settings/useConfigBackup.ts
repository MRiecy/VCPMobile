// useConfigBackup: 「服务器连接」页配置备份（导出/导入）
// 导出范围：用户身份、应用/语音连接、总结模型与已应用的主题/聊天偏好。
// 头像不在导出范围：头像属于数据同步职责，重装后由同步带回
// 评估与决策记录见 plan/Settings_Import_Export_Evaluation_2026-08-18.md
import { invoke } from "@tauri-apps/api/core";
import { deleteTempFile } from "tauri-plugin-vcp-mobile";
import { pickFile, withNativeFileOperation } from "../../core/utils/nativeFilePicker";
import type { AppSettings } from "../../core/stores/settings";
import type { SettingsFieldValues } from "../../core/config/fields";
import { normalizeThemeModuleKey, useThemeStore, type ThemePreferences } from "../../core/stores/theme";
import { useChatPreferencesStore } from "../chat/preferences/store";
import type { ChatPreferences } from "../chat/preferences/model";
import type { DownloadsSaveResultDto } from "../../core/types/native";
import type { GroupLocalSettings, GroupLocalSettingsPatch } from '../../core/types/assistant';
import { isAndroid, isIOS } from "../../core/utils/platform";

export const CONFIG_BACKUP_KIND = "vcp-mobile-settings-backup";
export const CONFIG_BACKUP_VERSION = 2;
export const CONFIG_BACKUP_EXTENSION = ".vcpcfg.json";
/** 导入文件大小上限（纯配置 JSON 正常仅数 KB） */
export const MAX_BACKUP_FILE_BYTES = 1024 * 1024;

type PartialConfig<T> = { [K in keyof T]?: T[K] extends readonly unknown[] ? T[K] : T[K] extends object ? PartialConfig<T[K]> : T[K] };
/** Rust schema owns export eligibility and import validation; no second field whitelist. */
export type BackupDomains = PartialConfig<AppSettings>;

export interface ConfigBackupFile {
  app: "vcp-mobile";
  kind: typeof CONFIG_BACKUP_KIND;
  version: number;
  exportedAt: string;
  domains: BackupDomains;
  preferences: { theme: ThemePreferences; chat: ChatPreferences };
  groupPreferences?: Record<string, GroupLocalSettings>;
}

export interface ParsedConfigBackup {
  domains: BackupDomains;
  preferences?: { theme?: Partial<ThemePreferences>; chat?: Partial<ChatPreferences> };
  groupPreferences?: Record<string, GroupLocalSettingsPatch>;
  exportedAt: string;
}

export interface BackupImportResult {
  applied: string[];
  skipped: string[];
  failed: { scope: string; message: string }[];
}

const DOMAIN_LABELS: Record<keyof AppSettings, string> = {
  identity: '用户身份', server: '主服务器连接', log: '日志连接', sync: '数据同步',
  distributed: '分布式系统', conversation: '话题总结模型', voice: '语音服务', navigation: '侧栏排序',
  jev: 'JEV 决策服务',
};

const hasFields = (value: object | undefined): boolean => Boolean(value && Object.keys(value).length);
const settingsScopes = (domains: BackupDomains) => (Object.keys(domains) as (keyof AppSettings)[])
  .filter(domain => hasFields(domains[domain]))
  .map(domain => DOMAIN_LABELS[domain]);

/** Only called for nested leaves already accepted by Rust's domain schema. */
export function backupFieldChanges(domains: BackupDomains): Partial<SettingsFieldValues> {
  const changes: Record<string, unknown> = {};
  const visit = (value: unknown, path: string) => {
    if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
      for (const [key, child] of Object.entries(value)) visit(child, path ? `${path}.${key}` : key);
    } else {
      changes[path] = value;
    }
  };
  visit(domains, '');
  return changes as Partial<SettingsFieldValues>;
}

export function configBackupScopes(backup: ParsedConfigBackup): string[] {
  return [
    ...settingsScopes(backup.domains),
    ...(hasFields(backup.preferences?.theme) ? ['主题与深浅模式'] : []),
    ...(hasFields(backup.preferences?.chat) ? ['聊天偏好'] : []),
    ...(hasFields(backup.groupPreferences) ? ['群聊本机设置'] : []),
  ];
}

export function unavailableBackupTheme(backup: ParsedConfigBackup): string | null {
  const key = backup.preferences?.theme?.themeKey;
  return key !== undefined && !normalizeThemeModuleKey(key) ? key : null;
}

export function captureBackupPreferences(): ConfigBackupFile['preferences'] {
  const theme = useThemeStore();
  const chat = useChatPreferencesStore();
  return {
    theme: { themeKey: theme.currentTheme, mode: theme.mode },
    chat: {
      presentationMode: chat.presentationMode,
      smoothStreamingEnabled: chat.smoothStreamingEnabled,
      messageHapticsEnabled: chat.messageHapticsEnabled,
      collapseToolsWhileStreaming: chat.collapseToolsWhileStreaming,
    },
  };
}

/** Apply only validated, present fields through their existing persistence owners. */
export async function applyConfigBackup(
  backup: ParsedConfigBackup,
  applySettings: (changes: Partial<SettingsFieldValues>, groupPreferences: Record<string, GroupLocalSettingsPatch>) => Promise<void>,
): Promise<BackupImportResult> {
  const result: BackupImportResult = { applied: [], skipped: [], failed: [] };
  if (backup.domains.voice?.tts?.mode === 'clone' || backup.domains.voice?.tts?.cloneSampleId) {
    result.skipped.push('复刻参考音频不包含在配置备份中；请在语音设置中确认或重新导入本机样本');
  }
  const databaseScopes = settingsScopes(backup.domains);
  if (hasFields(backup.groupPreferences)) databaseScopes.push('群聊本机设置');
  if (databaseScopes.length) {
    try {
      await applySettings(backupFieldChanges(backup.domains), backup.groupPreferences ?? {});
      result.applied.push(...databaseScopes);
    } catch (error) {
      result.failed.push({ scope: databaseScopes.join('、'), message: String(error) });
    }
  }

  const theme = backup.preferences?.theme;
  if (hasFields(theme)) {
    const missing = unavailableBackupTheme(backup);
    if (missing !== null) {
      result.skipped.push(`主题 ${missing} 在当前版本不可用，已保留当前主题与深浅模式`);
    } else {
      try {
        const applied = await useThemeStore().restorePreferences(theme!);
        if (applied.ok) result.applied.push('主题与深浅模式');
        else result.failed.push({ scope: '主题与深浅模式', message: applied.error || '主题应用失败' });
      } catch (error) {
        result.failed.push({ scope: '主题与深浅模式', message: String(error) });
      }
    }
  }

  const chat = backup.preferences?.chat;
  if (hasFields(chat)) {
    const owner = useChatPreferencesStore();
    const fields = [
      { key: 'presentationMode', label: '消息呈现', apply: () => owner.setPresentationMode(chat!.presentationMode!) },
      { key: 'smoothStreamingEnabled', label: '流式平滑动效', apply: () => owner.setSmoothStreamingEnabled(chat!.smoothStreamingEnabled!) },
      { key: 'messageHapticsEnabled', label: '消息震动', apply: () => owner.setMessageHapticsEnabled(chat!.messageHapticsEnabled!) },
      { key: 'collapseToolsWhileStreaming', label: '流式工具折叠', apply: () => owner.setCollapseToolsWhileStreaming(chat!.collapseToolsWhileStreaming!) },
    ] as const;
    for (const field of fields) {
      if (!Object.prototype.hasOwnProperty.call(chat, field.key)) continue;
      const scope = `聊天偏好 · ${field.label}`;
      try {
        const applied = field.apply();
        if (applied.ok) result.applied.push(scope);
        else result.failed.push({ scope, message: applied.error || '设置保存失败' });
      } catch (error) {
        result.failed.push({ scope, message: String(error) });
      }
    }
  }
  return result;
}

// ------------------------------------------------------------------
// 纯函数（可单测）
// ------------------------------------------------------------------

/** Capture one committed SQLite snapshot, filtered by the Rust field definitions. */
export function captureBackupDomains(): Promise<BackupDomains> {
  return invoke<BackupDomains>('export_config_domains');
}

export function captureBackupGroupPreferences(): Promise<Record<string, GroupLocalSettings>> {
  return invoke<Record<string, GroupLocalSettings>>('export_group_preferences');
}

/** 生成导出文件名：vcp-mobile-config-YYYYMMDD-HHmm.vcpcfg.json */
export function buildBackupFileName(now: Date = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  const stamp = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}`;
  return `vcp-mobile-config-${stamp}${CONFIG_BACKUP_EXTENSION}`;
}

/** 组装备份文件 payload */
export function buildBackupPayload(
  domains: BackupDomains,
  preferences: ConfigBackupFile['preferences'],
  now: Date = new Date(),
  groupPreferences: Record<string, GroupLocalSettings> = {},
): ConfigBackupFile {
  return {
    app: "vcp-mobile",
    kind: CONFIG_BACKUP_KIND,
    version: CONFIG_BACKUP_VERSION,
    exportedAt: now.toISOString(),
    domains,
    groupPreferences,
    preferences: {
      theme: { themeKey: preferences.theme.themeKey, mode: preferences.theme.mode },
      chat: {
        presentationMode: preferences.chat.presentationMode,
        smoothStreamingEnabled: preferences.chat.smoothStreamingEnabled,
        messageHapticsEnabled: preferences.chat.messageHapticsEnabled,
        collapseToolsWhileStreaming: preferences.chat.collapseToolsWhileStreaming,
      },
    },
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
  operationId: string,
  reservationToken?: string,
): Promise<DownloadsSaveResultDto> {
  return await invoke<DownloadsSaveResultDto>("plugin:vcp-mobile|save_to_downloads", {
    fileName,
    contentBase64,
    mimeType: "application/json",
    purpose: 'config',
    operationId,
    ...(reservationToken ? { reservationToken } : {}),
  });
}

/**
 * 导出当前配置到系统下载目录。
 * API 26–28 首次导出时若缺少储存空间权限，会先发起权限申请并重试一次。
 * @returns 保存结果（含展示文件名）
 */
export async function exportConfigBackup(
  domains: BackupDomains,
  operationId: string = `config_export_${crypto.randomUUID()}`,
  groupPreferences: Record<string, GroupLocalSettings> = {},
): Promise<DownloadsSaveResultDto> {
  if (!isAndroid && !isIOS) {
    throw new Error('当前预览环境不提供原生配置文件导出');
  }
  const payload = buildBackupPayload(domains, captureBackupPreferences(), new Date(), groupPreferences);
  const bytes = new TextEncoder().encode(JSON.stringify(payload, null, 2));
  if (bytes.byteLength > MAX_BACKUP_FILE_BYTES) {
    throw new Error('配置备份超过 1 MiB，请检查配置字段');
  }
  const contentBase64 = bytesToBase64(bytes);
  const fileName = buildBackupFileName();

  return withNativeFileOperation(operationId, async reservationToken => {
    try {
      return await invokeSaveToDownloads(fileName, contentBase64, operationId, reservationToken);
    } catch (e: any) {
      const message = String(e);
      if (!isAndroid || !message.includes("储存空间权限")) throw e;
      // API 26–28：引导用户授予储存空间权限后重试一次
      await invoke("plugin:vcp-mobile|request_android_permission", { pType: "storage" });
      return await invokeSaveToDownloads(fileName, contentBase64, operationId, reservationToken);
    }
  });
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
