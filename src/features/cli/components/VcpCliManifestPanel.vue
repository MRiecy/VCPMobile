<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { openFileNative, writeTempFile } from "tauri-plugin-vcp-mobile";
import {
  Copy,
  FileOutput,
  RefreshCw,
  Share2,
} from "lucide-vue-next";
import {
  VCP_CLI_MANIFEST_COMMAND,
  manifestExportFileName,
  parseCanonicalVcpCliManifest,
  type VcpCliManifestDocument,
} from "../manifest";

const props = defineProps<{ isOpen: boolean }>();

type FeedbackTone = "success" | "warning" | "error";

const manifestDocument = ref<VcpCliManifestDocument | null>(null);
const loading = ref(false);
const loadError = ref("");
const copyBusy = ref(false);
const exportBusy = ref(false);
const shareBusy = ref(false);
const actionFeedback = ref<{ tone: FeedbackTone; message: string } | null>(
  null,
);
let loadGeneration = 0;

const canUseSystemShare = computed(() => typeof navigator.share === "function");
const manifest = computed(() => manifestDocument.value?.manifest ?? null);
const manifestSizeLabel = computed(() => {
  if (!manifestDocument.value) return "—";
  return `${manifestDocument.value.byteLength.toLocaleString()} B`;
});

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function setFeedback(tone: FeedbackTone, message: string): void {
  actionFeedback.value = { tone, message };
}

async function loadManifest(): Promise<void> {
  const generation = ++loadGeneration;
  loading.value = true;
  loadError.value = "";
  actionFeedback.value = null;

  try {
    const payload = await invoke<unknown>(VCP_CLI_MANIFEST_COMMAND);
    const parsed = parseCanonicalVcpCliManifest(payload);
    if (generation !== loadGeneration) return;
    manifestDocument.value = parsed;
  } catch (error) {
    if (generation !== loadGeneration) return;
    manifestDocument.value = null;
    loadError.value = describeError(error);
  } finally {
    if (generation === loadGeneration) loading.value = false;
  }
}

async function copyManifest(): Promise<void> {
  if (!manifestDocument.value || copyBusy.value) return;
  copyBusy.value = true;
  try {
    await navigator.clipboard.writeText(manifestDocument.value.rawJson);
    setFeedback(
      "success",
      "规范 manifest 已原样复制。请在 VCPToolBox 侧导入或放置。",
    );
  } catch (error) {
    setFeedback("error", `复制失败：${describeError(error)}`);
  } finally {
    copyBusy.value = false;
  }
}

async function exportManifest(): Promise<void> {
  if (!manifestDocument.value || exportBusy.value) return;
  exportBusy.value = true;
  try {
    const bytes = new TextEncoder().encode(manifestDocument.value.rawJson);
    const fileName = manifestExportFileName(
      manifestDocument.value.manifest.version,
    );
    const path = await writeTempFile(bytes, fileName);
    await openFileNative(path);
    setFeedback("success", "已生成临时 JSON，并交给系统中的可用应用打开。");
  } catch (error) {
    setFeedback("error", `导出失败：${describeError(error)}`);
  } finally {
    exportBusy.value = false;
  }
}

async function shareManifest(): Promise<void> {
  if (!manifestDocument.value || !canUseSystemShare.value || shareBusy.value)
    return;
  shareBusy.value = true;
  try {
    await navigator.share({
      title: `${manifestDocument.value.manifest.displayName} manifest`,
      text: manifestDocument.value.rawJson,
    });
    setFeedback("success", "系统分享面板已完成交付。");
  } catch (error) {
    if (error instanceof DOMException && error.name === "AbortError") return;
    setFeedback("error", `分享失败：${describeError(error)}`);
  } finally {
    shareBusy.value = false;
  }
}

watch(
  () => props.isOpen,
  (isOpen) => {
    if (!isOpen) {
      loadGeneration += 1;
      return;
    }
    void loadManifest();
  },
  { immediate: true },
);

onBeforeUnmount(() => {
  loadGeneration += 1;
});
</script>

