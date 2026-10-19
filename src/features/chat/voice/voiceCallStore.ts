import { defineStore } from 'pinia';
import { onScopeDispose, ref, shallowRef, watch } from 'vue';
import { useChatSessionStore, type ConversationKey } from '../../../core/stores/chatSessionStore';
import { useAppLifecycleStore } from '../../../core/stores/appLifecycle';
import { createMicrophoneMonitor, type VoiceAudioFrame } from './microphoneMonitor';
import { createVoiceConversation, type VoiceCallSnapshot, type VoiceServiceView } from './voiceConversation';

export type MicrophoneState = 'off' | 'requesting' | 'on' | 'error';

const AUDIO_ERRORS: ReadonlyArray<readonly [RegExp, string]> = [
  [/\bmicrophone_permission_denied\b/, '麦克风权限未开启，请在系统设置中允许后重试。'],
  [/\b(voice_model_unavailable|vad_resources_unavailable|vad_initialization_failed)\b/, '本地人声检测模型暂不可用，请重新开启麦克风。'],
  [/\b(vad_inference_failed|vad_output_missing|vad_output_shape_invalid|vad_probability_invalid)\b/, '本地人声检测已中断，请重新开启麦克风。'],
  [/\baudio_focus_lost\b/, '音频已被其他应用接管，点击麦克风重新开启。'],
  [/\bmicrophone_silenced\b/, '麦克风已被系统暂停，请检查是否被其他应用占用。'],
  [/\bbluetooth_disconnected\b/, '蓝牙音频已断开，确认音频路线后重新开启麦克风。'],
  [/\baudio_interrupted\b/, '音频已被系统中断，点击麦克风重新开启。'],
  [/\b(audio_route_changed|audio_configuration_changed|route_changed|route-change)\b/, '音频路线已改变，请重新开启麦克风。'],
  [/\b(audio_services_lost|audio_services_reset)\b/, '系统音频服务已重置，请重新开启麦克风。'],
  [/\b(voice_session_busy|voice_audio_busy|audio_session_busy)\b/, '麦克风正在切换，请稍后重试。'],
  [/\bmicrophone_not_ready\b/, '麦克风尚未就绪，请先重新开启麦克风。'],
  [/\bvoice_requires_foreground\b/, '请保持应用在前台，再开启麦克风。'],
  [/\bvoice_events_unavailable\b/, '语音事件连接暂不可用，请重新打开应用。'],
  [/\b(voice_processing_failed|voice_processing_unavailable)\b/, '系统语音处理暂不可用，请检查音频设备后重试。'],
  [/\b(microphone_format_unavailable|audio_format_unavailable|audio_converter_unavailable)\b/, '当前音频设备的采集格式不可用，请切换设备后重试。'],
  [/\b(audio_input_discontinuity|audio_conversion_failed|audio_sample_invalid|audio_event_backpressure)\b/, '音频处理已中断，请重新开启麦克风。'],
  [/\bvoice_probe_busy\b/, '本地样本正在准备，请稍后重试。'],
  [/\bvoice_playback_busy\b/, '正在朗读，请等待结束或重新开麦后播放本地样本。'],
  [/\bvoice_call_not_active\b/, '通话会话已结束，请重新进入语音模式。'],
  [/\bvoice_call_busy\b/, '上一通话正在结束，请稍后重试。'],
  [/\bvoice_segment_retention_busy\b/, '前面的语音仍在识别，请稍后再开启麦克风。'],
  [/\baudio_conversion_drain_failed\b/, '末尾音频处理失败，请重新说一遍。'],
  [/\b(voice_probe_missing|voice_probe_invalid)\b/, '本地诊断样本不可用，请检查应用安装。'],
  [/\b(voice_start_failed|audio_start_failed)\b/, '麦克风启动失败，请检查音频设备后重试。'],
  [/\bvoice_capture_failed\b/, '音频采集已中断，点击麦克风重新开启。'],
  [/\bvoice_probe_failed\b/, '本地样本播放失败，请重试。'],
  [/\bunsupported\b/, '当前环境不支持原生语音，请在 Android 或 iOS 客户端使用。'],
];

