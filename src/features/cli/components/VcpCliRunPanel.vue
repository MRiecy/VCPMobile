<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import { computed, onMounted, ref, watch } from "vue";
import { storeToRefs } from "pinia";
import {
  Ban,
  ChevronRight,
  CircleStop,
  RefreshCw,
  Send,
} from "lucide-vue-next";
import RefreshButton from "../../../components/ui/RefreshButton.vue";
import {
  VCP_CLI_SHELL,
  VCP_CLI_WORKSPACE,
  type VcpCliJobState,
  useVcpCliStore,
} from "../vcpCliStore";

defineProps<{ keyboardHeight: number }>();

const store = useVcpCliStore();
const {
  runtimeStatus,
  runtimeLoading,
  runtimeError,
  jobs,
  jobsLoading,
  jobsError,
  commandDraft,
  runBusy,
  runError,
  canRun,
  selectedJobId,
  selectedJob,
  jobDetailError,
  cancelBusy,
} = storeToRefs(store);

const outputChannel = ref<"stdout" | "stderr">("stdout");
const notificationPermission = ref<boolean | null>(null);
const notificationPermissionBusy = ref(false);

const terminalStates = new Set<VcpCliJobState>([
  "completed",
  "failed",
  "timed_out",
  "cancelled",
  "interrupted",
]);

const selectedSummary = computed(
  () => jobs.value.find((job) => job.id === selectedJobId.value) ?? null,
);
const selectedOutput = computed(() =>
  outputChannel.value === "stdout"
    ? (selectedJob.value?.stdout ?? "")
    : (selectedJob.value?.stderr ?? ""),
);
const selectedCanCancel = computed(
  () =>
    Boolean(selectedJob.value) && !terminalStates.has(selectedJob.value!.state),
);
const concurrencyFull = computed(() => {
  const status = runtimeStatus.value;
  return Boolean(status && status.running_jobs >= status.max_concurrent_jobs);
});
const runtimeStateLabel = computed(() => {
  const status = runtimeStatus.value;
  if (!status) {
    return runtimeLoading.value
      ? "正在准备本地 Runtime"
      : "尚未读取本地 Runtime";
  }
  if (
    runBusy.value &&
    (status.phase === "unprovisioned" || status.phase === "error")
  ) {
    return "正在准备本地 Runtime";
  }
  const labels = {
    unavailable: "本地 Runtime 不可用",
    unprovisioned: "本地 Runtime 等待首次准备",
    preparing: "正在准备本地 Runtime",
    ready: "本地 Runtime 可用",
    error: "本地 Runtime 准备失败",
  } as const;
  return labels[status.phase];
});
const runtimeStateTone = computed(() => {
  const phase = runtimeStatus.value?.phase;
  if (phase === "ready") return "bg-blue-500";
  if (phase === "unavailable" || phase === "error") return "bg-red-500";
  return "bg-amber-500";
});
const blockedReason = computed(() => {
  if (runtimeError.value) return runtimeError.value.message;
  const status = runtimeStatus.value;
  if (!status)
    return runtimeLoading.value
      ? "正在校验并解包内置 PRoot / Alpine 资源，请保持本页在前台。"
      : "点击刷新以读取 Rust Runtime 状态。";
  if (
    runBusy.value &&
    (status.phase === "unprovisioned" || status.phase === "error")
  ) {
    const previousReason = status.availability_reason
      ? ` 上次状态：${status.availability_reason}`
      : "";
    return `正在校验并解包内置 PRoot / Alpine 资源。${previousReason}`;
  }
  if (status.phase === "unavailable") {
    return status.availability_reason || "此设备不支持本地 CLI Runtime。";
  }
  if (status.phase === "unprovisioned") {
    return (
      status.availability_reason ||
      "输入命令后点击“准备并执行”；首次运行会校验并解包内置 Runtime。"
    );
  }
  if (status.phase === "preparing") {
    return (
      status.availability_reason ||
      "正在校验并解包内置 PRoot / Alpine 资源，请保持本页在前台。"
    );
  }
  if (status.phase === "error") {
    return (
      status.availability_reason ||
      "准备失败。检查可用空间后，可用当前命令重试准备。"
    );
  }
  if (concurrencyFull.value) {
    return `运行中 Job 已达上限 ${status.running_jobs}/${status.max_concurrent_jobs}`;
  }
  return status.availability_reason || "";
});
const runButtonLabel = computed(() => {
  if (runBusy.value) {
    return runtimeStatus.value?.phase === "ready" ? "执行中" : "准备中";
  }
  const phase = runtimeStatus.value?.phase;
  if (!phase) return runtimeLoading.value ? "准备中" : "等待状态";
  if (phase === "unprovisioned") return "准备并执行";
  if (phase === "preparing") return "准备中";
  if (phase === "error") return "重试准备";
  if (phase === "unavailable") return "不可用";
  return "执行";
});

