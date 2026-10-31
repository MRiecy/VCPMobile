// useConfigBackup: 「服务器连接」页配置备份（导出/导入）
// 导出范围：用户身份、应用/语音连接、总结模型与已应用的主题/聊天偏好。
// 头像不在导出范围：头像属于数据同步职责，重装后由同步带回
// 评估与决策记录见 plan/Settings_Import_Export_Evaluation_2026-08-18.md
import { invoke } from "@tauri-apps/api/core";
import { deleteTempFile } from "tauri-plugin-vcp-mobile";
import { pickFile, withNativeFileOperation } from "../../core/utils/nativeFilePicker";
import { useSettingsStore, type AppSettings } from "../../core/stores/settings";
import { buildSettingsUpdate, type SettingsFieldValues } from "../../core/config/fields";
import { useTopicStore } from "../../core/stores/topicListManager";
import { useDiaryStore } from "../diary/diaryStore";
import { useUpdateStore } from "../../core/stores/update";
import { useAssistantStore } from "../../core/stores/assistant";
import { useNotificationStore } from "../../core/stores/notification";
import { normalizeThemePreferencePatch, useThemeStore } from "../../core/stores/theme";
import { useChatPreferencesStore } from "../chat/preferences/store";
import { normalizeChatPreferencePatch } from "../chat/preferences/model";
import type { DownloadsSaveResultDto } from "../../core/types/native";
import type { GroupLocalSettings, GroupLocalSettingsPatch } from '../../core/types/assistant';
import { isAndroid, isIOS } from "../../core/utils/platform";

export const CONFIG_BACKUP_KIND = "vcp-mobile-settings-backup";
export const CONFIG_BACKUP_VERSION = 2;
export const CONFIG_BACKUP_EXTENSION = ".vcpcfg.json";
/** 导入文件大小上限（纯配置 JSON 正常仅数 KB） */
export const MAX_BACKUP_FILE_BYTES = 1024 * 1024;

import {
  STAGED_BACKUP_STORAGE_KEY,
  STAGED_BACKUP_TTL_MS,
  type BackupDomains,
  type AgentVoiceConfig,
  type AgentLocalPreferences,
  type AgentLocalSettings,
  type AgentLocalSettingsPatch,
  type TarvenRule,
  type StandaloneData,
  type StandaloneDataImport,
  type ClientPreferences,
  type ConfigBackupPayload,
  type ConfigBackupFile,
  type ParsedConfigBackup,
  type RawParsedConfigBackup,
  type BackupImportResult,
  type StagedEntityBackupPayload,
} from "./types";

export {
  STAGED_BACKUP_STORAGE_KEY,
  STAGED_BACKUP_TTL_MS,
  type BackupDomains,
  type AgentVoiceConfig,
  type AgentLocalPreferences,
  type AgentLocalSettings,
  type AgentLocalSettingsPatch,
  type TarvenRule,
  type StandaloneData,
  type StandaloneDataImport,
  type ClientPreferences,
  type ConfigBackupPayload,
  type ConfigBackupFile,
  type ParsedConfigBackup,
  type RawParsedConfigBackup,
  type BackupImportResult,
  type StagedEntityBackupPayload,
};

