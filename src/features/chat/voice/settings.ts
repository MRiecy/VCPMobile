export type VoiceLanguage = 'auto' | 'zh' | 'en';
export type VoiceInputEncoding = 'wav' | 'mp3_64';
export type VoiceVadSensitivity = 'low' | 'standard' | 'high';
export type VoiceTtsMode = 'builtin' | 'clone';
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
  voiceTtsMode: VoiceTtsMode;
  voiceTtsStylePrompt: string;
  voiceTtsCloneSampleId: string;
  voiceAudioTagRules: string;
  voiceAutoGuidance: boolean;
  voiceGuidanceRules: string;
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
  voiceTtsMode: 'builtin',
  voiceTtsStylePrompt: '',
  voiceTtsCloneSampleId: '',
  voiceAudioTagRules: '回复将用于语音通话。保持当前角色和正常对话内容，保留已选音色，只控制情绪、语气和节奏；只在确有表达需要时添加简短音频标签。整体风格写在正文开头的半角圆括号内，例如（温柔 平静）应写为 (温柔 平静)；局部声音或情绪写在对应文字前的方括号内，例如 [轻笑]、[叹气]、[低声]。标签使用自然语言，完整闭合，不解释标签，不使用唱歌模式，不把整句正文放入标签。',
  voiceAutoGuidance: true,
  voiceGuidanceRules: '在每条助手回复的正文之前，为本条回复生成一份自然语言风格指导。依据当前角色、对话场景和情绪，描述语气、语速与必要的停顿；保留已选预置音色或参考样本的声线，不重新设计声音。用一至四句清晰描述，避免互相矛盾的要求，不要求混响、回声等后期效果。指导只描述如何说，不重复待朗读的正文。',
  voiceVadSensitivity: 'standard',
  voiceVadEndSilenceMs: 512,
  voiceVadPlaybackOnsetMs: 192,
};

export const MIMO_TTS_MODES = [
  { value: 'builtin', label: '预置音色', model: 'mimo-v2.5-tts' },
  { value: 'clone', label: '样本复刻', model: 'mimo-v2.5-tts-voiceclone' },
] as const;

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
