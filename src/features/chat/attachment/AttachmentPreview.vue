<script setup lang="ts">
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import AttachmentViewer from "./AttachmentViewer.vue";
import AttachmentRenderer from './AttachmentRenderer.vue';

import { useChatHistoryStore } from "../../../core/stores/chatHistoryStore";

interface Attachment {
  type: string;
  src: string;
  resolvedSrc?: string;
  name: string;
  size: number;
  hash?: string;
  extractedText?: string;
  thumbnailPath?: string;
  id?: string;
  progress?: number;
  status?: string;
  internalPath?: string;
  imageFrames?: string[];
  createdAt?: number;
}

const props = defineProps<{
  attachments: Attachment[];
  messageId?: string;
  topicId?: string;
}>();

const isViewerOpen = ref(false);
const activeFile = ref<Attachment | null>(null);

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
  try {
    await invoke("open_file", { path });
  } catch (e) {
    console.error("[AttachmentPreview] Open failed:", e);
  }
};

const removeAttachment = async (index: number) => {
  const att = props.attachments[index];
  if (!att || !att.hash || !props.messageId || !props.topicId) return;

  const confirmed = confirm("是否确定要移除该附件？该操作仅会隐藏历史消息中的附件，您仍能继续使用此模型进行对话。");
  if (!confirmed) return;

  try {
    const historyStore = useChatHistoryStore();
    await historyStore.deleteAttachment(props.topicId, props.messageId, att.hash);
  } catch (err) {
    console.error("[AttachmentPreview] Failed to delete attachment:", err);
    alert("删除附件失败，请重试");
  }
};
</script>

<template>
  <div
    class="vcp-attachment-preview flex flex-wrap gap-3 mt-3 w-full max-w-full overflow-hidden"
  >
    <div
      v-for="(att, index) in attachments"
      :key="index"
      class="attachment-item relative group"
      @click="openViewer(att)"
    >
      <AttachmentRenderer
        :file="att"
        :index="index"
        :show-remove="!!props.messageId"
        @remove="removeAttachment"
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
