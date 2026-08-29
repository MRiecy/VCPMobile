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
  avatarCalculatedColor: string | null;
  topics: TopicDto[];
}

export interface AgentListItemDto {
  id: string;
  name: string;
  model: string;
  avatarCalculatedColor: string | null;
}

export interface GroupConfigDto {
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
