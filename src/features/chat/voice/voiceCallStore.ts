import { defineStore } from 'pinia';
import { computed, onScopeDispose, ref, shallowRef } from 'vue';
import { useChatSessionStore } from '../../../core/stores/chatSessionStore';
import { useNotificationStore } from '../../../core/stores/notification';
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
  [/\baudio_route_unavailable\b/, '当前录音或播放设备不可用，请确认系统音频输出后重新开启麦克风。'],
  [/\b(voice_input_unavailable|voice_input_device_unavailable)\b/, '所选麦克风不可用，请点击「麦克风」选择其他输入，再开启麦克风。'],
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
  [/\bcall_in_progress\b/, '已有通话进行中，请先挂断。'],
  [/\bstale_navigation\b/, '当前会话已变化，请在目标话题重新开始通话。'],
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
  const notifications = useNotificationStore();
  const captureState = ref<VoiceCallSnapshot['micState']>('closed');
  const inputDeviceId = ref<string | null>(null);
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
  const ttsState = ref<VoiceCallSnapshot['ttsState']>('idle');
  const speakingMessageId = ref<string | null>(null);
  let microphoneGeneration = 0;
  let microphoneStopReason: string | null = null;

  const syncPlaybackState = () => {
    playbackActive.value = ttsState.value === 'speaking';
  };

  const voiceConversation = createVoiceConversation({
    conversation: () => session.currentConversationKey,
    window: () => session.acceptedWindow,
    connectDocument: document.connect,
    onTargetDeleted: snapshot => notifications.addNotification({
      type: 'info', title: '通话已结束', message: snapshot.notice || '通话目标已删除。', toastOnly: true,
    }),
    onView: view => {
      serviceState.value = view.state;
      serviceMessage.value = view.message;
      partialText.value = view.partialText;
    },
    onSnapshot: snapshot => {
      monitor.observe(snapshot?.active && snapshot.micState !== 'closed' ? snapshot.inputSessionId : null);
      active.value = snapshot?.active ?? false;
      ttsState.value = snapshot?.ttsState ?? 'idle';
      speakingMessageId.value = snapshot?.active && snapshot.ttsState === 'speaking'
        ? snapshot.speakingMessageId ?? null : null;
      const reasonChanged = microphoneStopReason !== (snapshot?.micStopReason ?? null);
      microphoneStopReason = snapshot?.micStopReason ?? null;
      captureState.value = snapshot?.active ? snapshot.micState : 'closed';
      inputDeviceId.value = snapshot?.active ? snapshot.inputDeviceId ?? null : null;
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

  const enableMicrophone = async (startingCall = false) => {
    if ((!active.value && !startingCall) || lifecycle.isBackground) return;
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
      if (generation !== microphoneGeneration) return;
      if (!active.value) {
        notifications.addNotification({ type: 'error', title: '无法开始通话',
          message: audioErrorMessage(error, '当前会话已变化或通话未就绪，请重试。'), toastOnly: true });
        if (errorDetail(error) === 'call_in_progress') {
          await voiceConversation.recover().catch(failure => console.warn('[VoiceCall] 无法恢复当前通话', failure));
        }
        return;
      }
      syncPlaybackState();
      errorMessage.value = audioErrorMessage(error, '麦克风启动失败，请重试。');
    } finally {
      if (generation === microphoneGeneration) microphonePending.value = false;
    }
  };

  const start = () => {
    if (active.value || microphonePending.value || !session.currentConversationKey) return;
    void enableMicrophone(true);
  };

  const end = () => {
    void voiceConversation.stop().catch(error => {
      notifications.addNotification({ type: 'error', title: '挂断通话时出错',
        message: audioErrorMessage(error, '音频清理未完成，请检查当前通话状态。'), toastOnly: true });
    });
    // Hangup has one Rust owner. Do not race it with a separate mute/drain
    // command that could admit an unsent transcript while hangup cancels it.
    microphoneGeneration += 1;
    microphonePending.value = false;
  };

  const recover = () => voiceConversation.recover();

  const selectInput = (deviceId: string | null) =>
    voiceConversation.selectInput(deviceId, lifecycle.lifecycleSnapshot?.transitionEpoch ?? null);

  const toggleMicrophone = () => {
    if (!active.value) return;
    if (microphoneState.value === 'on' || microphoneState.value === 'requesting') stopMicrophone();
    else void enableMicrophone();
  };

  // Native lifecycle facts stop capture even when this document is suspended.
  // Foreground recovery only projects the snapshot; it never reopens the mic.
  onScopeDispose(() => {
    microphoneGeneration += 1;
    monitor.detach();
    voiceConversation.dispose();
  });

  return {
    active, microphoneState, inputDeviceId, listInputs: voiceConversation.listInputs, selectInput, level, bands,
    playbackActive, errorMessage, serviceState, serviceMessage, partialText,
    ttsState, speakingMessageId,
    start, end, toggleMicrophone, recover,
  };
});
