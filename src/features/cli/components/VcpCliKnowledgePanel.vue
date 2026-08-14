<script setup lang="ts">
import { storeToRefs } from "pinia";
import { computed } from "vue";
import { FileKey2, RefreshCw, ShieldMinus } from "lucide-vue-next";
import { useOverlayStore } from "../../../core/stores/overlay";
import { useVcpCliStore, type VcpCliKnowledgeSource } from "../vcpCliStore";

const store = useVcpCliStore();
const overlayStore = useOverlayStore();
const {
  knowledgeCatalog,
  knowledgeLoading,
  knowledgeError,
  knowledgeImportCandidate,
  knowledgeMutationBusy,
  knowledgeMutationError,
  knowledgeMutationNotice,
  pendingKnowledgeInspectOperationId,
  pendingKnowledgeCommitOperationId,
  pendingKnowledgeDiscardOperationId,
  pendingKnowledgeRevoke,
} = storeToRefs(store);

function compareStableText(left: string, right: string): number {
  if (left === right) return 0;
  return left < right ? -1 : 1;
}

const knowledgeSources = computed(() =>
  [...(knowledgeCatalog.value?.sources ?? [])].sort(
    (left, right) =>
      compareStableText(left.display_name, right.display_name) ||
      compareStableText(left.source_id, right.source_id),
  ),
);

const hasCandidateMutation = computed(
  () =>
    pendingKnowledgeCommitOperationId.value !== null ||
    pendingKnowledgeDiscardOperationId.value !== null,
);

function revokeDisabled(source: VcpCliKnowledgeSource): boolean {
  if (
    knowledgeMutationBusy.value ||
    pendingKnowledgeInspectOperationId.value ||
    hasCandidateMutation.value ||
    knowledgeImportCandidate.value
  ) {
    return true;
  }
  return Boolean(
    pendingKnowledgeRevoke.value &&
    pendingKnowledgeRevoke.value.sourceId !== source.source_id,
  );
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}

function statusLabel(status: VcpCliKnowledgeSource["index_status"]): string {
  if (status === "ready") return "可召回";
  if (status === "indexing") return "索引中";
  return "索引失败";
}

async function confirmKnowledgeImport(): Promise<void> {
  const candidate = knowledgeImportCandidate.value;
  if (!candidate || knowledgeMutationBusy.value) return;
  const confirmed = await overlayStore.showConfirm({
    title: "复制并授权本机知识？",
    message:
      `“${candidate.display_name}”会复制到 App 私有知识库，并同时授予 localLoopback vref 使用。` +
      "命中后文件还会复制给目标 CLI Job；命令 stdout/stderr 或模型续轮可能把其中内容发送给你当前选择的模型服务。" +
      "它不是聊天附件、同步数据或提示词；Agent 不能选择 catalog 外文件。",
  });
  if (!confirmed || knowledgeImportCandidate.value?.token !== candidate.token) {
    return;
  }
  await store.commitKnowledgeImport();
}

async function confirmRevoke(source: VcpCliKnowledgeSource): Promise<void> {
  if (knowledgeMutationBusy.value) return;
  const confirmed = await overlayStore.showConfirm({
    title: "撤销本机知识授权？",
    message:
      `“${source.display_name}”会立即从未来的 vref 召回中移除并请求删除。` +
      "已完成 Runtime admission 的 CLI Job 拥有私有副本，无法追溯收回。",
    isDanger: true,
  });
  if (!confirmed) return;
  const current = knowledgeCatalog.value?.sources.find(
    (item) => item.source_id === source.source_id,
  );
  if (!current || current.source_sha256 !== source.source_sha256) return;
  await store.revokeKnowledgeSource(current);
}
</script>

