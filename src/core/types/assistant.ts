import type { VoiceTtsMode } from "../config/generated/config";

export type OwnerType = "agent" | "group";

export interface TopicDto {
  id: string;
  name: string;
  createdAt: number;
  updatedAt?: number;
  locked: boolean;
  unread: boolean;
  unreadCount: number;
  msgCount: number;
  ownerId: string;
  ownerType: OwnerType;
}

export interface AgentConfigDto {
  id: string;
  name: string;
  systemPrompt: string;
  mobileSystemPrompt: string;
  model: string;
  temperature: number;
  contextTokenLimit: number;
  maxOutputTokens: number;
  streamOutput: boolean;
  useTemperature: boolean;
  voice: AgentVoiceConfigDto;
  avatarCalculatedColor: string | null;
  topics: TopicDto[];
}

export interface AgentVoiceConfigDto {
  enabled: boolean;
  mode: VoiceTtsMode;
  voice: string;
  cloneSampleId: string;
  stream: boolean;
}

export interface AgentListItemDto {
  id: string;
  name: string;
  model: string;
  avatarCalculatedColor: string | null;
}

export type GroupMode = "sequential" | "naturerandom" | "invite_only" | "jev";

export interface JevModeSettings {
  navigatorPrompt: string;
  speakerThreshold: number;
  continueThreshold: number;
  stopThreshold: number;
  maxSpeakersPerRound: number;
  maxAutonomousRounds: number;
  historyWindow: number;
  continueDebounceMs: number;
  fallbackPolicy: "stop" | "top_speaker";
  memberStyles: Record<string, string>;
}

export interface GroupModeSettings {
  sequential: { speakerOrder: string[] };
  jev: JevModeSettings;
}

export interface GroupLocalSettings {
  chatMode: GroupMode;
  modeSettings: GroupModeSettings;
  enableContextMessageWindow: boolean;
  contextMessageWindowSize: number;
}

/** 备份按字段恢复；数组和 memberStyles 映射若出现则整体替换。 */
export type GroupLocalSettingsPatch = Partial<Omit<GroupLocalSettings, "modeSettings">> & {
  modeSettings?: {
    sequential?: Partial<GroupModeSettings["sequential"]>;
    jev?: Partial<JevModeSettings>;
  };
};

export interface GroupConfigDto extends GroupLocalSettings {
  id: string;
  name: string;
  avatarCalculatedColor: string | null;
  members: string[];
  mode: string;
  memberTags: Record<string, string> | null;
  groupPrompt: string | null;
  invitePrompt: string | null;
  useUnifiedModel: boolean;
  unifiedModel: string | null;
  topics: TopicDto[];
  tagMatchMode: string | null;
  createdAt: number;
}

export interface GroupListItemDto {
  id: string;
  name: string;
  avatarCalculatedColor: string | null;
  members: string[];
  mode: string;
}

export interface AssistantsSnapshotDto {
  agents: AgentListItemDto[];
  groups: GroupListItemDto[];
  unreadCounts: Record<string, number>;
}

export type AssistantListItem =
  | (AgentListItemDto & { type: "agent" })
  | (GroupListItemDto & { type: "group" });

export interface ConversationOwnerItem {
  id: string;
  name?: string;
  type: OwnerType;
  avatarCalculatedColor?: string | null;
}
