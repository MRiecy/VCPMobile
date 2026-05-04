import { ref, nextTick, watch, type Ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useChatSessionStore } from '../../../core/stores/chatSessionStore';
import { useAttachmentStore } from '../../../core/stores/attachmentStore';
import { useNotificationStore } from '../../../core/stores/notification';

const LONG_TEXT_THRESHOLD = 1200;

export function useLongTextPaste(input: Ref<string>) {
  const sessionStore = useChatSessionStore();
  const attachmentStore = useAttachmentStore();
  const notificationStore = useNotificationStore();
  const isProcessing = ref(false);

  /**
   * 校验是否已选择 Agent 和话题
   */
  const assertTopicSelected = (): boolean => {
    if (!sessionStore.currentSelectedItem?.id || !sessionStore.currentTopicId) {
      notificationStore.addNotification({
        type: 'warning',
        title: '无法粘贴长文本',
        message: '请先选择一个 Agent 和话题，再粘贴长文本。',
        toastOnly: true,
      });
      return false;
    }
    return true;
  };

  /**
   * 将文本内容暂存为 .txt 附件
   */
  const stageTextAsFile = async (text: string, fileName?: string) => {
    const name = fileName || `pasted_text_${new Date().toISOString().slice(0, 19).replace(/[:T]/g, '-')}.txt`;
    const bytes = new TextEncoder().encode(text);
    const stableId = `att_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`;

    // 1. 插入 loading 占位
    attachmentStore.stagedAttachments.unshift({
      id: stableId,
      type: 'text/plain',
      src: '',
      name,
      size: bytes.length,
      status: 'loading',
    });
    await nextTick();
    window.dispatchEvent(new Event('resize'));

    try {
      // 2. IPC 保存到附件目录
      const finalData = await invoke<any>('store_file', {
        originalName: name,
        fileBytes: bytes,
        mimeType: 'text/plain',
      });

      // 3. 更新为完成状态
      const index = attachmentStore.stagedAttachments.findIndex(a => a.id === stableId);
      if (index !== -1) {
        attachmentStore.stagedAttachments[index] = {
          ...attachmentStore.stagedAttachments[index],
          type: finalData.type,
          src: finalData.internalPath,
          name: finalData.name,
          size: finalData.size,
          hash: finalData.hash,
          internalPath: finalData.internalPath,
          status: 'done',
        };
      }

      notificationStore.addNotification({
        type: 'success',
        title: '长文本已转附件',
        message: `已保存为 ${name}（${text.length} 字符）`,
        toastOnly: true,
      });
    } catch (err) {
      console.error('[useLongTextPaste] Failed to stage text as file:', err);
      const index = attachmentStore.stagedAttachments.findIndex(a => a.id === stableId);
      if (index !== -1) attachmentStore.stagedAttachments.splice(index, 1);
      notificationStore.addNotification({
        type: 'error',
        title: '附件保存失败',
        message: err instanceof Error ? err.message : String(err),
        toastOnly: true,
      });
    }
  };

  /**
   * 粘贴事件处理器
   * 优先级：文件 > 图片 > 长文本 > 默认行为
   */
  const handlePaste = async (event: ClipboardEvent) => {
    if (isProcessing.value) {
      event.preventDefault();
      return;
    }

    const clipboardData = event.clipboardData;
    if (!clipboardData) return;

    // 1. 优先处理文件类型内容
    if (clipboardData.files.length > 0) return;

    // 2. 优先处理图片数据
    for (let i = 0; i < clipboardData.items.length; i++) {
      if (clipboardData.items[i].type.startsWith('image/')) return;
    }

    // 3. 检查纯文本长度
    const pastedText = clipboardData.getData('text/plain');
    if (pastedText && pastedText.length > LONG_TEXT_THRESHOLD) {
      event.preventDefault();

      if (!assertTopicSelected()) return;

      isProcessing.value = true;
      try {
        await stageTextAsFile(pastedText);
      } finally {
        isProcessing.value = false;
      }
    }
  };

  /**
   * beforeinput 事件处理器（覆盖 Android 键盘粘贴按钮）
   */
  const handleBeforeInput = (event: InputEvent) => {
    if (
      event.inputType !== 'insertFromPaste' &&
      event.inputType !== 'insertFromPasteAsQuotation'
    ) return;

    const text = event.data;
    if (text && text.length > LONG_TEXT_THRESHOLD) {
      event.preventDefault();

      if (!assertTopicSelected()) return;

      stageTextAsFile(text).catch(console.error);
    }
  };

  /**
   * Layer 3: watch 突变检测（覆盖不标准输入法的 paste）
   */
  watch(input, (newVal, oldVal) => {
    const delta = newVal.length - oldVal.length;
    if (delta > LONG_TEXT_THRESHOLD) {
      let pastedText = newVal;
      if (newVal.startsWith(oldVal)) {
        pastedText = newVal.slice(oldVal.length);
      }

      // 恢复旧值
      input.value = oldVal;

      if (!assertTopicSelected()) return;

      stageTextAsFile(pastedText).catch(console.error);
    }
  });

  return {
    handlePaste,
    handleBeforeInput,
    isProcessing,
    LONG_TEXT_THRESHOLD,
  };
}
