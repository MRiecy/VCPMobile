<script setup lang="ts">
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { shareFileNative, type NativeFileActionResult } from "tauri-plugin-vcp-mobile";
import { X } from "lucide-vue-next";
import AttachmentViewer from "./AttachmentViewer.vue";
import AttachmentRenderer from './AttachmentRenderer.vue';

import { useChatHistoryStore } from "../../../core/stores/chatHistoryStore";
import { useOverlayStore } from "../../../core/stores/overlay";
import { useNotificationStore } from "../../../core/stores/notification";
import { isAndroid, isIOS } from "../../../core/utils/platform";
import type { Attachment } from "../../../core/types/chat";

const props = defineProps<{
  attachments: Attachment[];
  messageId?: string;
  topicId?: string;
}>();

const isViewerOpen = ref(false);
const activeFile = ref<Attachment | null>(null);
const nativeBusy = ref(false);
const unsupportedPreviewPath = ref("");

const overlayStore = useOverlayStore();
const notificationStore = useNotificationStore();

const IMAGE_WHITELIST = ["jpg", "jpeg", "png", "gif", "webp", "svg", "bmp", "heic", "heif", "avif"];
const TEXT_WHITELIST = [
  "txt", "md", "csv", "json", "js", "ts", "py", "rs", "java", "c", "cpp",
  "h", "go", "rb", "php", "swift", "kt", "html", "css", "xml", "yaml",
  "yml", "toml", "ini", "log", "sql", "vue", "jsx", "tsx"
];

const isPreviewableText = (att: Attachment): boolean => {
  const ext = att.name.split(".").pop()?.toLowerCase() || "";
  
  // 核心加固：若存在后缀且完全不属于文本白名单，绝不判定为文本（杜绝 MIME 误判）
  if (ext && !TEXT_WHITELIST.includes(ext)) {
    return false;
  }
  
  if (TEXT_WHITELIST.includes(ext)) {
    return true;
  }
  
  const type = (att.type || "").toLowerCase();
  return (
    type.startsWith("text/") ||
    type === "application/json" ||
    type === "application/javascript" ||
    type === "application/x-javascript"
  );
};

const openViewer = (att: Attachment) => {
  if (att.status === "desktop_only") return;
  unsupportedPreviewPath.value = "";
  activeFile.value = att;
  const ext = att.name.split(".").pop()?.toLowerCase() || "";
  const isImage = IMAGE_WHITELIST.includes(ext) || (att.type || "").startsWith("image/");
  const isText = isPreviewableText(att);

  if (isImage || isText) {
    activeFile.value = att;
    isViewerOpen.value = true;
  } else {
    // 重型文档、音视频及其他所有类型秒开外部原始应用，免除弹窗
    openExternal(att.internalPath || att.src);
  }
};

const openExternal = async (path: string) => {
  if (nativeBusy.value) return;
  if (!path) {
    notificationStore.addNotification({
      type: "error",
      title: "无法打开附件",
      message: "附件没有可用的本地文件路径",
    });
    return;
  }
  if (!isAndroid && !isIOS) {
    notificationStore.addNotification({
      type: "info",
      title: "原生文件打开暂不可用",
      message: "当前预览环境不提供原生文件打开。",
      toastOnly: true,
    });
    return;
  }
  nativeBusy.value = true;
  try {
    const result = await invoke<NativeFileActionResult>("open_file", { path, fileName: activeFile.value?.name });
    unsupportedPreviewPath.value = result.outcome === "cannotPreview" ? path : "";
  } catch (e) {
    if (String(e).toLowerCase().includes("cancel")) return;
    console.error("[AttachmentPreview] Open failed:", e);
    notificationStore.addNotification({
      type: "error",
      title: "无法打开附件",
      message: e instanceof Error ? e.message : String(e),
    });
  } finally {
    nativeBusy.value = false;
  }
};

