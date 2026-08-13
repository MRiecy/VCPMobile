<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { openFileNative, writeTempFile } from "tauri-plugin-vcp-mobile";
import {
  ChevronDown,
  Copy,
  FileOutput,
  RefreshCw,
  Share2,
  SquareTerminal,
  X,
} from "lucide-vue-next";
import SlidePage from "../../../components/ui/SlidePage.vue";
import {
  LOCAL_ROUTE_GUIDE_STORAGE_KEY,
  VCP_CLI_MANIFEST_COMMAND,
  manifestExportFileName,
  parseCanonicalVcpCliManifest,
  type VcpCliManifestDocument,
} from "../manifest";

const props = defineProps<{
  isOpen: boolean;
  zIndex: number;
}>();

const emit = defineEmits<{
  close: [];
}>();

type FeedbackTone = "success" | "warning" | "error";

const manifestDocument = ref<VcpCliManifestDocument | null>(null);
const loading = ref(false);
const loadError = ref("");
const guideAcknowledged = ref(false);
const guideExpanded = ref(false);
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

function readGuidePreference(): void {
  try {
    guideAcknowledged.value =
      localStorage.getItem(LOCAL_ROUTE_GUIDE_STORAGE_KEY) === "1";
  } catch {
    guideAcknowledged.value = false;
  }
  guideExpanded.value = !guideAcknowledged.value;
}

function acknowledgeGuide(): void {
  try {
    localStorage.setItem(LOCAL_ROUTE_GUIDE_STORAGE_KEY, "1");
  } catch {
    // The guide can still collapse for this session when storage is unavailable.
  }
  guideAcknowledged.value = true;
  guideExpanded.value = false;
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
    if (!isOpen) return;
    readGuidePreference();
    void loadManifest();
  },
  { immediate: true },
);

onBeforeUnmount(() => {
  loadGeneration += 1;
});
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex">
    <main
      class="flex h-full min-h-0 w-full flex-col bg-[var(--primary-bg)] text-[var(--primary-text)]"
      aria-labelledby="vcp-cli-manifest-title"
    >
      <header
        class="shrink-0 border-b border-black/10 px-4 pb-3 pt-[calc(var(--vcp-safe-top,24px)+10px)] dark:border-white/10"
      >
        <div class="flex min-w-0 items-center gap-3">
          <div
            class="flex h-9 w-9 shrink-0 items-center justify-center rounded-xl border border-black/10 bg-[var(--secondary-bg)] dark:border-white/10"
            aria-hidden="true"
          >
            <SquareTerminal :size="18" class="text-[var(--highlight-text)]" />
          </div>
          <div class="min-w-0 flex-1">
            <p
              class="text-[9px] font-black uppercase tracking-[0.18em] opacity-45"
            >
              Protocol asset
            </p>
            <h1
              id="vcp-cli-manifest-title"
              class="truncate text-[17px] font-bold"
            >
              VCP CLI manifest
            </h1>
          </div>
          <button
            type="button"
            class="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl opacity-55 transition-opacity active:opacity-100"
            aria-label="关闭 VCP CLI manifest"
            @click="emit('close')"
          >
            <X :size="20" />
          </button>
        </div>
      </header>

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
          <p
            class="mt-1 break-words font-mono text-[10px] leading-5 opacity-70"
          >
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
                >Route</span
              >
              <code class="font-mono text-[11px]"
                >local_loopback / vcp_plugin</code
              >
              <span class="font-bold uppercase tracking-[0.12em] opacity-40"
                >Bytes</span
              >
              <code class="font-mono text-[11px]">{{ manifestSizeLabel }}</code>
            </div>
            <p
              class="border-t border-black/10 px-3 py-2 text-[10px] leading-5 opacity-55 dark:border-white/10"
            >
              本页只交付协议声明，不代表 CLI Runtime、Job 或人工终端已经可用。
            </p>
          </section>

          <section
            class="mt-3 overflow-hidden rounded-xl border border-black/10 dark:border-white/10"
          >
            <button
              type="button"
              class="flex min-h-12 w-full items-center gap-3 px-3 text-left active:bg-black/5 dark:active:bg-white/5"
              :aria-expanded="guideExpanded"
              aria-controls="vcp-cli-local-route-guide"
              @click="guideExpanded = !guideExpanded"
            >
              <div class="min-w-0 flex-1">
                <div class="flex items-center gap-2">
                  <span class="text-[12px] font-bold"
                    >VCPToolBox 本地路由首次配置</span
                  >
                  <span
                    class="rounded-md bg-black/5 px-1.5 py-0.5 font-mono text-[8px] font-bold uppercase dark:bg-white/10"
                  >
                    {{ guideAcknowledged ? "已读" : "首次指引" }}
                  </span>
                </div>
                <p class="mt-0.5 text-[9px] opacity-45">
                  只提供步骤，不代替用户修改提示词
                </p>
              </div>
              <ChevronDown
                :size="16"
                class="shrink-0 opacity-45 transition-transform"
                :class="guideExpanded ? 'rotate-180' : ''"
              />
            </button>

            <div
              v-if="guideExpanded"
              id="vcp-cli-local-route-guide"
              class="border-t border-black/10 px-3 py-3 dark:border-white/10"
            >
              <ol class="space-y-2 text-[10px] leading-5 opacity-75">
                <li>
                  <span class="mr-2 font-mono font-bold">01</span
                  >复制或导出本页的规范 manifest。
                </li>
                <li>
                  <span class="mr-2 font-mono font-bold">02</span>在 VCPToolBox
                  的 local-route 预设或工具说明处导入/放置其中的
                  <code class="font-mono">description</code> 与
                  <code class="font-mono">example</code>，并由你按实际部署微调。
                </li>
                <li>
                  <span class="mr-2 font-mono font-bold">03</span
                  >确保中央工具循环不执行
                  <code class="font-mono">VCPMobileCLI</code>。候选标记
                  <code class="font-mono">[[VCPToolUse=Forbidden]]</code>
                  需在你的 VCPToolBox 版本中验证。
                </li>
                <li>
                  <span class="mr-2 font-mono font-bold">04</span>本地路由使用
                  <code class="font-mono">local_loopback</code>；无需为此开启
                  Distributed 或 WS。
                </li>
              </ol>

              <div
                class="mt-3 border-l-2 border-[var(--highlight-text)] pl-3 text-[10px] leading-5"
              >
                <strong class="block text-[11px]"
                  >提示词由用户 / VCPToolBox 所有</strong
                >
                <span class="opacity-60">
                  VCPMobile 不会自动注入、追加或改写 Agent
                  提示词，也不会因打开本页建立插件中心连接。
                </span>
              </div>

              <button
                type="button"
                class="mt-3 min-h-10 rounded-xl border border-black/10 px-3 text-[10px] font-bold active:opacity-70 dark:border-white/10"
                @click="acknowledgeGuide"
              >
                我知道了，不再自动展开
              </button>
            </div>
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
    </main>
  </SlidePage>
</template>
