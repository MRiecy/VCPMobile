import { ref, nextTick, onScopeDispose, watch, type Ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useChatSessionStore } from '../../../core/stores/chatSessionStore';
import { useAttachmentStore } from '../../../core/stores/attachmentStore';
import { useNotificationStore } from '../../../core/stores/notification';
import type { AttachmentData } from '../../../core/types/chat';

const LONG_TEXT_THRESHOLD = 1000;
// 文本产物使用更低的独立预算；先计数再编码，避免生成大字节数组。
const MAX_TEXT_FILE_BYTES = 2 * 1024 * 1024;

function fitsTextFileBudget(text: string): boolean {
  if (text.length > MAX_TEXT_FILE_BYTES) return false;
  let bytes = 0;
  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i);
    if (code <= 0x7f) bytes += 1;
    else if (code <= 0x7ff) bytes += 2;
    else if (code >= 0xd800 && code <= 0xdbff && i + 1 < text.length &&
      text.charCodeAt(i + 1) >= 0xdc00 && text.charCodeAt(i + 1) <= 0xdfff) {
      bytes += 4;
      i += 1;
    } else {
      // BMP 字符为三字节；孤立代理项由 TextEncoder 编为三字节替换字符。
      bytes += 3;
    }
    if (bytes > MAX_TEXT_FILE_BYTES) return false;
  }
  return true;
}