<template>
  <section
    class="flex min-h-0 flex-1 flex-col"
    aria-label="本地知识"
    :aria-busy="knowledgeLoading || knowledgeMutationBusy"
  >
    <div
      class="flex min-h-12 shrink-0 items-center gap-2 border-b border-black/10 px-3 dark:border-white/10"
    >
      <div class="min-w-0 flex-1">
        <h2 class="text-[11px] font-bold">本机授权 catalog</h2>
        <p class="mt-0.5 truncate text-[9px] opacity-45">
          仅 localLoopback vref；Runtime 能力仍以运行门为准。
        </p>
      </div>
      <button
        type="button"
        class="inline-flex min-h-9 shrink-0 items-center gap-1.5 rounded-lg border border-black/10 px-2.5 text-[9px] font-bold disabled:opacity-35 dark:border-white/10"
        data-vcp-cli-action="inspect-knowledge-import"
        :disabled="
          knowledgeMutationBusy ||
          !!knowledgeImportCandidate ||
          hasCandidateMutation ||
          !!pendingKnowledgeRevoke
        "
        @click="store.inspectKnowledgeImport"
      >
        <FileKey2 :size="13" />{{
          knowledgeMutationBusy
            ? "读取中"
            : pendingKnowledgeInspectOperationId
              ? "重试选择"
              : "选择文件"
        }}
      </button>
      <button
        type="button"
        class="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg opacity-45 disabled:opacity-25 active:opacity-100"
        aria-label="刷新本地知识"
        :disabled="knowledgeLoading || knowledgeMutationBusy"
        @click="store.loadKnowledgeCatalog"
      >
        <RefreshCw :size="14" :class="knowledgeLoading ? 'animate-spin' : ''" />
      </button>
    </div>

    <dl
      v-if="knowledgeCatalog"
      class="grid shrink-0 grid-cols-3 border-b border-black/10 px-3 py-2 text-[8px] dark:border-white/10"
      data-vcp-cli-role="knowledge-quota"
    >
      <div class="min-w-0 border-l-2 border-[var(--highlight-text)] pl-2">
        <dt class="font-bold uppercase opacity-35">Objects</dt>
        <dd class="mt-0.5 truncate font-mono opacity-70">
          {{ knowledgeCatalog.active_source_count }} /
          {{ knowledgeCatalog.active_source_limit }}
        </dd>
      </div>
      <div class="min-w-0 border-l-2 border-black/10 pl-2 dark:border-white/10">
        <dt class="font-bold uppercase opacity-35">Storage</dt>
        <dd class="mt-0.5 truncate font-mono opacity-70">
          {{ formatBytes(knowledgeCatalog.used_bytes) }} /
          {{ formatBytes(knowledgeCatalog.limit_bytes) }}
        </dd>
      </div>
      <div class="min-w-0 border-l-2 border-black/10 pl-2 dark:border-white/10">
        <dt class="font-bold uppercase opacity-35">Pending</dt>
        <dd class="mt-0.5 truncate font-mono opacity-70">
          {{ knowledgeCatalog.pending_candidate_count }} /
          {{ knowledgeCatalog.pending_candidate_limit }} ·
          {{ formatBytes(knowledgeCatalog.pending_used_bytes) }} /
          {{ formatBytes(knowledgeCatalog.pending_limit_bytes) }}
        </dd>
      </div>
    </dl>

    <div
      v-if="knowledgeImportCandidate"
      class="shrink-0 border-b border-black/10 border-l-2 border-l-[var(--highlight-text)] px-3 py-2 dark:border-white/10"
      data-vcp-cli-role="knowledge-import-review"
    >
      <div class="flex items-start gap-3">
        <div class="min-w-0 flex-1">
          <p class="truncate text-[10px] font-bold">
            {{ knowledgeImportCandidate.display_name }}
          </p>
          <p class="mt-1 font-mono text-[8px] opacity-50">
            {{ knowledgeImportCandidate.mime_type }} ·
            {{ formatBytes(knowledgeImportCandidate.size_bytes) }} ·
            {{ knowledgeImportCandidate.chunk_count }} CHUNKS
          </p>
          <p class="mt-0.5 break-all font-mono text-[8px] opacity-35">
            SHA256 {{ knowledgeImportCandidate.candidate_sha256 }}
          </p>
          <p class="mt-1 text-[9px] leading-4 opacity-65">
            确认后复制进 App 私有知识库并授予 localLoopback
            vref；当前只是候选，尚未授权。
          </p>
          <p class="mt-1 font-mono text-[8px] opacity-45">
            OBJECTS
            {{ formatBytes(knowledgeImportCandidate.used_bytes) }} /
            {{ formatBytes(knowledgeImportCandidate.limit_bytes) }} · PENDING
            {{ formatBytes(knowledgeImportCandidate.pending_used_bytes) }} /
            {{ formatBytes(knowledgeImportCandidate.pending_limit_bytes) }}
          </p>
          <p
            v-if="knowledgeImportCandidate.index_text_truncated"
            class="mt-1 text-[9px] text-amber-600 dark:text-amber-400"
          >
            原文件完整保留；索引文本受 1 MiB 上限截断。
          </p>
          <p
            v-for="warning in knowledgeImportCandidate.warnings"
            :key="warning"
            class="mt-1 text-[9px] leading-4 text-amber-600 dark:text-amber-400"
          >
            {{ warning }}
          </p>
        </div>
        <div class="flex shrink-0 flex-col gap-1.5">
          <button
            type="button"
            class="min-h-8 rounded-md bg-[var(--highlight-text)] px-2 text-[9px] font-bold text-white disabled:opacity-35"
            data-vcp-cli-action="commit-knowledge-import"
            :disabled="
              knowledgeMutationBusy || !!pendingKnowledgeDiscardOperationId
            "
            @click="confirmKnowledgeImport"
          >
            {{
              knowledgeMutationBusy
                ? "提交中"
                : pendingKnowledgeCommitOperationId
                  ? "重试授权"
                  : "复制并授权"
            }}
          </button>
          <button
            type="button"
            class="min-h-8 rounded-md border border-black/10 px-2 text-[9px] disabled:opacity-35 dark:border-white/10"
            data-vcp-cli-action="discard-knowledge-import"
            :disabled="
              knowledgeMutationBusy || !!pendingKnowledgeCommitOperationId
            "
            @click="store.discardKnowledgeImport"
          >
            {{
              knowledgeMutationBusy
                ? "处理中"
                : pendingKnowledgeDiscardOperationId
                  ? "重试放弃"
                  : "放弃"
            }}
          </button>
        </div>
      </div>
    </div>

    <div
      v-if="knowledgeMutationError || knowledgeMutationNotice"
      class="shrink-0 border-b border-black/10 px-3 py-2 font-mono text-[9px] leading-4 dark:border-white/10"
      :class="knowledgeMutationError ? 'text-red-500' : 'opacity-60'"
      role="status"
    >
      <template v-if="knowledgeMutationError">
        {{ knowledgeMutationError.code }} · {{ knowledgeMutationError.message }}
      </template>
      <template v-else>{{ knowledgeMutationNotice }}</template>
    </div>

    <div
      v-if="knowledgeError"
      class="flex shrink-0 items-center gap-2 border-b border-red-500/20 px-3 py-2 text-[9px] text-red-500"
      role="alert"
    >
      <span class="min-w-0 flex-1 break-words font-mono">
        {{ knowledgeError.code }} · {{ knowledgeError.message }}
      </span>
      <button
        type="button"
        class="min-h-8 shrink-0 rounded-lg border border-red-500/25 px-2 font-bold disabled:opacity-35"
        :disabled="knowledgeLoading || knowledgeMutationBusy"
        @click="store.loadKnowledgeCatalog"
      >
        重试
      </button>
    </div>

    <div
      v-if="knowledgeLoading && !knowledgeCatalog"
      class="flex min-h-0 flex-1 items-center justify-center text-[10px] opacity-45"
      role="status"
    >
      正在读取本机知识 catalog…
    </div>
    <div
      v-else-if="!knowledgeCatalog || knowledgeSources.length === 0"
      class="flex min-h-0 flex-1 items-center justify-center px-6 text-center text-[10px] leading-5 opacity-40"
    >
      尚未授权本机知识。文件选择与读取均由原生 owner 完成，WebView
      不接触设备路径。
    </div>
    <div
      v-else
      class="min-h-0 flex-1 overflow-y-auto divide-y divide-black/10 no-rubber-band dark:divide-white/10"
      data-vcp-cli-role="knowledge-list"
    >
      <article
        v-for="source in knowledgeSources"
        :key="source.source_id"
        class="flex min-h-16 items-center gap-3 border-l-2 px-3 py-2"
        :class="{
          'border-emerald-500': source.index_status === 'ready',
          'border-amber-500': source.index_status === 'indexing',
          'border-red-500': source.index_status === 'failed',
        }"
        data-vcp-cli-role="knowledge-row"
      >
        <div class="min-w-0 flex-1">
          <div class="flex min-w-0 items-center gap-2">
            <h3 class="min-w-0 flex-1 truncate text-[11px] font-semibold">
              {{ source.display_name }}
            </h3>
            <span class="shrink-0 font-mono text-[8px] opacity-55">
              {{ statusLabel(source.index_status) }}
            </span>
          </div>
          <p class="mt-1 truncate font-mono text-[8px] opacity-45">
            {{ source.mime_type }} · {{ formatBytes(source.size_bytes) }} ·
            {{ source.chunk_count }} CHUNKS
          </p>
          <p class="mt-0.5 truncate font-mono text-[8px] opacity-30">
            {{ source.source_id }} · SHA256 {{ source.source_sha256 }}
          </p>
          <p
            v-if="source.index_text_truncated"
            class="mt-1 text-[8px] text-amber-600 dark:text-amber-400"
          >
            INDEX TEXT TRUNCATED
          </p>
          <p
            v-if="source.failure_code"
            class="mt-1 font-mono text-[8px] text-red-500"
          >
            {{ source.failure_code }}
          </p>
        </div>
        <button
          type="button"
          class="inline-flex min-h-9 shrink-0 items-center gap-1 rounded-lg border border-red-500/20 px-2 text-[9px] font-bold text-red-500 disabled:opacity-35"
          data-vcp-cli-action="revoke-knowledge"
          :aria-label="`撤销 ${source.display_name} 的知识授权`"
          :disabled="revokeDisabled(source)"
          @click="confirmRevoke(source)"
        >
          <ShieldMinus :size="13" />{{
            knowledgeMutationBusy &&
            pendingKnowledgeRevoke?.sourceId === source.source_id
              ? "撤权中"
              : pendingKnowledgeRevoke?.sourceId === source.source_id
                ? "重试撤权"
                : "撤权"
          }}
        </button>
      </article>
    </div>

    <footer
      class="shrink-0 border-t border-black/10 bg-[var(--primary-bg)] px-3 pb-[calc(var(--vcp-safe-bottom,48px)+8px)] pt-2 text-[9px] leading-4 opacity-45 dark:border-white/10"
    >
      首期仅接收实际 UTF-8 文本、Markdown 与常见代码/配置，单文件最多 32
      MiB；PDF 与 Office 文档暂不开放。
    </footer>
  </section>
</template>
