import { defineStore } from 'pinia';
import { onScopeDispose, ref, shallowRef, watch } from 'vue';
import { useChatSessionStore, type ConversationKey } from '../../../core/stores/chatSessionStore';
import { useAppLifecycleStore } from '../../../core/stores/appLifecycle';
import { createMicrophoneMonitor } from './microphoneMonitor';

export type MicrophoneState = 'off' | 'requesting' | 'on' | 'error';

export const useVoiceCallStore = defineStore('voiceCall', () => {
  const session = useChatSessionStore();
  const lifecycle = useAppLifecycleStore();
  const active = ref(false);
  const conversation = shallowRef<ConversationKey | null>(null);
  const microphoneState = ref<MicrophoneState>('off');
  const level = ref(0);
  const bands = shallowRef<readonly number[]>([]);
  const errorMessage = ref('');
  let microphoneGeneration = 0;

  const monitor = createMicrophoneMonitor(
    (value, spectrum) => { level.value = value; bands.value = spectrum; },
    () => {
      microphoneGeneration += 1;
      if (!active.value) return;
      microphoneState.value = 'error';
      errorMessage.value = '麦克风已被系统中断，点击麦克风重试。';
    },
  );

  const stopMicrophone = () => {
    microphoneGeneration += 1;
    monitor.stop();
    microphoneState.value = 'off';
    errorMessage.value = '';
  };

  const enableMicrophone = async () => {
    if (!active.value || lifecycle.isBackground) return;
    const generation = ++microphoneGeneration;
    microphoneState.value = 'requesting';
    errorMessage.value = '';
    try {
      const started = await monitor.start();
      if (generation !== microphoneGeneration || !active.value) return;
      microphoneState.value = started ? 'on' : 'off';
    } catch (error) {
      if (generation !== microphoneGeneration || !active.value) return;
      microphoneState.value = 'error';
      const name = error instanceof Error ? error.name : '';
      errorMessage.value = name === 'NotAllowedError' || name === 'SecurityError'
        ? '麦克风权限未开启，请在系统设置中允许后重试。'
        : name === 'NotFoundError'
          ? '未找到可用麦克风。'
          : name === 'NotReadableError'
            ? '麦克风暂时不可用，请检查是否被其他应用占用。'
            : error instanceof Error ? error.message : '麦克风启动失败，请重试。';
    }
  };

  const start = () => {
    if (active.value || !session.currentConversationKey) return;
    conversation.value = { ...session.currentConversationKey };
    active.value = true;
    void enableMicrophone();
  };

  const end = () => {
    stopMicrophone();
    active.value = false;
    conversation.value = null;
  };

  const toggleMicrophone = () => {
    if (!active.value) return;
    if (microphoneState.value === 'on' || microphoneState.value === 'requesting') stopMicrophone();
    else void enableMicrophone();
  };

  watch(() => session.currentConversationKey, () => {
    if (active.value && !session.isConversationCurrent(conversation.value)) end();
  }, { flush: 'sync' });
  // Keep the scene when backgrounded, but release hardware. Resuming requires
  // an explicit microphone tap; no hidden re-acquisition or permission prompt.
  watch(() => lifecycle.isBackground, background => {
    if (background && active.value) stopMicrophone();
  }, { flush: 'sync' });
  onScopeDispose(end);

  return { active, microphoneState, level, bands, errorMessage, start, end, toggleMicrophone };
});
