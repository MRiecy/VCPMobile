<script setup lang="ts">
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { X } from "lucide-vue-next";
import AttachmentViewer from "./AttachmentViewer.vue";
import AttachmentRenderer from './AttachmentRenderer.vue';

import { useChatHistoryStore } from "../../../core/stores/chatHistoryStore";
import { useOverlayStore } from "../../../core/stores/overlay";
import { useNotificationStore } from "../../../core/stores/notification";
import type { Attachment } from "../../../core/types/chat";

const props = defineProps<{
  attachments: Attachment[];
  messageId?: string;
  topicId?: string;
}>();

const isViewerOpen = ref(false);
const activeFile = ref<Attachment | null>(null);

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
  if (!path) {
    notificationStore.addNotification({
      type: "error",
      title: "无法打开附件",
      message: "附件没有可用的本地文件路径",
    });
    return;
  }
  try {
    await invoke("open_file", { path });
  } catch (e) {
    console.error("[AttachmentPreview] Open failed:", e);
    notificationStore.addNotification({
      type: "error",
      title: "无法打开附件",
      message: e instanceof Error ? e.message : String(e),
    });
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
      />
    </Teleport>
  </div>
</template>

<style scoped>
audio::-webkit-media-controls-enclosure {
  background-color: rgba(255, 255, 255, 0.05);
}
</style>
