import { defineStore } from "pinia";
import { ref, nextTick, onScopeDispose, watch } from "vue";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useNotificationStore } from "./notification";
import { useChatSessionStore } from "./chatSessionStore";
import { isAndroid, isIOS } from "../utils/platform";
import { cancelFileOperation, pickFile, pickFiles } from "../utils/nativeFilePicker";
import type { PickedFile } from "tauri-plugin-vcp-mobile";
import type {
  NativeFileProgressDetail,
  NativeFileStartDetail,
} from "../types/native";
import type {
  Attachment,
  AttachmentData,
  AttachmentRegisterProgressDto,
  ChatMessage,
} from "../types/chat";



export const useAttachmentStore = defineStore("attachment", () => {
  // 暂存的附件列表，准备随下一条消息发送
  const stagedAttachments = ref<Attachment[]>([]);

  // 全局监听 Rust 端发出的注册进度，用于大文件哈希/移动等Phase 2流程
  const progressListener = listen<AttachmentRegisterProgressDto>("vcp-file-register-progress", (event) => {
    const { progress, stableId } = event.payload;
    if (stableId) {
      const idx = stagedAttachments.value.findIndex((a) => a.id === stableId);
      if (idx !== -1 && stagedAttachments.value[idx].status === "loading") {
        if (progress >= 99) {
          // 进度达到 99% 说明物理传输/Hash已完毕，开始进入后端同步文本提取的 processing 阶段
          stagedAttachments.value[idx].status = "processing";
          stagedAttachments.value[idx].progress = undefined;
        } else {
          const currentProgress = stagedAttachments.value[idx].progress || 0;
          // 防抖/防回退：仅在进度增加时更新
          if (progress > currentProgress) {
            stagedAttachments.value[idx].progress = progress;
          }
        }
      }
    }
  });

  /**
   * 处理消息中的本地资源路径 (仅附件)，使用 Tauri 原生 asset:// 协议绕过 WebView 限制
   */
  const resolveMessageAssets = (msg: ChatMessage) => {
    // 处理附件 (仅处理图片类型)
    if (msg.attachments && msg.attachments.length > 0) {
      msg.attachments.forEach((att: Attachment) => {
        // 消息内显示只使用有界派生；原件保持业务资源身份。
        const sourcePath = att.thumbnailPath;
        if (
          att.type.startsWith("image/") &&
          sourcePath &&
          !sourcePath.startsWith("http") &&
          !sourcePath.startsWith("data:")
        ) {
          try {
            att.resolvedSrc = convertFileSrc(sourcePath);
          } catch (err) {
            console.warn(
              `[AttachmentStore] Failed to convert attachment image path ${att.name}:`,
              err,
            );
          }
        }
      });
    }
  };

  // The card identity is also the native/Rust operation identity. Removing a card
  // invalidates its consumer immediately; the resource owners finish cancellation.
  const activeOperations = new Set<string>();
  let picking = false;

  const cancelOperation = (operationId: string) => {
    if (!activeOperations.delete(operationId)) return;
    void invoke("cancel_attachment_import", { operationId }).catch(console.warn);
    void cancelFileOperation(operationId).catch(console.warn);
  };

  const beginImport = async (operationId: string) => {
    activeOperations.add(operationId);
    await invoke("begin_file_operation", { operationId });
    if (activeOperations.has(operationId)) return true;
    await invoke("cancel_attachment_import", { operationId });
    return false;
  };

  const finishImport = async (operationId: string) => {
    activeOperations.delete(operationId);
    await invoke("finish_file_operation", { operationId }).catch(console.warn);
  };

  const sessionStore = useChatSessionStore();
  watch(() => sessionStore.currentConversationKey?.epoch, () => {
    const invalidated = new Set(activeOperations);
    for (const operationId of invalidated) cancelOperation(operationId);
    stagedAttachments.value = stagedAttachments.value.filter(a => !a.id || !invalidated.has(a.id));
  }, { flush: "sync" });

  /** External files always enter through the native picker and an owned path. */
  const handleAttachment = async (mode: 'camera' | 'gallery' | 'file' = 'file') => {
    const notificationStore = useNotificationStore();
    if (!isAndroid && !isIOS) {
      notificationStore.addNotification({
        type: "info", message: "请在移动端应用中选取附件。", toastOnly: true,
      });
      return;
    }
    if (picking) return;
    picking = true;
    const operationId = `att_${crypto.randomUUID()}`;
    const pendingFiles = new Set<PickedFile>();
    const pendingIds = new Set<string>();
    const releasePickedFile = async (file: PickedFile) => {
      if (!pendingFiles.delete(file)) return;
      await invoke("plugin:vcp-mobile|delete_temp_file", { filePath: file.path }).catch(console.warn);
      if (file.thumbnailPath) {
        await invoke("plugin:vcp-mobile|delete_temp_file", { filePath: file.thumbnailPath }).catch(console.warn);
      }
    };
    const current = () => activeOperations.has(operationId);
    const card = () => stagedAttachments.value.find((a) => a.id === operationId);
    const updateCard = (name: string, mime: string, size: number, progress: number) => {
      if (!current()) return;
      const existing = card();
      if (existing) {
        existing.name = name || existing.name;
        existing.type = mime || existing.type;
        existing.size = size || existing.size;
        existing.progress = Math.max(existing.progress || 0, progress);
      } else {
        stagedAttachments.value.unshift({
          id: operationId, name: name || "正在选取附件", type: mime || "application/octet-stream",
          src: "", size, progress, status: "loading",
        });
      }
    };
    const handleStart = (event: Event) => {
      const detail = (event as CustomEvent<NativeFileStartDetail>).detail;
      if (detail.operationId === operationId) {
        const label = detail.selectionCount && detail.selectionCount > 1
          ? `（${(detail.selectionIndex ?? 0) + 1}/${detail.selectionCount}）${detail.name}`
          : detail.name;
        updateCard(label, detail.mime, detail.size, 0);
        const pending = card();
        if (pending) {
          pending.size = detail.size;
          pending.progress = detail.size ? 0 : undefined;
          pending.transferredBytes = undefined;
        }
      }
    };
    const handleProgress = (event: Event) => {
      const detail = (event as CustomEvent<NativeFileProgressDetail>).detail;
      if (detail.operationId === operationId) {
        const label = detail.selectionCount && detail.selectionCount > 1
          ? `（${(detail.selectionIndex ?? 0) + 1}/${detail.selectionCount}）${detail.name}`
          : detail.name;
        updateCard(label, detail.mime, detail.total, Math.min(90, Math.round(detail.progress * 0.9)));
        const pending = card();
        if (pending) {
          pending.transferredBytes = detail.loaded;
          if (!detail.total) pending.progress = undefined;
        }
      }
    };
    window.addEventListener("vcp-mobile-file-start", handleStart);
    window.addEventListener("vcp-mobile-file-progress", handleProgress);
    try {
      if (!await beginImport(operationId)) return;
      updateCard("正在选择文件", "application/octet-stream", 0, 0);
      const pending = card();
      if (pending) pending.progress = undefined;
      const result = mode === "camera"
        ? { files: [await pickFile(mode, { operationId })], errors: [] }
        : await pickFiles(mode, { operationId });
      const pickedFiles = result.files;
      for (const file of pickedFiles) pendingFiles.add(file);
      window.removeEventListener("vcp-mobile-file-start", handleStart);
      window.removeEventListener("vcp-mobile-file-progress", handleProgress);
      if (!current()) return;
      stagedAttachments.value = stagedAttachments.value.filter(a => a.id !== operationId);
      const failures = result.errors.map(error => `${error.name}: ${error.message}`);
      let importedCount = 0;
      // Acquire as one cancellable selection, then give each CAS registration its
      // own identity. All queued cards exist before the first registration starts.
      const imports = pickedFiles.map(file => {
        const id = `att_${crypto.randomUUID()}`;
        activeOperations.add(id);
        pendingIds.add(id);
        return { id, file };
      });
      stagedAttachments.value.unshift(...imports.map(({ id, file }): Attachment => ({
        id, name: file.name, type: file.mime || "application/octet-stream",
        src: "", size: file.size, progress: 90, status: "loading",
      })));
      await nextTick();
      for (const { id, file } of imports) {
        try {
          if (!current() || !activeOperations.has(id)) continue;
          if (!await beginImport(id)) continue;
          await invoke("check_attachment_support", { originalName: file.name });
          if (!activeOperations.has(id)) continue;
          const finalData = await invoke<AttachmentData>("register_local_file", {
            localPath: file.path, originalName: file.name,
            mimeType: file.mime || "application/octet-stream",
            thumbnailPath: file.thumbnailPath || null,
            stableId: id, operationId: id, expectedHash: file.hash || null,
          });
          if (!activeOperations.has(id)) continue;
          const index = stagedAttachments.value.findIndex(a => a.id === id);
          if (index !== -1) {
            stagedAttachments.value[index] = {
              ...stagedAttachments.value[index], ...finalData, id,
              src: finalData.internalPath, internalPath: finalData.internalPath,
              thumbnailPath: finalData.thumbnailPath ?? undefined,
              extractedText: finalData.extractedText ?? undefined,
              status: "done", progress: undefined, transferredBytes: undefined,
            };
            importedCount++;
          }
        } catch (error) {
          if (activeOperations.has(id) && !String(error).toLowerCase().includes("cancel")) {
            failures.push(`${file.name}: ${String(error)}`);
          }
          stagedAttachments.value = stagedAttachments.value.filter(a => a.id !== id);
        } finally {
          pendingIds.delete(id);
          await finishImport(id);
          await releasePickedFile(file);
        }
      }
      if (current() && failures.length) {
        notificationStore.addNotification({
          type: "warning", title: importedCount ? "部分附件未能导入" : "附件导入失败",
          message: `已导入 ${importedCount} 项；${failures.join("；")}`, toastOnly: false,
        });
      }
    } catch (error) {
      if (!current()) return;
      const index = stagedAttachments.value.findIndex((a) => a.id === operationId);
      if (index !== -1) stagedAttachments.value.splice(index, 1);
      if (!String(error).toLowerCase().includes("cancel")) {
        notificationStore.addNotification({
          type: "warning", title: "选取附件失败", message: String(error), toastOnly: true,
        });
      }
    } finally {
      window.removeEventListener("vcp-mobile-file-start", handleStart);
      window.removeEventListener("vcp-mobile-file-progress", handleProgress);
      stagedAttachments.value = stagedAttachments.value.filter(a => a.id !== operationId);
      stagedAttachments.value = stagedAttachments.value.filter(a => !a.id || !pendingIds.has(a.id));
      for (const id of pendingIds) {
        cancelOperation(id);
        await finishImport(id);
      }
      await finishImport(operationId);
      // A committed CAS file is owned by Rust; this command only removes native
      // staging paths, including unsupported files and stale picker completions.
      for (const file of pendingFiles) await releasePickedFile(file);
      picking = false;
    }
  };
  /**
   * 移除特定位置的暂存附件
   */
  const removeStaged = (index: number) => {
    if (index >= 0 && index < stagedAttachments.value.length) {
      const operationId = stagedAttachments.value[index].id;
      if (operationId) cancelOperation(operationId);
      stagedAttachments.value.splice(index, 1);
    }
  };

  /**
   * 清空暂存附件
   */
  const clearStaged = () => {
    for (const operationId of activeOperations) cancelOperation(operationId);
    stagedAttachments.value = [];
  };

  onScopeDispose(() => {
    clearStaged();
    void progressListener.then((unlisten) => unlisten()).catch(console.warn);
  });

  return {
    stagedAttachments,
    handleAttachment,
    resolveMessageAssets,
    removeStaged,
    clearStaged,
    beginImport,
    finishImport,
  };
});