<template>
  <section class="flex min-h-0 flex-1 flex-col" aria-label="VCP CLI Manifest">
    <div class="min-h-0 flex-1 overflow-y-auto px-3 py-3 no-rubber-band">
      <div
        v-if="loading"
        class="flex min-h-48 items-center justify-center gap-2 text-[12px] font-semibold opacity-55"
        role="status"
      >
        <RefreshCw :size="15" class="animate-spin" />
        正在读取规范 manifest…
      </div>

      <section
        v-else-if="loadError"
        class="rounded-xl border border-red-500/25 bg-red-500/5 px-3 py-3"
        role="alert"
      >
        <p class="text-[12px] font-bold text-red-500">manifest 暂不可用</p>
        <p class="mt-1 break-words font-mono text-[10px] leading-5 opacity-70">
          {{ loadError }}
        </p>
        <button
          type="button"
          class="mt-3 inline-flex min-h-10 items-center gap-2 rounded-xl border border-red-500/25 px-3 text-[11px] font-bold text-red-500 active:opacity-70"
          @click="loadManifest"
        >
          <RefreshCw :size="14" />
          重新读取
        </button>
      </section>

      <template v-else-if="manifestDocument && manifest">
        <section
          class="overflow-hidden rounded-xl border border-black/10 dark:border-white/10"
        >
          <div
            class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 px-3 py-3 text-[10px]"
          >
            <span class="font-bold uppercase tracking-[0.12em] opacity-40"
              >Tool</span
            >
            <code class="min-w-0 break-all font-mono text-[11px] font-bold">{{
              manifest.name
            }}</code>
            <span class="font-bold uppercase tracking-[0.12em] opacity-40"
              >Version</span
            >
            <code class="font-mono text-[11px]">{{ manifest.version }}</code>
            <span class="font-bold uppercase tracking-[0.12em] opacity-40"
              >Bytes</span
            >
            <code class="font-mono text-[11px]">{{ manifestSizeLabel }}</code>
          </div>
          <p
            class="border-t border-black/10 px-3 py-2 text-[10px] leading-5 opacity-55 dark:border-white/10"
          >
            规范 JSON 只描述工具协议；当前可用性与 Job 状态以运行页的 Rust
            Runtime 为准。
          </p>
        </section>

        <section
          class="mt-3 overflow-hidden rounded-xl border border-black/10 dark:border-white/10"
        >
          <div
            class="flex min-h-11 items-center justify-between gap-3 border-b border-black/10 px-3 dark:border-white/10"
          >
            <div>
              <h2 class="text-[11px] font-bold">规范 JSON</h2>
              <p class="font-mono text-[8px] opacity-40">
                backend canonical serializer
              </p>
            </div>
            <button
              type="button"
              class="flex h-9 w-9 items-center justify-center rounded-lg opacity-55 active:opacity-100"
              aria-label="刷新 manifest"
              @click="loadManifest"
            >
              <RefreshCw :size="14" />
            </button>
          </div>
          <pre
            class="no-swipe max-h-[42vh] overflow-auto bg-black/[0.03] px-3 py-3 font-mono text-[9px] leading-[1.55] text-[var(--primary-text)] dark:bg-white/[0.03]"
            data-vcp-cli-role="manifest-json"
          ><code>{{ manifestDocument.rawJson }}</code></pre>
        </section>
      </template>
    </div>

    <footer
      class="shrink-0 border-t border-black/10 bg-[var(--primary-bg)] px-3 pb-[calc(var(--vcp-safe-bottom,48px)+10px)] pt-2 dark:border-white/10"
    >
      <p
        v-if="actionFeedback"
        class="mb-2 min-h-5 text-[10px] leading-5"
        :class="{
          'text-emerald-600 dark:text-emerald-400':
            actionFeedback.tone === 'success',
          'text-amber-600 dark:text-amber-400':
            actionFeedback.tone === 'warning',
          'text-red-500': actionFeedback.tone === 'error',
        }"
        role="status"
      >
        {{ actionFeedback.message }}
      </p>
      <div class="grid grid-cols-3 gap-2">
        <button
          type="button"
          class="flex min-h-11 items-center justify-center gap-1.5 rounded-xl bg-[var(--highlight-text)] px-2 text-[10px] font-bold text-white active:opacity-75 disabled:opacity-35"
          data-vcp-cli-action="copy"
          :disabled="!manifestDocument || loading || copyBusy"
          @click="copyManifest"
        >
          <Copy :size="14" />
          {{ copyBusy ? "复制中" : "复制" }}
        </button>
        <button
          type="button"
          class="flex min-h-11 items-center justify-center gap-1.5 rounded-xl border border-black/10 px-2 text-[10px] font-bold active:opacity-70 disabled:opacity-35 dark:border-white/10"
          data-vcp-cli-action="export"
          :disabled="!manifestDocument || loading || exportBusy"
          @click="exportManifest"
        >
          <FileOutput :size="14" />
          {{ exportBusy ? "导出中" : "导出 JSON" }}
        </button>
        <button
          type="button"
          class="flex min-h-11 items-center justify-center gap-1.5 rounded-xl border border-black/10 px-2 text-[10px] font-bold active:opacity-70 disabled:opacity-35 dark:border-white/10"
          data-vcp-cli-action="share"
          :disabled="
            !manifestDocument || loading || shareBusy || !canUseSystemShare
          "
          @click="shareManifest"
        >
          <Share2 :size="14" />
          {{ shareBusy ? "分享中" : "系统分享" }}
        </button>
      </div>
      <p class="mt-2 text-[9px] leading-4 opacity-45">
        导出会创建应用缓存内的临时 JSON 并交给系统应用打开；
        <template v-if="!canUseSystemShare">
          当前 WebView 未提供系统分享，Android
          <code class="font-mono">ACTION_SEND</code> 原生接口尚未接入。
        </template>
        <template v-else>系统分享发送的是同一份规范文本。</template>
      </p>
    </footer>
  </section>
</template>
