import { ref, computed } from 'vue';
import { useSettingsStore } from '../stores/settings';
import { useAudioRecorder } from './useAudioRecorder';

export function useSpeechRecognition() {
  const settingsStore = useSettingsStore();
  const { isRecording, startRecording, stopRecording, cancelRecording } = useAudioRecorder();

  const isListening = ref(false);
  const transcriptionResult = ref('');
  const interimTranscription = ref('');
  const isFallbackMode = ref(false); // 是否已降级为云端 Whisper 录音识别模式

  // 判定当前设备环境是否原生支持 Web Speech Recognition API
  const SpeechRecognitionClass = (window as any).SpeechRecognition || (window as any).webkitSpeechRecognition;
  const isNativeSupported = computed(() => !!SpeechRecognitionClass);

  let recognitionInstance: any = null;
  let finalTranscriptAccumulator = '';

  const initNativeRecognition = (onResultCallback: (text: string) => void) => {
    if (!SpeechRecognitionClass) return;

    const rec = new SpeechRecognitionClass();
    rec.continuous = true;
    rec.interimResults = true;
    rec.lang = 'zh-CN'; // 默认采用中文识别

    rec.onstart = () => {
      isListening.value = true;
      transcriptionResult.value = '';
      interimTranscription.value = '';
      finalTranscriptAccumulator = '';
    };

    rec.onresult = (event: any) => {
      let interimTranscript = '';
      let finalTranscript = '';

      for (let i = event.resultIndex; i < event.results.length; ++i) {
        if (event.results[i].isFinal) {
          finalTranscript += event.results[i][0].transcript;
        } else {
          interimTranscript += event.results[i][0].transcript;
        }
      }

      if (finalTranscript) {
        finalTranscriptAccumulator += finalTranscript;
      }

      interimTranscription.value = interimTranscript;
      const totalResult = finalTranscriptAccumulator + interimTranscript;
      transcriptionResult.value = totalResult;

      onResultCallback(totalResult);
    };

    rec.onerror = (event: any) => {
      console.warn('[SpeechRecognition] Native error encountered:', event.error);
      
      // 捕获系统级别无法提供语音识别服务 (例如定制系统无 Google 语音服务框架)
      if (event.error === 'service-not-allowed' || event.error === 'network') {
        console.log('[SpeechRecognition] Service unavailable. Triggering cloud fallback path.');
        isFallbackMode.value = true;
        // 结束原生实例并转移到录音状态
        stopListening();
        startListening(onResultCallback);
      }
    };

    rec.onend = () => {
      isListening.value = false;
    };

    recognitionInstance = rec;
  };

  /**
   * 开启语音输入
   */
  const startListening = async (onResultCallback: (text: string) => void): Promise<void> => {
    transcriptionResult.value = '';
    interimTranscription.value = '';

    // 1. 如果支持原生且未激活降级，首选本地 Google 流式转文字
    if (isNativeSupported.value && !isFallbackMode.value) {
      try {
        const isAndroid = navigator.userAgent.toLowerCase().includes('android');
        if (isAndroid) {
          try {
            // 主动在原生端触发 Android 麦克风录音权限弹窗申请
            const { invoke } = await import('@tauri-apps/api/core');
            await invoke('plugin:vcp-mobile|requestAndroidPermission', { type: 'microphone' });
          } catch (pe) {
            console.warn('[SpeechRecognition] Failed to request native microphone permission:', pe);
          }
        }

        if (!recognitionInstance) {
          initNativeRecognition(onResultCallback);
        }
        recognitionInstance.start();
      } catch (err) {
        console.error('[SpeechRecognition] Failed to start native recognition:', err);
        isFallbackMode.value = true;
        await startListening(onResultCallback); // 降级递归
      }
    } else {
      // 2. 降级为录音 + 云端 Whisper API 转写
      console.log('[SpeechRecognition] Native recognition unsupported. Initiating Whisper mode.');
      isFallbackMode.value = true;
      try {
        await startRecording();
        isListening.value = true;
      } catch (err) {
        console.error('[SpeechRecognition] Failed to start recorder in fallback:', err);
        throw err;
      }
    }
  };

  /**
   * 停止语音输入并获取识别结果 (云端模式下会发起网络请求)
   */
  const stopListening = async (): Promise<string> => {
    if (!isListening.value) return transcriptionResult.value;

    if (!isFallbackMode.value && recognitionInstance) {
      // 原生模式：直接结束，内容已在 onresult 中累加完成
      recognitionInstance.stop();
      isListening.value = false;
      return transcriptionResult.value;
    } else {
      // 云端 Whisper 模式：停止录音 -> 上传转写 -> 返回文本
      isListening.value = false;
      const recordResult = await stopRecording();
      if (!recordResult) return '';

      const { blob } = recordResult;
      
      // 检查配置
      const serverUrl = settingsStore.settings?.vcpServerUrl;
      const apiKey = settingsStore.settings?.vcpApiKey;

      if (!serverUrl || !apiKey) {
        console.warn('[SpeechRecognition] Server settings missing for Whisper STT.');
        return '[错误：未配置云端 API 密钥，无法完成识别]';
      }

      try {
        // 对齐桌面端，做 baseURL 归一化提取，以防配置了完整 Completions 的 URL
        const urlObj = new URL(serverUrl);
        const baseUrl = `${urlObj.protocol}//${urlObj.host}`;
        const whisperEndpoint = new URL('/v1/audio/transcriptions', baseUrl).toString();

        const formData = new FormData();
        formData.append('file', blob, 'recording.webm');
        formData.append('model', 'whisper-1');

        const response = await fetch(whisperEndpoint, {
          method: 'POST',
          headers: {
            'Authorization': `Bearer ${apiKey}`
          },
          body: formData
        });

        if (!response.ok) {
          throw new Error(`Whisper server responded with ${response.status}`);
        }

        const data = await response.json();
        if (data && data.text) {
          transcriptionResult.value = data.text.trim();
          return data.text.trim();
        }
        return '';
      } catch (err: any) {
        console.error('[SpeechRecognition] Whisper cloud transcription failed:', err);
        return `[识别失败: ${err.message || String(err)}]`;
      }
    }
  };

  /**
   * 取消当前倾听
   */
  const cancelListening = (): void => {
    if (!isListening.value) return;

    if (!isFallbackMode.value && recognitionInstance) {
      recognitionInstance.abort();
    } else {
      cancelRecording();
    }
    isListening.value = false;
    transcriptionResult.value = '';
    interimTranscription.value = '';
  };

  return {
    isListening,
    isFallbackMode,
    transcriptionResult,
    interimTranscription,
    startListening,
    stopListening,
    cancelListening,
    isRecording,
  };
}
