import { acceptHMRUpdate, defineStore } from 'pinia';
import { readonly, ref, type Ref } from 'vue';
import {
  normalizeChatPresentationMode,
  type BooleanPreferenceChangeResult,
  type ChatPreferences,
  type PresentationChangeResult,
} from './model';
import { readChatPreferences, writeChatPreference } from './storage';

export const useChatPreferencesStore = defineStore('chatPreferences', () => {
  const restored = readChatPreferences();
  const presentationMode = ref(restored.presentationMode);
  const smoothStreamingEnabled = ref(restored.smoothStreamingEnabled);
  const messageHapticsEnabled = ref(restored.messageHapticsEnabled);
  const collapseToolsWhileStreaming = ref(restored.collapseToolsWhileStreaming);

  const setPresentationMode = (candidate: unknown): PresentationChangeResult => {
    const mode = normalizeChatPresentationMode(candidate);
    if (presentationMode.value === mode) return { ok: true, changed: false, mode };
    const saved = writeChatPreference('presentationMode', mode);
    if (!saved.ok) {
      return { ok: false, changed: false, mode: presentationMode.value, error: `消息呈现设置保存失败：${saved.error}` };
    }
    presentationMode.value = mode;
    return { ok: true, changed: true, mode };
  };

  const setBoolean = (
    key: Exclude<keyof ChatPreferences, 'presentationMode'>,
    state: Ref<boolean>,
    enabled: boolean,
    label: string,
  ): BooleanPreferenceChangeResult => {
    if (state.value === enabled) return { ok: true, changed: false, enabled };
    const saved = writeChatPreference(key, enabled);
    if (!saved.ok) {
      return { ok: false, changed: false, enabled: state.value, error: `${label}设置保存失败：${saved.error}` };
    }
    state.value = enabled;
    return { ok: true, changed: true, enabled };
  };

  const setSmoothStreamingEnabled = (enabled: boolean) =>
    setBoolean('smoothStreamingEnabled', smoothStreamingEnabled, enabled, '流式平滑动效');
  const setMessageHapticsEnabled = (enabled: boolean) =>
    setBoolean('messageHapticsEnabled', messageHapticsEnabled, enabled, '消息震动');
  const setCollapseToolsWhileStreaming = (enabled: boolean) =>
    setBoolean('collapseToolsWhileStreaming', collapseToolsWhileStreaming, enabled, '流式工具折叠');

  return {
    presentationMode: readonly(presentationMode),
    smoothStreamingEnabled: readonly(smoothStreamingEnabled),
    messageHapticsEnabled: readonly(messageHapticsEnabled),
    collapseToolsWhileStreaming: readonly(collapseToolsWhileStreaming),
    setPresentationMode,
    setSmoothStreamingEnabled,
    setMessageHapticsEnabled,
    setCollapseToolsWhileStreaming,
  };
});

if (import.meta.hot) {
  import.meta.hot.accept(acceptHMRUpdate(useChatPreferencesStore, import.meta.hot));
}
