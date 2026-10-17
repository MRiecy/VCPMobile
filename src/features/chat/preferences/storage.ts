import { readLocalPreference, writeLocalPreference } from '../../../core/preferences/localPreference';
import {
  CHAT_PREFERENCE_DEFAULTS,
  normalizeChatBoolean,
  normalizeChatPresentationMode,
  type ChatPreferences,
} from './model';

const KEYS: Record<keyof ChatPreferences, string> = {
  presentationMode: 'vcp-chat-presentation-mode',
  smoothStreamingEnabled: 'vcp-smooth-streaming-enabled',
  messageHapticsEnabled: 'vcp-message-haptics-enabled',
  collapseToolsWhileStreaming: 'vcp-collapse-tools-while-streaming',
};

function restore<K extends keyof ChatPreferences>(
  key: K,
  decode: (raw: string | null) => ChatPreferences[K],
): ChatPreferences[K] {
  const result = readLocalPreference(KEYS[key]);
  if (!result.ok) {
    console.warn(`[ChatPreferences] Failed to read ${key}:`, result.error);
    return CHAT_PREFERENCE_DEFAULTS[key];
  }
  const value = decode(result.raw);
  // Missing preferences keep their default without writing; malformed values
  // retain the previous rendering preferences' best-effort repair behavior.
  if (result.raw !== null && result.raw !== String(value)) {
    const repaired = writeChatPreference(key, value);
    if (!repaired.ok) console.warn(`[ChatPreferences] Failed to repair ${key}:`, repaired.error);
  }
  return value;
}

export function readChatPreferences(): ChatPreferences {
  return {
    presentationMode: restore('presentationMode', normalizeChatPresentationMode),
    smoothStreamingEnabled: restore('smoothStreamingEnabled', (raw) =>
      normalizeChatBoolean(raw, CHAT_PREFERENCE_DEFAULTS.smoothStreamingEnabled)),
    messageHapticsEnabled: restore('messageHapticsEnabled', (raw) =>
      normalizeChatBoolean(raw, CHAT_PREFERENCE_DEFAULTS.messageHapticsEnabled)),
    collapseToolsWhileStreaming: restore('collapseToolsWhileStreaming', (raw) =>
      normalizeChatBoolean(raw, CHAT_PREFERENCE_DEFAULTS.collapseToolsWhileStreaming)),
  };
}

export function writeChatPreference<K extends keyof ChatPreferences>(key: K, value: ChatPreferences[K]) {
  return writeLocalPreference(KEYS[key], String(value));
}