export function normalizeParsedConfigBackup(backup: RawParsedConfigBackup): ParsedConfigBackup {
  if (backup.preferences === undefined) return backup as ParsedConfigBackup;
  const raw = backup.preferences;
  if (raw === null || typeof raw !== 'object' || Array.isArray(raw)) {
    return { ...backup, preferences: {
      theme: normalizeThemePreferencePatch(null),
      chat: normalizeChatPreferencePatch(null),
    } };
  }
  const source = raw as Record<string, unknown>;
  const preferences: NonNullable<ParsedConfigBackup['preferences']> = {};
  if (Object.prototype.hasOwnProperty.call(source, 'theme')) {
    const theme = normalizeThemePreferencePatch(source.theme);
    if (Object.keys(theme).length) preferences.theme = theme;
  }
  if (Object.prototype.hasOwnProperty.call(source, 'chat')) {
    const chat = normalizeChatPreferencePatch(source.chat);
    if (Object.keys(chat).length) preferences.chat = chat;
  }
  if (Object.prototype.hasOwnProperty.call(source, 'topic') && source.topic && typeof source.topic === 'object') {
    const topicRaw = source.topic as Record<string, unknown>;
    const topicNorm: { sortMode?: 'created' | 'updated'; pinnedTopicKeys?: string[] } = {};
    if (topicRaw.sortMode === 'created' || topicRaw.sortMode === 'updated') {
      topicNorm.sortMode = topicRaw.sortMode;
    }
    if (Array.isArray(topicRaw.pinnedTopicKeys)) {
      topicNorm.pinnedTopicKeys = topicRaw.pinnedTopicKeys.filter(k => typeof k === 'string');
    }
    if (Object.keys(topicNorm).length) preferences.topic = topicNorm;
  }
  if (Object.prototype.hasOwnProperty.call(source, 'diary') && source.diary && typeof source.diary === 'object') {
    const diaryRaw = source.diary as Record<string, unknown>;
    const diaryNorm: { hiddenFolders?: string[]; folderOrder?: string[]; collapsedCategories?: ('diary' | 'cluster')[] } = {};
    if (Array.isArray(diaryRaw.hiddenFolders)) {
      diaryNorm.hiddenFolders = diaryRaw.hiddenFolders.filter(f => typeof f === 'string');
    }
    if (Array.isArray(diaryRaw.folderOrder)) {
      diaryNorm.folderOrder = diaryRaw.folderOrder.filter(f => typeof f === 'string');
    }
    if (Array.isArray(diaryRaw.collapsedCategories)) {
      diaryNorm.collapsedCategories = diaryRaw.collapsedCategories.filter(c => c === 'diary' || c === 'cluster');
    }
    if (Object.keys(diaryNorm).length) preferences.diary = diaryNorm;
  }
  if (Object.prototype.hasOwnProperty.call(source, 'client') && source.client && typeof source.client === 'object') {
    const clientRaw = source.client as Record<string, unknown>;
    if (typeof clientRaw.autoCheckUpdate === 'boolean') {
      preferences.client = { autoCheckUpdate: clientRaw.autoCheckUpdate };
    }
  }
  return { ...backup, preferences };
}


const DOMAIN_LABELS: Record<keyof AppSettings, string> = {
  identity: '用户身份', server: '主服务器连接', log: '日志连接', sync: '数据同步',
  distributed: '分布式系统', conversation: '话题总结模型', voice: '语音服务', navigation: '侧栏排序',
  jev: 'JEV 设置',
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
  backup = normalizeParsedConfigBackup(backup);
  return [
    ...settingsScopes(backup.domains),
    ...(hasFields(backup.preferences?.theme) ? ['主题与深浅模式'] : []),
    ...(hasFields(backup.preferences?.chat) ? ['聊天偏好'] : []),
    ...(hasFields(backup.preferences?.topic) ? ['话题展示与置顶偏好'] : []),
    ...(hasFields(backup.preferences?.diary) ? ['日记目录排版'] : []),
    ...(backup.preferences?.client && typeof backup.preferences.client.autoCheckUpdate === 'boolean' ? ['自动更新检查偏好'] : []),
    ...(hasFields(backup.groupPreferences) ? ['群聊设置'] : []),
    ...(hasFields(backup.agentPreferences) ? ['智能体专属设置'] : []),
    ...(backup.standaloneData?.modelFavorites?.length ? ['常用模型收藏'] : []),
    ...(backup.standaloneData?.tarvenRules?.length ? ['Tarven规则'] : []),
  ];
}

export function captureBackupPreferences(): ConfigBackupFile['preferences'] {
  const theme = useThemeStore();
  const chat = useChatPreferencesStore();
  const topic = useTopicStore();
  const diary = useDiaryStore();
  const update = useUpdateStore();

  return {
    theme: { themeKey: theme.currentTheme, mode: theme.mode },
    chat: {
      presentationMode: chat.presentationMode,
      smoothStreamingEnabled: chat.smoothStreamingEnabled,
      messageHapticsEnabled: chat.messageHapticsEnabled,
      collapseToolsWhileStreaming: chat.collapseToolsWhileStreaming,
    },
    topic: {
      sortMode: topic.sortMode,
      pinnedTopicKeys: [...topic.pinnedTopicKeys],
    },
    diary: {
      hiddenFolders: [...diary.hiddenFolders],
      folderOrder: [...diary.folderOrder],
      collapsedCategories: [...diary.collapsedCategories],
    },
    client: {
      autoCheckUpdate: update.autoCheckEnabled,
    },
  };
}

