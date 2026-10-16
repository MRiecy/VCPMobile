<script setup lang="ts">
import { computed, watch, ref } from "vue";
import { useModalHistory } from "../../../core/composables/useModalHistory";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { X, ExternalLink, Download, Share2, FileDown } from "lucide-vue-next";
import { saveImageFromPath } from "tauri-plugin-vcp-mobile";
import { useNotificationStore } from "../../../core/stores/notification";
import { isAndroid, isIOS } from "../../../core/utils/platform";
import type { Attachment } from "../../../core/types/chat";

const props = defineProps<{
  file: Attachment | null;
  isOpen: boolean;
}>();

const emit = defineEmits(["close", "open-external", "share-file", "export-file"]);

const { registerModal, unregisterModal } = useModalHistory();
const modalId = 'AttachmentViewer';
const notificationStore = useNotificationStore();
const hasLocalCapability = computed(() =>
  !!props.file &&
  props.file.status !== "desktop_only" &&
  !!(props.file.internalPath || props.file.src)
);

const previewText = ref("");
const isTextTruncated = ref(false);
const isLoading = ref(false);

const IMAGE_WHITELIST = ["jpg", "jpeg", "png", "gif", "webp", "svg", "bmp", "heic", "heif", "avif"];
const TEXT_WHITELIST = [
  "txt", "md", "csv", "json", "js", "ts", "py", "rs", "java", "c", "cpp",
  "h", "go", "rb", "php", "swift", "kt", "html", "css", "xml", "yaml",
  "yml", "toml", "ini", "log", "sql", "vue", "jsx", "tsx"
];

const isImage = computed(() => {
  if (!props.file) return false;
  const ext = props.file.name.split(".").pop()?.toLowerCase() || "";
  return IMAGE_WHITELIST.includes(ext) || (props.file.type || "").startsWith("image/");
});

const isText = computed(() => {
  if (!props.file) return false;
  const ext = props.file.name.split(".").pop()?.toLowerCase() || "";
  
  // 核心加固：若存在后缀且完全不属于文本白名单，绝不判定为文本（与 Preview 判定主权一致）
  if (ext && !TEXT_WHITELIST.includes(ext)) {
    return false;
  }
  
  if (TEXT_WHITELIST.includes(ext)) {
    return true;
  }
  
  const type = (props.file.type || "").toLowerCase();
  return (
    type.startsWith("text/") ||
    type === "application/json" ||
    type === "application/javascript" ||
    type === "application/x-javascript"
  );
});

const renderSrc = ref("");
const previewError = ref("");
const close = () => emit("close");
watch([() => props.isOpen, () => props.file], async ([open, file], _old, onCleanup) => {
  let active = true;
  let registered = false;
  let ownedImagePath: string | undefined;
  onCleanup(() => {
    active = false;
    if (registered) unregisterModal(modalId);
    if (ownedImagePath) void invoke("release_attachment_image", { path: ownedImagePath }).catch(console.warn);
  });
  previewText.value = "";
  renderSrc.value = "";
  previewError.value = "";
  isTextTruncated.value = false;
  isLoading.value = false;
  if (!open || !file) return;
  if (!hasLocalCapability.value) { close(); return; }
  registerModal(modalId, close);
  registered = true;
  if (!file.hash) {
    previewError.value = "附件缺少本地资源标识，请使用系统预览。";
    return;
  }
  isLoading.value = true;
  try {
    if (isText.value) {
      const result = await invoke<{ text: string; truncated: boolean; totalBytes: number }>(
        "read_attachment_text_preview", { hash: file.hash },
      );
      if (!active) return;
      previewText.value = result.text;
      isTextTruncated.value = result.truncated;
    } else if (isImage.value) {
      const target = Math.min(2048, Math.ceil(Math.max(window.innerWidth, window.innerHeight) * window.devicePixelRatio));
      const image = await invoke<{ path: string }>("get_attachment_image", {
        hash: file.hash, purpose: "display", maxDimension: target,
      });
      if (!active) {
        await invoke("release_attachment_image", { path: image.path }).catch(console.warn);
        return;
      }
      ownedImagePath = image.path;
      renderSrc.value = convertFileSrc(image.path);
    }
  } catch (error) {
    if (active) previewError.value = `无法加载预览：${String(error)}`;
  } finally {
    if (active) isLoading.value = false;
  }
}, { immediate: true });

const isSaving = ref(false);

const saveToAlbum = async () => {
  if (!props.file || !hasLocalCapability.value || isSaving.value) return;
  isSaving.value = true;
  try {
    const sourceUrl = props.file.internalPath || props.file.src;
    if (!sourceUrl) {
      throw new Error("图片路径为空");
    }
    await saveImageFromPath(sourceUrl, props.file.name);
    notificationStore.addNotification({
      type: "success",
      title: "保存成功",
      message: "图片已保存到相册",
      toastOnly: true,
    });
  } catch (err) {
    if (String(err).toLowerCase().includes("cancel")) return;
    console.error("[AttachmentViewer] Failed to save image:", err);
    notificationStore.addNotification({
      type: "error",
      title: "保存失败",
      message: String(err),
      toastOnly: true,
    });
  } finally {
    isSaving.value = false;
  }
};
</script>

