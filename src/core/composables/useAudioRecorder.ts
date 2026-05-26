import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';

export function useAudioRecorder() {
  const isRecording = ref(false);
  const recordingDuration = ref(0);
  const audioBlob = ref<Blob | null>(null);

  let mediaRecorder: MediaRecorder | null = null;
  let audioChunks: Blob[] = [];
  let timerInterval: number | null = null;
  let startTime = 0;

  // 探测 WebView 支持的音频格式，优先采用 webm
  const getSupportedMimeType = (): string => {
    const types = [
      'audio/webm;codecs=opus',
      'audio/webm',
      'audio/ogg;codecs=opus',
      'audio/ogg',
      'audio/mp4',
      'audio/wav',
    ];
    for (const type of types) {
      if (MediaRecorder.isTypeSupported(type)) {
        return type;
      }
    }
    return '';
  };

  const startRecording = async (): Promise<void> => {
    if (isRecording.value) return;

    try {
      const isAndroid = navigator.userAgent.toLowerCase().includes('android');
      if (isAndroid) {
        try {
          // 主动呼起原生 Android 麦克风录音权限弹窗申请
          await invoke('plugin:vcp-mobile|requestAndroidPermission', { type: 'microphone' });
        } catch (pe) {
          console.warn('[AudioRecorder] Failed to request native microphone permission:', pe);
        }
      }

      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      audioChunks = [];
      recordingDuration.value = 0;
      audioBlob.value = null;

      const mimeType = getSupportedMimeType();
      const options = mimeType ? { mimeType } : undefined;

      mediaRecorder = new MediaRecorder(stream, options);

      mediaRecorder.ondataavailable = (event) => {
        if (event.data && event.data.size > 0) {
          audioChunks.push(event.data);
        }
      };

      mediaRecorder.onstop = () => {
        const mimeTypeUsed = mediaRecorder?.mimeType || 'audio/webm';
        audioBlob.value = new Blob(audioChunks, { type: mimeTypeUsed });
        
        // 释放音轨流以关闭物理麦克风占用指示灯
        stream.getTracks().forEach((track) => track.stop());
      };

      mediaRecorder.start(250); // 每 250ms 切割一次，防止内存泄露且保障数据连续性
      startTime = Date.now();
      isRecording.value = true;

      timerInterval = window.setInterval(() => {
        recordingDuration.value = Math.round((Date.now() - startTime) / 1000);
      }, 1000);

    } catch (err) {
      console.error('[AudioRecorder] Failed to start recording:', err);
      throw err;
    }
  };

  const stopRecording = (): Promise<{ blob: Blob; bytes: Uint8Array } | null> => {
    return new Promise((resolve) => {
      if (!isRecording.value || !mediaRecorder) {
        resolve(null);
        return;
      }

      const currentRecorder = mediaRecorder;

      // 清除计时器
      if (timerInterval) {
        clearInterval(timerInterval);
        timerInterval = null;
      }

      currentRecorder.onstop = async () => {
        const mimeTypeUsed = currentRecorder.mimeType || 'audio/webm';
        const blob = new Blob(audioChunks, { type: mimeTypeUsed });
        audioBlob.value = blob;

        // 释放麦克风资源
        const stream = currentRecorder.stream;
        if (stream) {
          stream.getTracks().forEach((track) => track.stop());
        }

        try {
          const arrayBuffer = await blob.arrayBuffer();
          const bytes = new Uint8Array(arrayBuffer);
          
          isRecording.value = false;
          resolve({ blob, bytes });
        } catch (e) {
          console.error('[AudioRecorder] Failed to parse array buffer:', e);
          isRecording.value = false;
          resolve(null);
        }
      };

      currentRecorder.stop();
    });
  };

  const cancelRecording = (): void => {
    if (!isRecording.value || !mediaRecorder) return;

    if (timerInterval) {
      clearInterval(timerInterval);
      timerInterval = null;
    }

    mediaRecorder.onstop = () => {
      const stream = mediaRecorder?.stream;
      if (stream) {
        stream.getTracks().forEach((track) => track.stop());
      }
      isRecording.value = false;
      audioChunks = [];
      audioBlob.value = null;
      recordingDuration.value = 0;
    };

    mediaRecorder.stop();
  };

  return {
    isRecording,
    recordingDuration,
    audioBlob,
    startRecording,
    stopRecording,
    cancelRecording,
  };
}
