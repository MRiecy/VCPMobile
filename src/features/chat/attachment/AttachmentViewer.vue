<script setup lang="ts">
import { computed, watch, ref } from "vue";
import { useModalHistory } from "../../../core/composables/useModalHistory";
import { convertFileSrc } from "@tauri-apps/api/core";
import { X, ExternalLink } from "lucide-vue-next";

interface Attachment {
  type: string;
  src: string;
  name: string;
  size: number;
  extractedText?: string;
  internalPath?: string;
}

const props = defineProps<{
  file: Attachment | null;
  isOpen: boolean;
}>();

const emit = defineEmits(["close", "open-external"]);

const { registerModal, unregisterModal } = useModalHistory();
const modalId = 'AttachmentViewer';

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

watch(() => props.isOpen, async (newVal) => {
  if (newVal) {
    registerModal(modalId, close);
    previewText.value = "";
    isTextTruncated.value = false;
    
    // 如果是可预览的文本，开始流式读取物理文件的前 128KB 进行预览
    if (isText.value && props.file) {
      isLoading.value = true;
      try {
        const sourcePath = props.file.internalPath || props.file.src;
        if (sourcePath) {
          let fetchUrl = sourcePath;
          if (
            !sourcePath.startsWith("http") &&
            !sourcePath.startsWith("blob:") &&
            !sourcePath.startsWith("data:")
          ) {
            fetchUrl = convertFileSrc(sourcePath.replace("file://", ""));
          }
          
          const response = await fetch(fetchUrl);
          const reader = response.body?.getReader();
          if (reader) {
            const chunks: Uint8Array[] = [];
            let receivedLength = 0;
            const LIMIT = 128 * 1024; // 128KB
            
            while (receivedLength < LIMIT) {
              const { done, value } = await reader.read();
              if (done) break;
              chunks.push(value);
              receivedLength += value.length;
            }
            
            // 合并并解码为 UTF-8
            const allChunks = new Uint8Array(receivedLength);
            let position = 0;
            for (const chunk of chunks) {
              allChunks.set(chunk, position);
              position += chunk.length;
            }
            
            previewText.value = new TextDecoder("utf-8").decode(allChunks);
            if (receivedLength >= LIMIT) {
              isTextTruncated.value = true;
            }
          }
        }
      } catch (e) {
        console.error('[AttachmentViewer] Failed to load text preview:', e);
        previewText.value = "⚠️ 本地文件预览失败，请使用外部应用打开。";
      } finally {
        isLoading.value = false;
      }
    }
  } else {
    unregisterModal(modalId);
    previewText.value = "";
    isTextTruncated.value = false;
  }
});

const renderSrc = computed(() => {
  if (!props.file?.src) return "";
  if (
    props.file.src.startsWith("http") ||
    props.file.src.startsWith("data:") ||
    props.file.src.startsWith("blob:")
  )
    return props.file.src;
  try {
    return convertFileSrc(props.file.src.replace("file://", "").replace("file://", ""));
  } catch (e) {
    return "";
  }
});

const close = () => emit("close");
</script>

<template>
  <Transition name="viewer-fade">
    <div
      v-show="isOpen && file"
      class="vcp-attachment-viewer fixed inset-0 z-viewer flex flex-col bg-[#f0f4f8] dark:bg-[#121e23] pointer-events-auto"
      @click.self="close"
    >
      <!-- Toolbar -->
      <div
        class="flex items-center justify-between px-4 pt-[calc(env(safe-area-inset-top,24px)+8px)] pb-3 bg-white/80 dark:bg-gray-900/80 border-b border-black/5 dark:border-white/5 shrink-0 shadow-sm z-10"
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
        class="flex-1 overflow-auto vcp-scrollable no-rubber-band pb-[env(safe-area-inset-bottom)]"
      >
        <!-- Text/Code/MD Viewer -->
        <div
          v-if="isText"
          class="w-full px-4 py-4 flex flex-col gap-3 min-h-full"
        >
          <!-- 截断友好提示 -->
          <div
            v-if="isTextTruncated"
            class="px-3 py-2 bg-yellow-500/10 border border-yellow-500/20 text-yellow-700 dark:text-yellow-400 text-xs rounded-lg flex items-center justify-between shrink-0"
          >
            <span>📄 当前文件过大，已自动为您预览前 128KB。要阅读全文，请使用外部应用打开。</span>
            <button
              @click="$emit('open-external', props.file?.internalPath || props.file?.src)"
              class="px-2 py-1 bg-yellow-500/20 hover:bg-yellow-500/30 rounded text-[10px] font-bold active:scale-95 transition-transform shrink-0 ml-2"
            >
              外部打开
            </button>
          </div>

          <!-- Loading state -->
          <div v-if="isLoading" class="flex-1 flex items-center justify-center p-12">
            <span class="text-xs text-gray-400 animate-pulse font-mono">正在加载预览流...</span>
          </div>

          <!-- Text content container -->
          <pre
            v-else
            class="flex-1 font-mono text-[13px] whitespace-pre-wrap select-text leading-relaxed opacity-90 p-4 bg-gray-50 dark:bg-gray-900/50 rounded-lg border border-black/5 dark:border-white/5 break-all overflow-x-auto"
          >{{ previewText }}</pre>
        </div>

        <!-- Image Viewer -->
        <div
          v-else-if="isImage"
          class="h-full w-full flex items-center justify-center p-4"
        >
          <img
            :src="renderSrc"
            class="max-w-full max-h-full object-contain rounded-lg shadow-2xl animate-zoom-in"
            @click.stop
          />
        </div>


      </div>
    </div>
  </Transition>
</template>

<style scoped>
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
