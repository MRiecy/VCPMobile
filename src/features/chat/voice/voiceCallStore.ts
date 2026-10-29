import { defineStore } from 'pinia';
import { computed, onScopeDispose, ref, shallowRef, watch } from 'vue';
import { useChatSessionStore, type ConversationKey } from '../../../core/stores/chatSessionStore';
import { useAppLifecycleStore } from '../../../core/stores/appLifecycle';
import { createMicrophoneMonitor } from './microphoneMonitor';
import { createVoiceConversation, type VoiceCallSnapshot, type VoiceServiceView } from './voiceConversation';
import { useVoiceDocumentStore } from './voiceDocument';

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
  [/\bvoice_playback_busy\b/, '正在朗读，请等待结束后重试。'],
  [/\bvoice_call_not_active\b/, '通话会话已结束，请重新进入语音模式。'],
  [/\bvoice_call_busy\b/, '上一通话正在结束，请稍后重试。'],
  [/\bvoice_segment_retention_busy\b/, '前面的语音仍在识别，请稍后再开启麦克风。'],
  [/\baudio_conversion_drain_failed\b/, '末尾音频处理失败，请重新说一遍。'],
  [/\b(voice_start_failed|audio_start_failed)\b/, '麦克风启动失败，请检查音频设备后重试。'],
  [/\bvoice_capture_failed\b/, '音频采集已中断，点击麦克风重新开启。'],
  [/\bunsupported\b/, '当前环境不支持原生语音，请在 Android 或 iOS 客户端使用。'],
];

const errorDetail = (error: unknown) => error instanceof Error ? error.message : String(error);
const audioErrorText = (detail: string, fallback: string) =>
  AUDIO_ERRORS.find(([code]) => code.test(detail))?.[1] ?? fallback;
const audioErrorMessage = (error: unknown, fallback: string) => {
  const detail = errorDetail(error);
  // Native details can contain ORT/NSError stacks. Keep a bounded diagnostic
  // in the console and expose only stable, actionable messages in the product.
  console.warn('[VoiceAudio]', detail.slice(0, 512));
  return audioErrorText(detail, fallback);
};

