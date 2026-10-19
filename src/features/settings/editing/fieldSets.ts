import type { AppSettings } from "../../../core/stores/settings";

export const IDENTITY_FIELDS = [
  "userName",
  "adminUsername",
  "adminPassword",
] as const;

export const CONNECTION_FIELDS = [
  "vcpServerUrl",
  "chatEndpointMode",
  "vcpApiKey",
  "vcpLogUrl",
  "vcpLogKey",
  "syncServerUrl",
  "syncHttpUrl",
  "syncToken",
  "fileKey",
] as const;

export const SUMMARY_FIELDS = ["topicSummaryModel"] as const;

export const VOICE_FIELDS = [
  "voiceApiBaseUrl", "voiceApiKey", "voiceInputEncoding", "voiceAsrStream", "voiceTtsStream", "voiceLanguage", "voiceTtsVoice",
  "voiceVadSensitivity", "voiceVadEndSilenceMs", "voiceVadPlaybackOnsetMs",
] as const;

export type SettingsEditableKey =
  | (typeof IDENTITY_FIELDS)[number]
  | (typeof CONNECTION_FIELDS)[number]
  | (typeof SUMMARY_FIELDS)[number]
  | (typeof VOICE_FIELDS)[number];

export type IdentityFields = Pick<AppSettings, (typeof IDENTITY_FIELDS)[number]>;
export type ConnectionFields = Pick<AppSettings, (typeof CONNECTION_FIELDS)[number]>;
export type SummaryFields = Pick<AppSettings, (typeof SUMMARY_FIELDS)[number]>;
export type VoiceFields = Pick<AppSettings, (typeof VOICE_FIELDS)[number]>;