function stateLabel(state: VcpCliJobState): string {
  const labels: Record<VcpCliJobState, string> = {
    queued: "QUEUED",
    starting: "STARTING",
    running: "RUNNING",
    stopping: "STOPPING",
    waiting_user: "WAITING USER",
    completed: "COMPLETED",
    failed: "FAILED",
    timed_out: "TIMED OUT",
    cancelled: "CANCELLED",
    interrupted: "INTERRUPTED",
  };
  return labels[state];
}

function stateTone(state: VcpCliJobState): string {
  if (state === "failed" || state === "timed_out" || state === "interrupted") {
    return "text-red-500 border-red-500";
  }
  if (state === "waiting_user") return "text-amber-500 border-amber-500";
  if (state === "running" || state === "starting") {
    return "text-blue-500 border-blue-500";
  }
  if (state === "stopping") return "text-amber-500 border-amber-500";
  if (state === "completed") return "text-emerald-600 border-emerald-500";
  return "text-[var(--primary-text)] border-black/25 dark:border-white/25";
}

function formatTime(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "—";
  return new Intl.DateTimeFormat("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(new Date(value));
}

function updateDraft(event: Event): void {
  store.setCommandDraft((event.target as HTMLTextAreaElement).value);
}

async function refreshNotificationPermission(): Promise<void> {
  try {
    const permissions = await invoke<{ notification: boolean }>(
      "plugin:vcp-mobile|check_all_permissions",
    );
    notificationPermission.value = permissions.notification;
  } catch {
    notificationPermission.value = null;
  }
}

async function requestNotificationPermission(): Promise<void> {
  if (notificationPermissionBusy.value) return;
  notificationPermissionBusy.value = true;
  try {
    await invoke("plugin:vcp-mobile|request_android_permission", {
      pType: "notification",
    });
    await refreshNotificationPermission();
  } catch {
    notificationPermission.value = false;
  } finally {
    notificationPermissionBusy.value = false;
  }
}

watch(selectedJobId, () => {
  outputChannel.value = "stdout";
});

onMounted(() => void refreshNotificationPermission());
</script>

<template>
  <section
    v-if="selectedJobId && selectedJob"
    class="flex min-h-0 flex-1 flex-col"
    aria-label="CLI Job 详情"
    data-vcp-cli-role="job-detail"
  >
    <div
      class="shrink-0 border-b border-black/10 px-3 py-2 dark:border-white/10"
    >
      <div
        class="flex items-start gap-3 border-l-2 pl-3"
        :class="stateTone(selectedJob.state)"
      >
        <div class="min-w-0 flex-1">
          <p class="truncate text-[11px] font-bold text-[var(--primary-text)]">
            {{
              selectedSummary?.command_preview ||
              selectedSummary?.description ||
              selectedJob.id
            }}
          </p>
          <div class="mt-1 flex flex-wrap gap-x-3 gap-y-1 font-mono text-[9px]">
            <span>{{ stateLabel(selectedJob.state) }}</span>
            <span class="text-[var(--primary-text)] opacity-45">{{
              selectedJob.id
            }}</span>
            <span
              v-if="selectedJob.exit_code !== null"
              class="text-[var(--primary-text)] opacity-45"
            >
              EXIT {{ selectedJob.exit_code }}
            </span>
          </div>
        </div>
        <button
          v-if="selectedCanCancel"
          type="button"
          class="inline-flex min-h-9 shrink-0 items-center gap-1.5 rounded-lg border border-red-500/25 px-2.5 text-[10px] font-bold text-red-500 disabled:opacity-40"
          data-vcp-cli-action="cancel"
          :disabled="cancelBusy"
          @click="store.cancelSelectedJob()"
        >
          <CircleStop :size="14" />
          {{ cancelBusy ? "正在确认取消" : "取消 Job" }}
        </button>
      </div>
      <p
        v-if="selectedJob.reason"
        class="mt-2 whitespace-pre-wrap text-[10px] leading-5 opacity-60"
      >
        {{ selectedJob.reason }}
      </p>
      <p
        v-if="jobDetailError"
        class="mt-2 whitespace-pre-wrap font-mono text-[9px] leading-4 text-red-500"
        role="status"
      >
        {{ jobDetailError.code }} · {{ jobDetailError.message }}
      </p>
    </div>

    <div
      class="grid shrink-0 grid-cols-2 border-b border-black/10 dark:border-white/10"
      role="tablist"
      aria-label="Job 输出通道"
    >
      <button
        v-for="channel in ['stdout', 'stderr'] as const"
        :key="channel"
        type="button"
        role="tab"
        class="min-h-10 border-b-2 font-mono text-[10px] font-bold uppercase"
        :class="
          outputChannel === channel
            ? 'border-[var(--highlight-text)] text-[var(--highlight-text)]'
            : 'border-transparent opacity-45'
        "
        :aria-selected="outputChannel === channel"
        @click="outputChannel = channel"
      >
        {{ channel }}
      </button>
    </div>

    <div
      class="vcp-scrollable no-swipe min-h-0 flex-1 overflow-auto bg-black/[0.025] dark:bg-white/[0.025]"
    >
      <pre
        class="min-h-full whitespace-pre-wrap break-words px-3 py-3 font-mono text-[10px] leading-[1.55] select-text"
        :data-vcp-cli-role="`job-${outputChannel}`"
        >{{ selectedOutput || `(${outputChannel} 暂无输出)` }}</pre
      >
    </div>

    <footer
      class="shrink-0 border-t border-black/10 bg-[var(--primary-bg)] px-3 pb-[calc(var(--vcp-safe-bottom,48px)+8px)] pt-2 dark:border-white/10"
    >
      <div class="flex min-h-8 items-center gap-2 text-[9px]">
        <span
          v-if="selectedJob.truncated"
          class="border-l-2 border-amber-500 pl-2 font-bold text-amber-600 dark:text-amber-400"
        >
          输出已截断，仅保留有界 tail
        </span>
        <span v-else class="opacity-45"
          >输出按 cursor 增量读取，stdout / stderr 分离。</span
        >
        <span v-if="selectedJob.artifact" class="ml-auto font-mono opacity-45">
          ARTIFACT {{ selectedJob.artifact.size_bytes }} B
        </span>
      </div>
    </footer>
  </section>

  <section v-else class="flex min-h-0 flex-1 flex-col" aria-label="CLI 运行">
    <div class="min-h-0 flex-1 overflow-y-auto no-rubber-band">
      <div class="border-b border-black/10 px-3 py-3 dark:border-white/10">
        <div class="flex items-center gap-2">
          <span
            class="rounded-md bg-blue-500/10 px-1.5 py-0.5 font-mono text-[9px] font-bold text-blue-600 dark:text-blue-400"
            >LOCAL</span
          >
          <code class="font-mono text-[9px] opacity-55">{{
            VCP_CLI_SHELL
          }}</code>
          <code class="font-mono text-[9px] opacity-55">{{
            VCP_CLI_WORKSPACE
          }}</code>
          <RefreshButton
            bare
            class="ml-auto flex h-8 w-8 items-center justify-center rounded-lg opacity-45 active:opacity-100"
            label="刷新 CLI 状态"
            :size="14"
            :loading="runtimeLoading || jobsLoading"
            :disabled="runtimeLoading || jobsLoading"
            @refresh="store.refreshView()"
          />
        </div>
        <div class="mt-2 flex items-start gap-2 text-[10px] leading-5">
          <span
            class="mt-1.5 h-1.5 w-1.5 shrink-0 rounded-full"
            :class="runtimeStateTone"
          />
          <div class="min-w-0 flex-1">
            <p
              class="font-semibold"
              :data-vcp-cli-phase="runtimeStatus?.phase || 'loading'"
            >
              {{ runtimeStateLabel }}
            </p>
            <p
              v-if="blockedReason"
              class="break-words font-mono text-[9px] opacity-60"
            >
              {{ blockedReason }}
            </p>
            <p v-else class="font-mono text-[9px] opacity-45">
              {{ runtimeStatus?.profile_id }} ·
              {{ runtimeStatus?.running_jobs }}/{{
                runtimeStatus?.max_concurrent_jobs
              }}
              RUNNING
            </p>
          </div>
        </div>
        <p
          class="mt-2 border-l-2 border-amber-500/70 pl-2 text-[9px] leading-4 opacity-65"
        >
          仅承诺前台运行；切入后台或系统回收后，Job 可能被标记为 interrupted。
        </p>
        <div
          v-if="notificationPermission === false"
          class="mt-2 flex items-center gap-2 border-l-2 border-amber-500/70 pl-2 text-[9px] leading-4"
          data-vcp-cli-role="background-permission"
        >
          <span class="min-w-0 flex-1 opacity-65">通知权限未允许，后台增强将自动降级为前台 Job。</span>
          <button
            type="button"
            class="min-h-8 shrink-0 px-2 font-bold text-[var(--highlight-text)] disabled:opacity-35"
            :disabled="notificationPermissionBusy"
            @click="requestNotificationPermission"
          >
            {{ notificationPermissionBusy ? "请求中…" : "允许通知" }}
          </button>
        </div>
      </div>

      <div
        class="flex min-h-10 items-center border-b border-black/10 px-3 dark:border-white/10"
      >
        <h2 class="text-[11px] font-bold">Jobs</h2>
        <span class="ml-2 font-mono text-[9px] opacity-40">{{
          jobs.length
        }}</span>
        <span v-if="jobsLoading" class="ml-auto text-[9px] opacity-45"
          >正在同步…</span
        >
      </div>

      <p
        v-if="jobsError"
        class="border-b border-red-500/20 px-3 py-2 font-mono text-[9px] leading-4 text-red-500"
        role="status"
      >
        {{ jobsError.code }} · {{ jobsError.message }}
      </p>
      <div
        v-else-if="jobs.length === 0"
        class="flex min-h-32 items-center justify-center px-6 text-center text-[10px] leading-5 opacity-40"
      >
        暂无 Job。输入一条命令后，执行与输出状态由 Rust ledger 记录。
      </div>
      <div v-else class="divide-y divide-black/10 dark:divide-white/10">
        <button
          v-for="job in jobs"
          :key="job.id"
          type="button"
          class="flex min-h-14 w-full items-stretch text-left active:bg-black/[0.035] dark:active:bg-white/[0.035]"
          data-vcp-cli-role="job-row"
          @click="store.openJob(job)"
        >
          <span
            class="w-0.5 shrink-0 border-l-2"
            :class="stateTone(job.state)"
          />
          <span class="flex min-w-0 flex-1 items-center gap-3 px-3 py-2">
            <span class="min-w-0 flex-1">
              <span class="block truncate text-[11px] font-semibold">
                {{ job.command_preview || job.description || job.id }}
              </span>
              <span class="mt-1 flex gap-2 font-mono text-[8px] opacity-45">
                <span class="truncate">{{ job.id }}</span>
                <span class="shrink-0">{{
                  formatTime(job.updated_at_ms)
                }}</span>
              </span>
            </span>
            <span
              class="shrink-0 font-mono text-[8px] font-bold"
              :class="stateTone(job.state)"
            >
              {{ stateLabel(job.state) }}
            </span>
            <ChevronRight :size="14" class="shrink-0 opacity-25" />
          </span>
        </button>
      </div>
    </div>

    <footer
      class="shrink-0 border-t border-black/10 bg-[var(--primary-bg)] px-3 pt-2 dark:border-white/10"
      :style="{
        paddingBottom: `calc(var(--vcp-safe-bottom, 48px) + ${keyboardHeight}px + 8px)`,
      }"
    >
      <p
        v-if="runError"
        class="mb-2 whitespace-pre-wrap font-mono text-[9px] leading-4 text-red-500"
        role="status"
      >
        {{ runError.code }} · {{ runError.message }}
      </p>
      <form class="flex items-end gap-2" @submit.prevent>
        <textarea
          :value="commandDraft"
          rows="3"
          spellcheck="false"
          autocomplete="off"
          class="min-h-16 min-w-0 flex-1 resize-none rounded-xl border border-black/12 bg-[var(--secondary-bg)] px-3 py-2 font-mono text-[11px] leading-5 outline-none focus:border-[var(--highlight-text)] dark:border-white/12"
          placeholder="输入 Bash 命令；Enter 换行"
          aria-label="Bash 命令"
          data-vcp-cli-role="command-input"
          @input="updateDraft"
        />
        <button
          type="button"
          class="flex min-h-16 w-20 shrink-0 flex-col items-center justify-center gap-1 rounded-xl bg-[var(--highlight-text)] text-[10px] font-bold text-white disabled:opacity-35"
          data-vcp-cli-action="run"
          :disabled="!canRun"
          @click="store.runDraft()"
        >
          <RefreshCw
            v-if="
              runBusy || runtimeLoading || runtimeStatus?.phase === 'preparing'
            "
            :size="16"
            class="animate-spin"
          />
          <Ban
            v-else-if="!runtimeStatus || runtimeStatus.phase === 'unavailable'"
            :size="16"
          />
          <Send v-else :size="16" />
          {{ runButtonLabel }}
        </button>
      </form>
    </footer>
  </section>
</template>
