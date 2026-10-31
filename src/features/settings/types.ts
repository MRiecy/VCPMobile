import type { ThemePreferences } from "../../core/stores/theme";
import type { ChatPreferences } from "../chat/preferences/model";
import type { GroupLocalSettings, GroupLocalSettingsPatch } from "../../core/types/assistant";
import type { AppSettings } from "../../core/stores/settings";

export const STAGED_BACKUP_STORAGE_KEY = "vcp_staged_entity_backup";
export const STAGED_BACKUP_TTL_MS = 7 * 24 * 60 * 60 * 1000; // 7 days

export type PartialConfig<T> = {
  [K in keyof T]?: T[K] extends readonly unknown[]
    ? T[K]
    : T[K] extends object
    ? PartialConfig<T[K]>
    : T[K];
};

export type BackupDomains = PartialConfig<AppSettings>;

export interface AgentVoiceConfig {
  enabled: boolean;
  mode: "builtin" | "clone";
  voice: string;
  cloneSampleId?: string;
  stream: boolean;
}

export interface AgentLocalPreferences {
  mobileSystemPrompt?: string;
  useTemperature?: boolean;
  voice?: AgentVoiceConfig;
}

export type AgentLocalSettings = AgentLocalPreferences;
export type AgentLocalSettingsPatch = Partial<AgentLocalPreferences>;

export interface TarvenRule {
  id: string;
  name: string;
  ruleType: string;
  isEnabled: boolean;
  content: string;
  scope: string;
  wrap: boolean;
  role?: string | null;
  depth?: number | null;
  position?: string | null;
  sortOrder: number;
}

export interface StandaloneData {
  modelFavorites: string[];
  tarvenRules: TarvenRule[];
}

export type StandaloneDataImport = Partial<StandaloneData>;

export interface ClientPreferences {
  theme: ThemePreferences;
  chat: ChatPreferences;
  topic?: {
    sortMode: "created" | "updated";
    pinnedTopicKeys: string[];
  };
  diary?: {
    hiddenFolders: string[];
    folderOrder: string[];
    collapsedCategories: ("diary" | "cluster")[];
  };
  client?: {
    autoCheckUpdate: boolean;
  };
}

export interface ConfigBackupPayload {
  app: "vcp-mobile";
  kind: "vcp-mobile-settings-backup";
  version: number;
  exportedAt: string;
  metadata?: {
    appVersion?: string;
    platform?: "android" | "ios";
    deviceId?: string;
  };
  domains: BackupDomains;
  preferences: ClientPreferences;
  agentPreferences?: Record<string, AgentLocalPreferences>;
  groupPreferences?: Record<string, GroupLocalSettings>;
  standaloneData?: StandaloneData;
}

export type ConfigBackupFile = ConfigBackupPayload;

export interface ParsedConfigBackup {
  domains: BackupDomains;
  preferences?: {
    theme?: Partial<ThemePreferences>;
    chat?: Partial<ChatPreferences>;
    topic?: {
      sortMode?: "created" | "updated";
      pinnedTopicKeys?: string[];
    };
    diary?: {
      hiddenFolders?: string[];
      folderOrder?: string[];
      collapsedCategories?: ("diary" | "cluster")[];
    };
    client?: {
      autoCheckUpdate?: boolean;
    };
  };
  agentPreferences?: Record<string, AgentLocalSettingsPatch>;
  groupPreferences?: Record<string, GroupLocalSettingsPatch>;
  standaloneData?: {
    modelFavorites?: string[];
    tarvenRules?: TarvenRule[];
  };
  exportedAt: string;
}

export type RawParsedConfigBackup = Omit<ParsedConfigBackup, "preferences"> & { preferences?: unknown };

export interface BackupImportResult {
  applied: string[];
  skipped: string[];
  failed: { scope: string; message: string }[];
}

export interface StagedEntityBackupPayload {
  timestamp: number;
  version: number;
  agentPreferences?: Record<string, AgentLocalSettingsPatch>;
  groupPreferences?: Record<string, GroupLocalSettingsPatch>;
}
