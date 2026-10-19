import type { SettingsFieldValues, SettingsFieldPath } from "../../../core/config/fields";

export const IDENTITY_FIELDS = [
  "identity.userName",
  "server.adminUsername",
  "server.adminPassword",
] as const;

export const CONNECTION_FIELDS = [
  "server.url",
  "server.endpointMode",
  "server.apiKey",
  "log.url",
  "log.key",
  "sync.serverUrl",
  "sync.httpUrl",
  "sync.token",
  "server.fileKey",
] as const;

export const SUMMARY_FIELDS = ["conversation.topicSummaryModel"] as const;

export const VOICE_FIELDS = [
  "voice.connection.apiBaseUrl", "voice.connection.apiKey", "voice.asr.inputEncoding", "voice.asr.stream", "voice.tts.stream", "voice.asr.language", "voice.tts.voice",
  "voice.vad.sensitivity", "voice.vad.endSilenceMs", "voice.vad.playbackOnsetMs",
  "voice.tts.mode", "voice.tts.stylePrompt", "voice.tts.cloneSampleId",
] as const;

export const VOICE_STYLE_GUIDANCE_FIELDS = ["voice.guidance.audioTagRules", "voice.guidance.autoGuidance", "voice.guidance.guidanceRules"] as const;

export const JEV_FIELDS = [
  'jev.enabled', 'jev.provider', 'jev.typesafe.apiUrl', 'jev.typesafe.apiKey', 'jev.typesafe.model',
  'jev.openrouter.apiUrl', 'jev.openrouter.apiKey', 'jev.openrouter.model',
  'jev.timeoutMs', 'jev.maxRetries', 'jev.retryBaseDelayMs', 'jev.proxyUrl', 'jev.httpReferer', 'jev.appTitle',
] as const;

export type SettingsEditableKey = SettingsFieldPath;

export type IdentityFields = Pick<SettingsFieldValues, (typeof IDENTITY_FIELDS)[number]>;
export type ConnectionFields = Pick<SettingsFieldValues, (typeof CONNECTION_FIELDS)[number]>;
export type SummaryFields = Pick<SettingsFieldValues, (typeof SUMMARY_FIELDS)[number]>;
export type VoiceFields = Pick<SettingsFieldValues, (typeof VOICE_FIELDS)[number]>;