const errorDetail = (error: unknown) => error instanceof Error ? error.message : String(error);
const audioErrorMessage = (error: unknown, fallback: string) => {
  const detail = errorDetail(error);
  // Native details can contain ORT/NSError stacks. Keep a bounded diagnostic
  // in the console and expose only stable, actionable messages in the product.
  console.warn('[VoiceAudio]', detail.slice(0, 512));
  return AUDIO_ERRORS.find(([code]) => code.test(detail))?.[1] ?? fallback;
};

export const useVoiceCallStore = defineStore('voiceCall', () => {
  const session = useChatSessionStore();
  const lifecycle = useAppLifecycleStore();
  const active = ref(false);
  const conversation = shallowRef<ConversationKey | null>(null);
  const microphoneState = ref<MicrophoneState>('off');
  const level = ref(0);
  const bands = shallowRef<readonly number[]>([]);
  const speechActive = ref(false);
  const aecState = ref<VoiceAudioFrame['aecState'] | null>(null);
  const route = ref('');
  const playbackActive = ref(false);
  const probeActive = ref(false);
  const probePending = ref(false);
  const probeError = ref('');
  const errorMessage = ref('');
  const serviceState = ref<VoiceServiceView['state']>('off');
  const serviceMessage = ref('');
  const partialText = ref('');
  const inputState = ref<VoiceCallSnapshot['inputState']>('idle');
  const replyState = ref<VoiceCallSnapshot['replyState']>('idle');
  const ttsState = ref<VoiceCallSnapshot['ttsState']>('idle');
  let microphoneGeneration = 0;
  let probeGeneration = 0;

  const resetAnalysis = () => {
    speechActive.value = false;
    aecState.value = null;
    route.value = '';
    playbackActive.value = ttsState.value === 'speaking';
    probeActive.value = false;
    probePending.value = false;
    probeError.value = '';
    probeGeneration += 1;
  };

  const voiceConversation = createVoiceConversation({
    conversation: () => conversation.value ?? session.currentConversationKey,
    isConversationCurrent: key => session.isConversationCurrent(key),
    onView: view => {
      serviceState.value = view.state;
      serviceMessage.value = view.message;
      partialText.value = view.partialText;
    },
    onSnapshot: snapshot => {
      active.value = snapshot?.active ?? false;
      conversation.value = snapshot?.active && session.currentConversationKey
        ? { ...session.currentConversationKey } : null;
      inputState.value = snapshot?.inputState ?? 'idle';
      replyState.value = snapshot?.replyState ?? 'idle';
      ttsState.value = snapshot?.ttsState ?? 'idle';
      if (!snapshot?.active || snapshot.micState === 'closed') {
        // The start intent owns the permission-prompt display until it settles.
        if (microphoneState.value !== 'requesting' || !snapshot?.active) {
          microphoneState.value = 'off';
          level.value = 0;
          bands.value = [];
          resetAnalysis();
        }
      } else if (microphoneState.value !== 'requesting') microphoneState.value = 'on';
      playbackActive.value = snapshot?.ttsState === 'speaking';
    },
  });

  const monitor = createMicrophoneMonitor(
    (value, spectrum) => { level.value = value; bands.value = spectrum; },
    event => {
      microphoneGeneration += 1;
      resetAnalysis();
      if (!active.value) return;
      if (event.kind === 'stopped' && (event.reason === 'background' || event.reason === 'requested')) {
        microphoneState.value = 'off';
        errorMessage.value = '';
      } else {
        microphoneState.value = 'error';
        errorMessage.value = audioErrorMessage(event.reason,
          event.kind === 'error' ? '麦克风发生错误，请重新开启。' : '麦克风已被系统中断，点击麦克风重试。');
      }
    },
    frame => {
      speechActive.value = frame.speechActive;
      aecState.value = frame.aecState;
      route.value = frame.route;
      playbackActive.value = frame.playbackActive;
      probeActive.value = frame.playbackActive && !frame.playbackId;
    },
    {
      beforeStart: voiceConversation.start,
      currentLifecycleEpoch: () => lifecycle.lifecycleSnapshot?.transitionEpoch ?? null,
      onConfigured: configured => {
        if (!configured && !['recognizing', 'thinking', 'speaking'].includes(serviceState.value)) {
          serviceState.value = 'local';
          serviceMessage.value = '配置语音服务并重新开麦后，即可识别与朗读。';
        }
      },
    },
  );

  const stopMicrophone = () => {
    microphoneGeneration += 1;
    monitor.stop();
    resetAnalysis();
    microphoneState.value = 'off';
    errorMessage.value = '';
  };

  const enableMicrophone = async () => {
    if (!active.value || lifecycle.isBackground) return;
    const generation = ++microphoneGeneration;
    resetAnalysis();
    microphoneState.value = 'requesting';
    errorMessage.value = '';
    try {
      const started = await monitor.start();
      if (generation !== microphoneGeneration || !active.value) return;
      microphoneState.value = started ? 'on' : 'off';
    } catch (error) {
      if (generation !== microphoneGeneration || !active.value) return;
      resetAnalysis();
      microphoneState.value = 'error';
      errorMessage.value = audioErrorMessage(error, '麦克风启动失败，请重试。');
    }
  };

  const start = () => {
    if (active.value || !session.currentConversationKey) return;
    conversation.value = { ...session.currentConversationKey };
    active.value = true;
    voiceConversation.begin();
    void enableMicrophone();
  };

  const end = () => {
    voiceConversation.stop();
    // Hangup has one Rust owner. Do not race it with a separate mute/drain
    // command that could admit an unsent transcript while hangup cancels it.
    microphoneGeneration += 1;
    monitor.detach();
    resetAnalysis();
    microphoneState.value = 'off';
    errorMessage.value = '';
    active.value = false;
    conversation.value = null;
  };

  const recover = () => voiceConversation.recover();

  const toggleMicrophone = () => {
    if (!active.value) return;
    if (microphoneState.value === 'on' || microphoneState.value === 'requesting') stopMicrophone();
    else void enableMicrophone();
  };

  const setProbe = async (enabled: boolean) => {
    if (enabled && (!active.value || microphoneState.value !== 'on' || lifecycle.isBackground)) return;
    const generation = ++probeGeneration;
    const micGeneration = microphoneGeneration;
    probePending.value = true;
    probeError.value = '';
    try {
      await monitor.setProbe(enabled);
    } catch (error) {
      if (generation !== probeGeneration || micGeneration !== microphoneGeneration) return;
      // Native VAD may cancel a pending playback request when speech starts.
      // That is a successful interruption, not a capture or playback failure.
      if (!/\bcancelled\b/.test(errorDetail(error))) {
        probeError.value = audioErrorMessage(error, '本地样本播放失败，请重试。');
      }
    } finally {
      if (generation === probeGeneration && micGeneration === microphoneGeneration) probePending.value = false;
    }
  };

  watch(() => session.currentConversationKey, () => {
    if (active.value && !session.isConversationCurrent(conversation.value)) end();
  }, { flush: 'sync' });
  // Keep the scene when backgrounded, but stop accepting microphone audio. Resuming requires
  // an explicit microphone tap; no hidden re-acquisition or permission prompt.
  watch(() => lifecycle.isBackground, background => {
    if (background && active.value) stopMicrophone();
  }, { flush: 'sync' });
  onScopeDispose(() => {
    microphoneGeneration += 1;
    monitor.detach();
    voiceConversation.dispose();
  });

  return {
    active, microphoneState, level, bands, speechActive, aecState, route,
    playbackActive, probeActive, probePending, probeError, errorMessage, serviceState, serviceMessage, partialText,
    inputState, replyState, ttsState,
    start, end, toggleMicrophone, setProbe, recover,
  };
});