export function useLongTextPaste(input: Ref<string>, canConvert: () => boolean = () => true) {
  const sessionStore = useChatSessionStore();
  const attachmentStore = useAttachmentStore();
  const notificationStore = useNotificationStore();
  const isProcessing = ref(false);
  let disposed = false;
  let activeAttachmentId: string | null = null;
  let programmaticDepth = 0;
  let inputBaseline = input.value;

  // Only known application writes bypass detection. Sync delivery keeps the
  // source scope alive for the watcher, including nested conversion rollback.
  const setProgrammaticInput = (value: string) => {
    programmaticDepth++;
    try {
      input.value = value;
      inputBaseline = value;
    } finally {
      programmaticDepth--;
    }
  };

  const removeActiveAttachment = () => {
    const index = attachmentStore.stagedAttachments.findIndex(a => a.id === activeAttachmentId);
    if (index !== -1) attachmentStore.removeStaged(index);
  };

  // 话题切换（包括切走再回来）及卸载不允许旧请求回填当前输入区。
  watch(() => sessionStore.currentConversationKey?.epoch, removeActiveAttachment, { flush: 'sync' });
  onScopeDispose(() => {
    disposed = true;
    removeActiveAttachment();
  });

  /**
   * 校验是否已选择 Agent 和话题
   */
  const assertTopicSelected = (): boolean => {
    if (!sessionStore.currentConversationKey) {
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
  const stageTextAsFile = async (
    text: string,
    consumeInput: () => void,
    recoveryInput?: string,
  ) => {
    // paste / beforeinput / watch 共享同一次占用；忙碌时保留普通文本输入。
    if (disposed || isProcessing.value || !canConvert() || !assertTopicSelected()) return;
    if (!fitsTextFileBudget(text)) {
      notificationStore.addNotification({
        type: 'warning',
        title: '长文本未转为附件',
        message: '文本超过 2 MiB，请缩短文本或分段粘贴。原文本仍保留在输入框中。',
        toastOnly: true,
      });
      return;
    }

    const conversation = sessionStore.currentConversationKey;
    const isCurrent = () => !disposed && sessionStore.isConversationCurrent(conversation);
    const recoveryText = recoveryInput ?? input.value + text;
    const name = `pasted_text_${new Date().toISOString().slice(0, 19).replace(/[:T]/g, '-')}.txt`;
    const stableId = `att_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`;
    let consumed = false;
    let retainedInput = input.value;
    isProcessing.value = true;
    activeAttachmentId = stableId;

    try {
      const bytes = new TextEncoder().encode(text);
      consumeInput();
      consumed = true;
      retainedInput = input.value;
      // 1. 只有校验和编码完成后才消费输入、插入 loading 占位。
      attachmentStore.stagedAttachments.unshift({
        id: stableId,
        type: 'text/plain',
        src: '',
        name,
        size: bytes.length,
        status: 'loading',
      });
      await nextTick();
      if (!isCurrent()) return;
      if (!attachmentStore.stagedAttachments.some(a => a.id === stableId)) return;
      if (!await attachmentStore.beginImport(stableId)) return;
      if (!isCurrent()) return;
      window.dispatchEvent(new Event('resize'));

      // 2. IPC 保存到附件目录
      const finalData = await invoke<AttachmentData>('store_file', {
        originalName: name,
        fileBytes: bytes,
        mimeType: 'text/plain',
        operationId: stableId,
      });

      if (!isCurrent()) return;
      // 3. 更新为完成状态；用户已移除的占位不再报告成功。
      const index = attachmentStore.stagedAttachments.findIndex(a => a.id === stableId);
      if (index === -1) return;
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

      notificationStore.addNotification({
        type: 'success',
        title: '长文本已转附件',
        message: `已保存为 ${name}（${text.length} 字符）`,
        toastOnly: true,
      });
    } catch (err) {
      console.error('[useLongTextPaste] Failed to stage text as file:', err);
      const removed = !attachmentStore.stagedAttachments.some(a => a.id === stableId);
      removeActiveAttachment();
      if (!isCurrent()) return;
      if (removed && String(err).toLowerCase().includes('cancel')) return;
      if (consumed) {
        // 保留处理中用户继续输入的内容；恢复操作不能再触发自动转附件。
        setProgrammaticInput(input.value === retainedInput ? recoveryText : input.value + text);
      }
      notificationStore.addNotification({
        type: 'error',
        title: '附件保存失败',
        message: err instanceof Error ? err.message : String(err),
        toastOnly: true,
      });
    } finally {
      if (!isCurrent()) removeActiveAttachment();
      await attachmentStore.finishImport(stableId);
      activeAttachmentId = null;
      isProcessing.value = false;
    }
  };

  /**
   * 粘贴事件处理器
   * 优先级：文件 > 图片 > 长文本 > 默认行为
   */
  const handlePaste = async (event: ClipboardEvent) => {
    if (event.defaultPrevented || !event.cancelable) return;
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
      await stageTextAsFile(pastedText, () => event.preventDefault());
    }
  };

  /**
   * beforeinput 事件处理器（覆盖 Android 键盘粘贴按钮）
   */
  const handleBeforeInput = (event: InputEvent) => {
    // 无法阻止的输入交给后续 watch，避免同时留下文本和转换后的附件。
    if (event.defaultPrevented || !event.cancelable) return;
    if (
      event.inputType !== 'insertFromPaste' &&
      event.inputType !== 'insertFromPasteAsQuotation'
    ) return;

    const text = event.data;
    if (text && text.length > LONG_TEXT_THRESHOLD) {
      void stageTextAsFile(text, () => event.preventDefault());
    }
  };

  /**
   * Layer 3: watch 突变检测（覆盖不标准输入法的 paste）
   */
  watch(input, (newVal) => {
    const oldVal = inputBaseline;
    inputBaseline = newVal;
    if (programmaticDepth || isProcessing.value || !canConvert()) return;
    const delta = newVal.length - oldVal.length;
    if (delta > LONG_TEXT_THRESHOLD) {
      let pastedText = newVal;
      if (newVal.startsWith(oldVal)) {
        pastedText = newVal.slice(oldVal.length);
      }

      // 超限或没有话题时保留已经进入输入框的文本。
      void stageTextAsFile(pastedText, () => setProgrammaticInput(oldVal), newVal);
    }
  }, { flush: 'sync' });

  return {
    handlePaste,
    handleBeforeInput,
    setProgrammaticInput,
    isProcessing,
    LONG_TEXT_THRESHOLD,
  };
}