<template>
  <Transition name="viewer-fade">
    <div
      v-show="isOpen && file && hasLocalCapability"
      class="vcp-attachment-viewer vcp-viewport-surface vcp-safe-inline fixed inset-0 z-viewer flex flex-col pointer-events-auto"
      @click.self="close"
    >
      <!-- Toolbar -->
      <div
        class="flex items-center justify-between px-4 pt-[calc(var(--vcp-safe-top,0px)+8px)] pb-3 border-b border-black/5 dark:border-white/5 shrink-0 z-10"
      >
        <div class="flex flex-col overflow-hidden mr-4 min-w-0">
          <span class="text-sm font-bold text-gray-800 dark:text-gray-200 truncate">{{
            file?.name
          }}</span>
          <span class="text-[10px] text-gray-400 dark:text-gray-500 uppercase tracking-widest">{{
            file?.type
          }}</span>
        </div>
        <div class="flex items-center gap-1">
          <button
            v-if="hasLocalCapability && (isAndroid || isIOS)"
            class="min-h-11 min-w-11 flex items-center justify-center text-gray-500 active:opacity-60"
            title="分享文件"
            aria-label="分享文件"
            @click="emit('share-file')"
          ><Share2 :size="20" /></button>
          <button
            v-if="hasLocalCapability && (isAndroid || isIOS)"
            class="min-h-11 min-w-11 flex items-center justify-center text-gray-500 active:opacity-60"
            title="导出到文件"
            aria-label="导出到文件"
            @click="emit('export-file')"
          ><FileDown :size="20" /></button>
          <button
            v-if="isImage && hasLocalCapability && (isAndroid || isIOS)"
            @click="saveToAlbum"
            :disabled="isSaving"
            class="p-2 -mr-1 rounded-full text-gray-500 hover:text-gray-800 dark:text-gray-400 dark:hover:text-gray-200 transition-colors active:bg-black/5 dark:active:bg-white/5 disabled:opacity-40"
            title="保存到相册"
          >
            <Download :size="20" />
          </button>
          <button
            v-if="hasLocalCapability && (isAndroid || isIOS)"
            @click="$emit('open-external', file?.internalPath || file?.src)"
            class="p-2 -mr-1 rounded-full text-gray-500 hover:text-gray-800 dark:text-gray-400 dark:hover:text-gray-200 transition-colors active:bg-black/5 dark:active:bg-white/5"
          >
            <ExternalLink :size="20" />
          </button>
          <button
            @click="close"
            class="p-2 -mr-2 rounded-full text-gray-500 hover:text-gray-800 dark:text-gray-400 dark:hover:text-gray-200 transition-colors active:bg-black/5 dark:active:bg-white/5"
          >
            <X :size="24" />
          </button>
        </div>
      </div>

      <!-- Main Content -->
      <div
        class="flex-1 overflow-auto vcp-scrollable no-rubber-band pb-[calc(var(--vcp-safe-bottom,48px))]"
      >
        <!-- Text/Code/MD Viewer -->
        <p v-if="previewError" class="px-4 py-6 text-sm text-red-500 select-text">{{ previewError }}</p>
        <div
          v-else-if="isText"
          class="w-full px-4 py-4 flex flex-col gap-3 min-h-full"
        >
          <!-- 截断友好提示 -->
          <div
            v-if="isTextTruncated"
            class="px-3 py-2 bg-yellow-500/10 border border-yellow-500/20 text-yellow-700 dark:text-yellow-400 text-xs rounded-lg flex items-center justify-between shrink-0"
          >
            <span>当前预览前 128 KiB。<template v-if="isAndroid || isIOS">要阅读全文，请使用系统预览。</template></span>
            <button
              v-if="isAndroid || isIOS"
              @click="$emit('open-external', props.file?.internalPath || props.file?.src)"
              class="px-2 py-1 bg-yellow-500/20 hover:bg-yellow-500/30 rounded text-[10px] font-bold active:scale-95 transition-transform shrink-0 ml-2"
            >
              系统预览
            </button>
          </div>

          <!-- Loading state -->
          <div v-if="isLoading" class="flex-1 flex items-center justify-center p-12">
            <span class="text-xs text-gray-400 animate-pulse font-mono">正在加载预览流...</span>
          </div>

          <!-- Text content container -->
          <pre
            v-else
            class="flex-1 font-mono text-[13px] whitespace-pre-wrap select-text leading-relaxed opacity-90 p-4 break-all overflow-x-auto"
          >{{ previewText }}</pre>
        </div>

        <!-- Image Viewer -->
        <div
          v-else-if="isImage"
          class="h-full w-full flex items-center justify-center p-4"
        >
          <img
            v-if="renderSrc"
            :src="renderSrc"
            class="max-w-full max-h-full object-contain"
            @click.stop
          />
          <span v-else-if="isLoading" class="text-xs opacity-50">正在准备图片...</span>
        </div>


      </div>
    </div>
  </Transition>
</template>

<style scoped>
.vcp-attachment-viewer {
  background: linear-gradient(145deg, #f9f9fb, #f2f2f7);
  color: #333;
}

html.dark .vcp-attachment-viewer {
  background: linear-gradient(145deg, #1c1c1e, #2c2c2e);
  color: #f2f2f7;
}

.viewer-fade-enter-active,
.viewer-fade-leave-active {
  transition: all 0.3s cubic-bezier(0.16, 1, 0.3, 1);
}

.viewer-fade-enter-from,
.viewer-fade-leave-to {
  opacity: 0;
  transform: scale(1.05);
}

.animate-zoom-in {
  animation: zoomIn 0.4s cubic-bezier(0.16, 1, 0.3, 1);
}

@keyframes zoomIn {
  from {
    opacity: 0;
    transform: scale(0.9);
  }

  to {
    opacity: 1;
    transform: scale(1);
  }
}
</style>