const handoffAttachment = async (action: "share" | "export") => {
  const path = unsupportedPreviewPath.value || activeFile.value?.internalPath || activeFile.value?.src;
  if (!path || nativeBusy.value) return;
  nativeBusy.value = true;
  try {
    if (action === "share") {
      const result = await shareFileNative(path, activeFile.value?.name);
      if (result.outcome === "cancelled") return;
      notificationStore.addNotification({
        type: "success", toastOnly: true,
        message: result.outcome === "completed" ? "已交给所选应用" : "已打开系统分享面板",
      });
    } else {
      const result = await invoke<{ displayName: string }>("export_attachment_file", {
        path, fileName: activeFile.value?.name,
      });
      notificationStore.addNotification({
        type: "success", toastOnly: true,
        message: `${isIOS ? '已保存到所选文件位置' : '已保存到下载目录'}：${result.displayName}`,
      });
    }
    unsupportedPreviewPath.value = "";
  } catch (error) {
    if (!String(error).toLowerCase().includes("cancel")) {
      notificationStore.addNotification({ type: "error", message: String(error), toastOnly: true });
    }
  } finally {
    nativeBusy.value = false;
  }
};

const removeAttachment = async (index: number) => {
  const att = props.attachments[index];
  if (
    !att ||
    !att.hash ||
    !props.messageId ||
    !props.topicId
  ) return;

  const historyStore = useChatHistoryStore();
  const messageKey = historyStore.captureMessageActionKey(props.messageId);
  if (!messageKey || messageKey.conversation.topicId !== props.topicId) return;

  const confirmed = await overlayStore.showConfirm({
    title: "移除附件",
    message: "是否确定要从这条消息中移除该附件？其他消息对同一文件的引用不会受到影响。",
    isDanger: true
  });
  if (!confirmed) return;

  try {
    await historyStore.deleteAttachment(messageKey, att.attachmentOrder ?? index, att.hash);
  } catch (err) {
    console.error("[AttachmentPreview] Failed to delete attachment:", err);
    notificationStore.addNotification({
      type: "error",
      message: "删除附件失败，请重试",
      toastOnly: true
    });
  }
};
</script>

<template>
  <div
    class="vcp-attachment-preview flex flex-wrap gap-3 mt-3 w-full max-w-full overflow-hidden"
  >
    <div v-if="unsupportedPreviewPath" class="w-full border-l-2 border-gray-400 pl-3 py-2 text-xs">
      <p class="mb-2 opacity-70">系统无法预览此文件，附件仍保留。</p>
      <div class="flex flex-wrap items-center gap-3">
        <button :disabled="nativeBusy" class="min-h-11 underline disabled:opacity-40" @click="handoffAttachment('share')">分享给其他应用</button>
        <button :disabled="nativeBusy" class="min-h-11 underline disabled:opacity-40" @click="handoffAttachment('export')">导出到文件</button>
        <button :disabled="nativeBusy" class="min-h-11 opacity-60" @click="unsupportedPreviewPath = ''">关闭</button>
      </div>
    </div>
    <div
      v-for="(att, index) in (attachments || []).filter(Boolean)"
      :key="`${att?.hash || 'attachment'}:${att?.attachmentOrder ?? index}`"
      class="attachment-item relative group"
    >
      <div
        v-if="att.status === 'desktop_only'"
        class="relative min-w-48 max-w-full border-l-2 border-gray-400/60 bg-black/5 dark:bg-white/5 pl-3 pr-9 py-2"
      >
        <div class="truncate text-xs font-medium text-gray-700 dark:text-gray-300">
          {{ att.name }}
        </div>
        <div class="mt-0.5 text-[10px] font-mono text-gray-500">
          桌面专用附件 · 未同步文件内容
        </div>
        <button
          v-if="props.messageId"
          type="button"
          class="absolute right-1.5 top-1.5 p-1 text-gray-400 hover:text-red-500 active:text-red-600"
          aria-label="移除附件"
          @click.stop="removeAttachment(index)"
        >
          <X :size="14" />
        </button>
      </div>
      <AttachmentRenderer
        v-else
        data-longpress-native
        :file="att"
        :index="index"
        :show-remove="!!props.messageId"
        @remove="removeAttachment"
        @click="openViewer(att)"
      />
    </div>

    <Teleport to="#vcp-feature-overlays">
      <AttachmentViewer
        :file="activeFile"
        :is-open="isViewerOpen"
        @close="isViewerOpen = false"
        @open-external="openExternal"
        @share-file="handoffAttachment('share')"
        @export-file="handoffAttachment('export')"
      />
    </Teleport>
  </div>
</template>

<style scoped>
audio::-webkit-media-controls-enclosure {
  background-color: rgba(255, 255, 255, 0.05);
}
</style>
