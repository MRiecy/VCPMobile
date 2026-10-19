export type VoiceLanguage = 'auto' | 'zh' | 'en';
export type VoiceInputEncoding = 'wav' | 'mp3_64';
export type VoiceVadSensitivity = 'low' | 'standard' | 'high';
export type VoiceTtsVoice = 'mimo_default' | '冰糖' | '茉莉' | '苏打' | '白桦' | 'Mia' | 'Chloe' | 'Milo' | 'Dean';

/** These fields belong to the committed AppSettings snapshot in SQLite. */
export interface VoiceServiceSettings {
  voiceApiBaseUrl: string;
  voiceApiKey: string;
  voiceInputEncoding: VoiceInputEncoding;
  voiceAsrStream: boolean;
  voiceTtsStream: boolean;
  voiceLanguage: VoiceLanguage;
  voiceTtsVoice: VoiceTtsVoice;
  voiceVadSensitivity: VoiceVadSensitivity;
  voiceVadEndSilenceMs: number;
  voiceVadPlaybackOnsetMs: number;
}

export const DEFAULT_VOICE_SETTINGS: Readonly<VoiceServiceSettings> = {
  voiceApiBaseUrl: 'https://api.xiaomimimo.com/v1',
  voiceApiKey: '',
  voiceInputEncoding: 'wav',
  voiceAsrStream: true,
  voiceTtsStream: true,
  voiceLanguage: 'auto',
  voiceTtsVoice: 'mimo_default',
  voiceVadSensitivity: 'standard',
  voiceVadEndSilenceMs: 512,
  voiceVadPlaybackOnsetMs: 192,
};

// IDs are the official API values, including the four Chinese voice IDs.
export const MIMO_VOICES: ReadonlyArray<{ value: VoiceTtsVoice; label: string }> = [
  { value: 'mimo_default', label: 'MiMo 默认' },
  { value: '冰糖', label: '冰糖 · 中文女声' },
  { value: '茉莉', label: '茉莉 · 中文女声' },
  { value: '苏打', label: '苏打 · 中文男声' },
  { value: '白桦', label: '白桦 · 中文男声' },
  { value: 'Mia', label: 'Mia · 英文女声' },
  { value: 'Chloe', label: 'Chloe · 英文女声' },
  { value: 'Milo', label: 'Milo · 英文男声' },
  { value: 'Dean', label: 'Dean · 英文男声' },
];
