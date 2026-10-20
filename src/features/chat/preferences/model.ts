export const CHAT_PRESENTATION_MODES = ['bubble', 'panel', 'immersive'] as const;
export type ChatPresentationMode = (typeof CHAT_PRESENTATION_MODES)[number];

export interface ChatPresentationOption {
  value: ChatPresentationMode;
  label: string;
  title: string;
  description: string;
}

export const CHAT_PRESENTATION_OPTIONS: readonly ChatPresentationOption[] = [
  { value: 'bubble', label: '气泡', title: '气泡模式', description: '保留左右气泡、头像与紧凑内容宽度。' },
  { value: 'panel', label: '统一', title: '统一模式', description: '消息共用连续全宽表面，以细分隔线区分。' },
  { value: 'immersive', label: '刊物', title: '刊物模式', description: '隐藏头像，使用居中的长文阅读宽度。' },
];

export interface ChatPreferences {
  readonly presentationMode: ChatPresentationMode;
  readonly smoothStreamingEnabled: boolean;
  readonly messageHapticsEnabled: boolean;
  readonly collapseToolsWhileStreaming: boolean;
}

export const CHAT_PREFERENCE_DEFAULTS: ChatPreferences = {
  presentationMode: 'bubble',
  smoothStreamingEnabled: false,
  messageHapticsEnabled: true,
  collapseToolsWhileStreaming: false,
};

export interface PresentationChangeResult {
  ok: boolean;
  changed: boolean;
  mode: ChatPresentationMode;
  error?: string;
}

export interface BooleanPreferenceChangeResult {
  ok: boolean;
  changed: boolean;
  enabled: boolean;
  error?: string;
}

export function normalizeChatPresentationMode(value: unknown): ChatPresentationMode {
  return CHAT_PRESENTATION_MODES.includes(value as ChatPresentationMode)
    ? value as ChatPresentationMode
    : CHAT_PREFERENCE_DEFAULTS.presentationMode;
}

export function normalizeChatBoolean(raw: string | null, fallback: boolean): boolean {
  return raw === 'true' ? true : raw === 'false' ? false : fallback;
}

/** JSON 导入只接受真实布尔值；localStorage 的字符串解码仍归 storage。 */
export function normalizeChatPreferencePatch(value: unknown): Partial<ChatPreferences> {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    return { ...CHAT_PREFERENCE_DEFAULTS };
  }
  const source = value as Record<string, unknown>;
  const patch: { -readonly [K in keyof ChatPreferences]?: ChatPreferences[K] } = {};
  for (const key of Object.keys(CHAT_PREFERENCE_DEFAULTS) as (keyof ChatPreferences)[]) {
    if (!Object.prototype.hasOwnProperty.call(source, key)) continue;
    if (key === 'presentationMode') {
      patch.presentationMode = normalizeChatPresentationMode(source[key]);
    } else {
      const candidate = source[key];
      patch[key] = typeof candidate === 'boolean' ? candidate : CHAT_PREFERENCE_DEFAULTS[key];
    }
  }
  return patch;
}