export async function restoreBackupPreferences(
  prefs: ConfigBackupFile['preferences'] | undefined,
  result: BackupImportResult,
): Promise<void> {
  if (!prefs) return;

  const theme = prefs.theme;
  if (hasFields(theme)) {
    try {
      const applied = await useThemeStore().restorePreferences(theme!);
      if (applied.ok) result.applied.push('主题与深浅模式');
      else result.failed.push({ scope: '主题与深浅模式', message: applied.error || '主题应用失败' });
    } catch (error) {
      result.failed.push({ scope: '主题与深浅模式', message: String(error) });
    }
  }

  const chat = prefs.chat;
  if (hasFields(chat)) {
    const owner = useChatPreferencesStore();
    const fields = [
      { key: 'presentationMode', label: '消息呈现', apply: () => owner.setPresentationMode(chat!.presentationMode!) },
      { key: 'smoothStreamingEnabled', label: '流式平滑动效', apply: () => owner.setSmoothStreamingEnabled(chat!.smoothStreamingEnabled!) },
      { key: 'messageHapticsEnabled', label: '消息震动', apply: () => owner.setMessageHapticsEnabled(chat!.messageHapticsEnabled!) },
      { key: 'collapseToolsWhileStreaming', label: '流式工具折叠', apply: () => owner.setCollapseToolsWhileStreaming(chat!.collapseToolsWhileStreaming!) },
    ] as const;
    for (const field of fields) {
      if (!Object.prototype.hasOwnProperty.call(chat, field.key) || chat[field.key] === undefined) continue;
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

  const topic = prefs.topic;
  if (topic && hasFields(topic)) {
    const topicStore = useTopicStore();
    if (topic.sortMode) {
      topicStore.sortMode = topic.sortMode;
    }
    if (Array.isArray(topic.pinnedTopicKeys)) {
      topicStore.pinnedTopicKeys = topic.pinnedTopicKeys.filter(k => typeof k === 'string');
    }
    result.applied.push('话题展示与置顶偏好');
  }

  const diary = prefs.diary;
  if (diary && hasFields(diary)) {
    const diaryStore = useDiaryStore();
    if (Array.isArray(diary.hiddenFolders)) {
      diaryStore.hiddenFolders = diary.hiddenFolders;
    }
    if (Array.isArray(diary.folderOrder)) {
      diaryStore.folderOrder = diary.folderOrder;
    }
    if (Array.isArray(diary.collapsedCategories)) {
      diaryStore.collapsedCategories = diary.collapsedCategories;
    }
    result.applied.push('日记目录排版');
  }

  const client = prefs.client;
  if (client && typeof client.autoCheckUpdate === 'boolean') {
    useUpdateStore().setAutoCheckEnabled(client.autoCheckUpdate);
    result.applied.push('自动更新检查偏好');
  }
}

export const restoreClientPreferences = restoreBackupPreferences;

/** Apply present, normalized fields through their existing persistence owners. */
export async function applyConfigBackup(
  backup: ParsedConfigBackup,
  applySettings: (
    changes: Partial<SettingsFieldValues>,
    groupPreferences: Record<string, GroupLocalSettingsPatch>,
    agentPreferences?: Record<string, AgentLocalSettingsPatch>,
    standaloneData?: StandaloneDataImport,
  ) => Promise<{ missingGroups?: boolean; missingAgents?: boolean } | boolean>,
): Promise<BackupImportResult> {
  backup = normalizeParsedConfigBackup(backup);
  const result: BackupImportResult = { applied: [], skipped: [], failed: [] };
  if (backup.domains.voice?.tts?.mode === 'clone' || backup.domains.voice?.tts?.cloneSampleId) {
    result.skipped.push('复刻参考音频不包含在配置备份中；请在语音设置中确认或重新导入本机样本');
  }
  const databaseScopes = settingsScopes(backup.domains);
  if (hasFields(backup.groupPreferences)) databaseScopes.push('群聊设置');
  if (hasFields(backup.agentPreferences)) databaseScopes.push('智能体专属设置');
  if (backup.standaloneData?.modelFavorites?.length) databaseScopes.push('常用模型收藏');
  if (backup.standaloneData?.tarvenRules?.length) databaseScopes.push('Tarven规则');

  if (databaseScopes.length) {
    try {
      const applyRes = (backup.agentPreferences !== undefined || backup.standaloneData !== undefined)
        ? await applySettings(
            backupFieldChanges(backup.domains),
            backup.groupPreferences ?? {},
            backup.agentPreferences ?? {},
            backup.standaloneData,
          )
        : await applySettings(
            backupFieldChanges(backup.domains),
            backup.groupPreferences ?? {},
          );
      result.applied.push(...databaseScopes);

      let missingGroups = false;
      let missingAgents = false;
      if (typeof applyRes === 'boolean') {
        missingGroups = applyRes;
      } else if (applyRes && typeof applyRes === 'object') {
        missingGroups = Boolean(applyRes.missingGroups ?? (applyRes as any).missing_groups);
        missingAgents = Boolean(applyRes.missingAgents ?? (applyRes as any).missing_agents);
      }

      if (missingGroups) {
        result.skipped.push('部分群聊尚未同步到本机，请先同步，再次导入');
      }
      if (missingAgents) {
        result.skipped.push('部分智能体尚未同步到本机，请先同步，再次导入');
      }
    } catch (error) {
      result.failed.push({ scope: databaseScopes.join('、'), message: String(error) });
    }
  }

  if (backup.preferences) {
    await restoreBackupPreferences(backup.preferences as ConfigBackupFile['preferences'], result);
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

export function captureBackupAgentPreferences(): Promise<Record<string, AgentLocalPreferences>> {
  return invoke<Record<string, AgentLocalPreferences>>('export_agent_preferences');
}

export async function captureBackupStandaloneData(): Promise<StandaloneData> {
  const [modelFavorites, tarvenRules] = await Promise.all([
    invoke<string[]>('get_favorite_models').catch(() => []),
    invoke<TarvenRule[]>('get_tarven_rules').catch(() => []),
  ]);
  return {
    modelFavorites,
    tarvenRules,
  };
}

export async function captureBackupPayload(now: Date = new Date()): Promise<ConfigBackupFile> {
  const [domains, groupPreferences, agentPreferences, standaloneData] = await Promise.all([
    captureBackupDomains(),
    captureBackupGroupPreferences().catch(() => ({})),
    captureBackupAgentPreferences().catch(() => ({})),
    captureBackupStandaloneData().catch(() => ({ modelFavorites: [], tarvenRules: [] })),
  ]);
  const preferences = captureBackupPreferences();
  return buildBackupPayload(domains, preferences, now, groupPreferences, agentPreferences, standaloneData);
}

/** Check whether staged entity backup payload is still within its 7-day TTL */
export function isStagedPayloadValid(
  payload: StagedEntityBackupPayload | null,
  now: number = Date.now(),
): boolean {
  if (!payload || typeof payload !== "object") return false;
  if (typeof payload.timestamp !== "number" || !Number.isFinite(payload.timestamp)) return false;
  if (payload.version !== CONFIG_BACKUP_VERSION) return false;
  const age = now - payload.timestamp;
  return age >= 0 && age <= STAGED_BACKUP_TTL_MS;
}

/** Helper to read staged backup safely with corrupted JSON defense */
export function readStagedEntityBackup(now: number = Date.now()): StagedEntityBackupPayload | null {
  try {
    const raw = localStorage.getItem(STAGED_BACKUP_STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as StagedEntityBackupPayload;
    if (!isStagedPayloadValid(parsed, now)) {
      localStorage.removeItem(STAGED_BACKUP_STORAGE_KEY);
      return null;
    }
    return parsed;
  } catch {
    localStorage.removeItem(STAGED_BACKUP_STORAGE_KEY);
    return null;
  }
}

/** Helper to write staged entity backup */
export function writeStagedEntityBackup(
  payload: Omit<StagedEntityBackupPayload, "timestamp" | "version">,
  timestamp: number = Date.now(),
): void {
  const staged: StagedEntityBackupPayload = {
    ...payload,
    timestamp,
    version: CONFIG_BACKUP_VERSION,
  };
  localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, JSON.stringify(staged));
}

/** Helper to clear staged entity backup */
export function clearStagedEntityBackup(): void {
  localStorage.removeItem(STAGED_BACKUP_STORAGE_KEY);
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
  agentPreferences: Record<string, AgentLocalPreferences> = {},
  standaloneData?: StandaloneData,
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
      topic: {
        sortMode: preferences.topic?.sortMode ?? 'created',
        pinnedTopicKeys: [...(preferences.topic?.pinnedTopicKeys ?? [])],
      },
      diary: {
        hiddenFolders: [...(preferences.diary?.hiddenFolders ?? [])],
        folderOrder: [...(preferences.diary?.folderOrder ?? [])],
        collapsedCategories: [...(preferences.diary?.collapsedCategories ?? [])],
      },
      client: {
        autoCheckUpdate: preferences.client?.autoCheckUpdate ?? true,
      },
    },
    ...(hasFields(agentPreferences) ? { agentPreferences } : {}),
    ...(standaloneData && (standaloneData.modelFavorites?.length || standaloneData.tarvenRules?.length)
      ? { standaloneData }
      : {}),
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
 * API 26–28 的储存空间权限由原生文件保存所有者处理。
 * @returns 保存结果（含展示文件名）
 */
export async function exportConfigBackup(
  domains: BackupDomains,
  operationId: string = `config_export_${crypto.randomUUID()}`,
  groupPreferences: Record<string, GroupLocalSettings> = {},
  agentPreferences: Record<string, AgentLocalPreferences> = {},
  standaloneData?: StandaloneData,
): Promise<DownloadsSaveResultDto> {
  if (!isAndroid && !isIOS) {
    throw new Error('当前预览环境不提供原生配置文件导出');
  }
  const payload = buildBackupPayload(
    domains,
    captureBackupPreferences(),
    new Date(),
    groupPreferences,
    agentPreferences,
    standaloneData,
  );
  const bytes = new TextEncoder().encode(JSON.stringify(payload, null, 2));
  if (bytes.byteLength > MAX_BACKUP_FILE_BYTES) {
    throw new Error('配置备份超过 1 MiB，请检查配置字段');
  }
  const contentBase64 = bytesToBase64(bytes);
  const fileName = buildBackupFileName();

  return withNativeFileOperation(operationId, reservationToken =>
    invokeSaveToDownloads(fileName, contentBase64, operationId, reservationToken));
}

/** Native staging is released after Rust has returned the validated patch. */
export async function pickConfigBackup(operationId: string): Promise<ParsedConfigBackup> {
  if (!isAndroid && !isIOS) {
    throw new Error('当前预览环境不提供原生配置文件导入');
  }
  const picked = await pickFile('config', { operationId, maxBytes: MAX_BACKUP_FILE_BYTES });
  try {
    const parsed = await invoke<RawParsedConfigBackup>('parse_config_backup', { path: picked.path });
    return normalizeParsedConfigBackup(parsed);
  } finally {
    await deleteTempFile(picked.path).catch(console.warn);
  }
}

/**
 * 完整配置导入入口，包含空机导入阶段一的暂存处理。
 */
export async function applyImport(
  backup: ParsedConfigBackup,
  customApplySettings?: (
    changes: Partial<SettingsFieldValues>,
    groupPreferences: Record<string, GroupLocalSettingsPatch>,
    agentPreferences?: Record<string, AgentLocalSettingsPatch>,
    standaloneData?: StandaloneDataImport,
  ) => Promise<{ missingGroups?: boolean; missingAgents?: boolean } | boolean>,
): Promise<BackupImportResult> {
  const baseHandler = customApplySettings ?? (async (changes, groupPreferences, agentPreferences, standaloneData) => {
    const settingsStore = useSettingsStore();
    const snapshot = settingsStore.snapshot ?? (await settingsStore.fetchSettings().then(() => settingsStore.snapshot));
    if (!snapshot) throw new Error('配置尚未加载');
    const importRes = await settingsStore.importBackup(
      buildSettingsUpdate(snapshot, changes),
      groupPreferences,
      agentPreferences,
      standaloneData,
    );
    const { result: saveResult } = importRes;
    if (saveResult.status !== 'saved') {
      const issues = saveResult.status === 'rejected' ? saveResult.issues : saveResult.conflicts;
      throw new Error(issues.map(issue => `${issue.domain}.${issue.field}: ${issue.message}`).join('\n'));
    }

    if (Object.keys(groupPreferences).length || (agentPreferences && Object.keys(agentPreferences).length)) {
      try {
        await useAssistantStore().fetchAgentsAndGroups();
      } catch {
        // ignore
      }
    }

    return importRes;
  });

  const wrappedHandler = async (
    changes: Partial<SettingsFieldValues>,
    groupPreferences: Record<string, GroupLocalSettingsPatch>,
    agentPreferences?: Record<string, AgentLocalSettingsPatch>,
    standaloneData?: StandaloneDataImport,
  ) => {
    const applyRes = await baseHandler(changes, groupPreferences, agentPreferences, standaloneData);
    let missingGroups = false;
    let missingAgents = false;
    if (typeof applyRes === 'boolean') {
      missingGroups = applyRes;
    } else if (applyRes && typeof applyRes === 'object') {
      missingGroups = Boolean(applyRes.missingGroups ?? (applyRes as any).missing_groups);
      missingAgents = Boolean(applyRes.missingAgents ?? (applyRes as any).missing_agents);
    }

    if (missingGroups || missingAgents) {
      writeStagedEntityBackup({
        agentPreferences: backup.agentPreferences ?? {},
        groupPreferences: backup.groupPreferences ?? {},
      });
      useNotificationStore().addNotification({
        type: 'info',
        title: '实体配置已暂存',
        message: '部分智能体或群聊尚未同步到本机，已暂存待同步后一键恢复',
        toastOnly: true,
      });
    }

    return applyRes;
  };

  return await applyConfigBackup(backup, wrappedHandler);
}