export const useVoiceCallStore = defineStore('voiceCall', () => {
  const session = useChatSessionStore();
  const lifecycle = useAppLifecycleStore();
  const document = useVoiceDocumentStore();
  const active = ref(false);
  const conversation = shallowRef<ConversationKey | null>(null);
  const captureState = ref<VoiceCallSnapshot['micState']>('closed');
  const microphonePending = ref(false);
  const level = ref(0);
  const bands = shallowRef<readonly number[]>([]);
  const playbackActive = ref(false);
  const errorMessage = ref('');
  const microphoneState = computed<MicrophoneState>(() => {
    if (captureState.value === 'listening') return 'on';
    if (captureState.value === 'starting' || captureState.value === 'stopping' || microphonePending.value) return 'requesting';
    return errorMessage.value ? 'error' : 'off';
  });
  const serviceState = ref<VoiceServiceView['state']>('off');
  const serviceMessage = ref('');
  const partialText = ref('');
  const inputState = ref<VoiceCallSnapshot['inputState']>('idle');
  const replyState = ref<VoiceCallSnapshot['replyState']>('idle');
  const ttsState = ref<VoiceCallSnapshot['ttsState']>('idle');
  let microphoneGeneration = 0;
  let microphoneStopReason: string | null = null;

  const syncPlaybackState = () => {
    playbackActive.value = ttsState.value === 'speaking';
  };

  const voiceConversation = createVoiceConversation({
    conversation: () => conversation.value ?? session.currentConversationKey,
    isConversationCurrent: key => session.isConversationCurrent(key),
    connectDocument: document.connect,
    onView: view => {
      serviceState.value = view.state;
      serviceMessage.value = view.message;
      partialText.value = view.partialText;
    },
    onSnapshot: snapshot => {
      monitor.observe(snapshot?.active && snapshot.micState !== 'closed' ? snapshot.inputSessionId : null);
      active.value = snapshot?.active ?? false;
      conversation.value = snapshot?.active && session.currentConversationKey
        ? { ...session.currentConversationKey } : null;
      inputState.value = snapshot?.inputState ?? 'idle';
      replyState.value = snapshot?.replyState ?? 'idle';
      ttsState.value = snapshot?.ttsState ?? 'idle';
      const reasonChanged = microphoneStopReason !== (snapshot?.micStopReason ?? null);
      microphoneStopReason = snapshot?.micStopReason ?? null;
      captureState.value = snapshot?.active ? snapshot.micState : 'closed';
      if (!snapshot?.active || snapshot.micState !== 'closed' || reasonChanged) microphonePending.value = false;
      if (!snapshot?.active || snapshot.micState === 'closed') {
        if (!microphonePending.value || !snapshot?.active) {
          const reason = snapshot?.active ? snapshot.micStopReason : null;
          // The snapshot retains the cause after the one-off native terminal,
          // including on view recovery and later reply/playback updates.
          errorMessage.value = reason && reason !== 'requested' && reason !== 'background'
            && reason !== 'webview_replaced' && reason !== 'webview_destroyed' && reason !== 'document_replaced' && reason !== 'document_left'
            && reason !== 'voice_cancelled' && !reason.startsWith('cancelled')
            ? audioErrorText(reason, '麦克风已被系统中断，点击麦克风重试。') : '';
          level.value = 0;
          bands.value = [];
          syncPlaybackState();
        }
      } else errorMessage.value = snapshot.micStopReason
        ? audioErrorText(snapshot.micStopReason, '麦克风操作失败，请重试。') : '';
      playbackActive.value = snapshot?.ttsState === 'speaking';
    },
  });

  const monitor = createMicrophoneMonitor(
    (value, spectrum) => { level.value = value; bands.value = spectrum; },
    // Native terminals clean up meters; the versioned call snapshot owns state.
    syncPlaybackState,
    frame => {
      playbackActive.value = frame.playbackActive;
    },
  );

  const stopMicrophone = () => {
    const generation = ++microphoneGeneration;
    microphonePending.value = false;
    return voiceConversation.closeMicrophone().catch(error => {
      if (generation !== microphoneGeneration || !active.value) return;
      // Keep the latest native projection and a usable close button. A failed
      // command does not prove that capture stopped, and must remain retryable.
      errorMessage.value = audioErrorMessage(error, '麦克风关闭失败，请重试。');
    });
  };

  const enableMicrophone = async () => {
    if (!active.value || lifecycle.isBackground) return;
    const generation = ++microphoneGeneration;
    syncPlaybackState();
    microphonePending.value = true;
    errorMessage.value = '';
    try {
      const started = await voiceConversation.openMicrophone(lifecycle.lifecycleSnapshot?.transitionEpoch ?? null);
      if (generation !== microphoneGeneration || !active.value) return;
      if (started && !started.configured && !['recognizing', 'thinking', 'speaking'].includes(serviceState.value)) {
        serviceState.value = 'local';
        serviceMessage.value = '配置语音服务并重新开麦后，即可识别与朗读。';
      }
    } catch (error) {
      if (generation !== microphoneGeneration || !active.value) return;
      syncPlaybackState();
      errorMessage.value = audioErrorMessage(error, '麦克风启动失败，请重试。');
    } finally {
      if (generation === microphoneGeneration) microphonePending.value = false;
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
    syncPlaybackState();
    captureState.value = 'closed';
    microphonePending.value = false;
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

  watch(() => session.currentConversationKey, () => {
    if (active.value && !session.isConversationCurrent(conversation.value)) end();
  }, { flush: 'sync' });
  // Native lifecycle facts stop capture even when this document is suspended.
  // Foreground recovery only projects the snapshot; it never reopens the mic.
  onScopeDispose(() => {
    microphoneGeneration += 1;
    monitor.detach();
    voiceConversation.dispose();
  });

  return {
    active, microphoneState, level, bands,
    playbackActive, errorMessage, serviceState, serviceMessage, partialText,
    inputState, replyState, ttsState,
    start, end, toggleMicrophone, recover,
  };
});
