import { invoke } from "@tauri-apps/api/core";
import { defineStore } from "pinia";
import { computed, ref } from "vue";

export const VCP_CLI_STATUS_COMMAND = "get_vcp_mobile_cli_status";
export const VCP_CLI_ACTION_COMMAND = "execute_vcp_mobile_cli_action";
export const VCP_CLI_SKILL_CATALOG_COMMAND = "get_vcp_mobile_cli_skill_catalog";
export const VCP_CLI_SKILL_IMPORT_INSPECT_COMMAND =
  "inspect_vcp_mobile_cli_skill_import";
export const VCP_CLI_SKILL_IMPORT_COMMIT_COMMAND =
  "commit_vcp_mobile_cli_skill_import";
export const VCP_CLI_SKILL_IMPORT_DISCARD_COMMAND =
  "discard_vcp_mobile_cli_skill_import";
export const VCP_CLI_NATIVE_PICK_FILE_COMMAND = "plugin:vcp-mobile|pick_file";
export const VCP_CLI_WORKSPACE = "/workspace";
export const VCP_CLI_SHELL = "/bin/bash";
export const VCP_CLI_DEFAULT_TIMEOUT_MS = 30 * 60 * 1_000;
export const VCP_CLI_POLL_WAIT_MS = 8_000;
export const VCP_CLI_POLL_RETRY_DELAYS_MS = [500, 1_500, 4_000] as const;
export const VCP_CLI_READ_BYTES = 65_536;
export const VCP_CLI_TAIL_BYTES = 262_144;

export type VcpCliRuntimePhase =
  | "unavailable"
  | "unprovisioned"
  | "preparing"
  | "ready"
  | "error";

export type VcpCliJobState =
  | "queued"
  | "starting"
  | "running"
  | "completed"
  | "failed"
  | "timed_out"
  | "cancelled"
  | "interrupted"
  | "waiting_user";

export interface VcpCliJobSummary {
  id: string;
  attempt_id: string;
  state: VcpCliJobState;
  command_preview: string;
  description: string | null;
  created_at_ms: number;
  updated_at_ms: number;
}

export interface VcpCliRuntimeStatus {
  available: boolean;
  availability_reason: string | null;
  background_reliability: "foreground_only";
  runtime_generation: number;
  phase: VcpCliRuntimePhase;
  profile_id: string;
  max_concurrent_jobs: number;
  running_jobs: number;
  jobs: VcpCliJobSummary[];
}

export interface VcpCliArtifactRef {
  id: string;
  sha256: string;
  size_bytes: number;
  mime_type?: string | null;
}

export interface VcpCliJobResult {
  id: string;
  state: VcpCliJobState;
  stdout: string;
  stderr: string;
  exit_code: number | null;
  cursor: string | null;
  truncated: boolean;
  artifact: VcpCliArtifactRef | null;
  reason?: string | null;
}

export interface VcpCliSkillSummary {
  id: string;
  name: string;
  version?: string | null;
  source: string;
  sha256: string;
}

export interface VcpCliSkillResult {
  id: string;
  name: string;
  resource_path: string;
  skill_root: string;
  sha256: string;
  truncated: boolean;
  materialized_path?: string | null;
}

export interface VcpCliSkillCatalogItem {
  id: string;
  name: string;
  description: string;
  version?: string | null;
  source: string;
  tree_sha256: string;
  resource_count: number;
  total_bytes: number;
  integrity_status: "valid" | "invalid";
}

export interface VcpCliSkillCatalogSnapshot {
  schema_version: number;
  generation: number;
  skills: VcpCliSkillCatalogItem[];
  warnings: string[];
}

export interface VcpCliSkillImportCandidate {
  token: string;
  candidate_sha256: string;
  catalog_generation: number;
  skill_id: string;
  name: string;
  description: string;
  version?: string | null;
  source_name: string;
  resource_count: number;
  total_bytes: number;
  tree_sha256: string;
  replaces_existing: boolean;
  warnings: string[];
}

interface NativePickedFile {
  path: string;
  name: string;
  mime: string;
  size: number;
  hash: string;
}

export interface VcpCliContentPart {
  type: "text";
  text: string;
}

export interface VcpCliResultBody {
  content: VcpCliContentPart[];
  job?: VcpCliJobResult | null;
  jobs?: VcpCliJobSummary[] | null;
  skill?: VcpCliSkillResult | null;
  skills?: VcpCliSkillSummary[] | null;
}

export type VcpCliResultEnvelope =
  | { status: "success"; result: VcpCliResultBody }
  | {
      status: "error";
      error: string;
      code: string;
      result: VcpCliResultBody;
    };

export type VcpCliAction =
  | {
      action: "run";
      command: string;
      cwd: typeof VCP_CLI_WORKSPACE;
      timeout_ms: number;
      run_in_background: false;
    }
  | { action: "list" }
  | {
      action: "poll";
      job_id: string;
      cursor?: string;
      max_output_bytes: number;
      wait_ms: number;
    }
  | { action: "cancel"; job_id: string }
  | { action: "list_skills" }
  | {
      action: "read_skill";
      skill_id: string;
      resource_path: "SKILL.md";
      max_bytes: number;
    }
  | { action: "materialize_skill"; skill_id: string };

export interface VcpCliActionRequest {
  operation_id: string;
  action: VcpCliAction;
}

export interface VcpCliActionResponse {
  operation_id: string;
  runtime_generation: number;
  envelope: VcpCliResultEnvelope;
}

export interface VcpCliUiError {
  code: string;
  message: string;
}

const TERMINAL_STATES = new Set<VcpCliJobState>([
  "completed",
  "failed",
  "timed_out",
  "cancelled",
  "interrupted",
]);

function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function newOperationId(kind: string): string {
  const suffix =
    globalThis.crypto?.randomUUID?.() ??
    `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  return `vcp-cli-${kind}-${suffix}`;
}

function isTerminal(state: VcpCliJobState): boolean {
  return TERMINAL_STATES.has(state);
}

function errorFromEnvelope(
  envelope: Extract<VcpCliResultEnvelope, { status: "error" }>,
): VcpCliUiError {
  const diagnostic = envelope.result.content
    .filter((part) => part.type === "text")
    .map((part) => part.text.trim())
    .filter(Boolean)
    .join("\n");
  return {
    code: envelope.code,
    message: diagnostic ? `${envelope.error}\n${diagnostic}` : envelope.error,
  };
}

function trimUtf8Start(value: string, maxBytes: number): string {
  const encoded = new TextEncoder().encode(value);
  if (encoded.byteLength <= maxBytes) return value;
  let start = encoded.byteLength - maxBytes;
  while (start < encoded.byteLength && (encoded[start] & 0xc0) === 0x80) {
    start += 1;
  }
  return new TextDecoder("utf-8", { fatal: false }).decode(
    encoded.slice(start),
  );
}

export function appendBoundedCliTail(
  currentStdout: string,
  currentStderr: string,
  stdoutChunk: string,
  stderrChunk: string,
): { stdout: string; stderr: string; locallyTruncated: boolean } {
  let stdout = currentStdout + stdoutChunk;
  let stderr = currentStderr + stderrChunk;
  const encoder = new TextEncoder();
  const stdoutBytes = encoder.encode(stdout).byteLength;
  const stderrBytes = encoder.encode(stderr).byteLength;
  const totalBytes = stdoutBytes + stderrBytes;
  if (totalBytes <= VCP_CLI_TAIL_BYTES) {
    return { stdout, stderr, locallyTruncated: false };
  }

  const half = Math.floor(VCP_CLI_TAIL_BYTES / 2);
  let stdoutBudget = Math.min(stdoutBytes, half);
  let stderrBudget = Math.min(stderrBytes, half);
  const unclaimed = VCP_CLI_TAIL_BYTES - stdoutBudget - stderrBudget;
  if (stdoutBytes > stdoutBudget && stderrBytes <= stderrBudget) {
    stdoutBudget += unclaimed;
  } else if (stderrBytes > stderrBudget && stdoutBytes <= stdoutBudget) {
    stderrBudget += unclaimed;
  } else {
    const stdoutNeed = Math.max(0, stdoutBytes - stdoutBudget);
    const stdoutExtra = Math.min(stdoutNeed, unclaimed);
    stdoutBudget += stdoutExtra;
    stderrBudget += unclaimed - stdoutExtra;
  }

  stdout = trimUtf8Start(stdout, stdoutBudget);
  stderr = trimUtf8Start(stderr, stderrBudget);
  return { stdout, stderr, locallyTruncated: true };
}

export const useVcpCliStore = defineStore("vcpCli", () => {
  const viewVisible = ref(false);
  const viewGeneration = ref(0);
  const runtimeGeneration = ref<number | null>(null);
  const pollGeneration = ref(0);

  const runtimeStatus = ref<VcpCliRuntimeStatus | null>(null);
  const runtimeLoading = ref(false);
  const runtimeError = ref<VcpCliUiError | null>(null);
  const jobs = ref<VcpCliJobSummary[]>([]);
  const jobsLoading = ref(false);
  const jobsError = ref<VcpCliUiError | null>(null);

  const commandDraft = ref("");
  const runBusy = ref(false);
  const runError = ref<VcpCliUiError | null>(null);
  const pendingRunOperationId = ref<string | null>(null);
  let runGeneration = 0;

  const selectedJobId = ref<string | null>(null);
  const selectedJob = ref<VcpCliJobResult | null>(null);
  const jobDetailError = ref<VcpCliUiError | null>(null);
  const cancelBusy = ref(false);
  let pollTimer: ReturnType<typeof setTimeout> | null = null;
  let lastPollFingerprint = "";
  let pollRetryAttempt = 0;
  let generationRefreshPromise: Promise<void> | null = null;
  let generationRefreshOwner = 0;
  let statusRefreshTimer: ReturnType<typeof setTimeout> | null = null;

  const skills = ref<VcpCliSkillSummary[]>([]);
  const skillsLoading = ref(false);
  const skillsLoaded = ref(false);
  const skillsError = ref<VcpCliUiError | null>(null);
  const selectedSkillId = ref<string | null>(null);
  const selectedSkill = ref<VcpCliSkillResult | null>(null);
  const selectedSkillContent = ref("");
  const skillLoading = ref(false);
  const skillError = ref<VcpCliUiError | null>(null);
  const skillCatalog = ref<VcpCliSkillCatalogSnapshot | null>(null);
  const skillImportCandidate = ref<VcpCliSkillImportCandidate | null>(null);
  const skillImportBusy = ref(false);
  const skillImportError = ref<VcpCliUiError | null>(null);
  const pendingSkillImportOperationId = ref<string | null>(null);
  const pendingSkillCommitOperationId = ref<string | null>(null);
  let skillGeneration = 0;
  let skillCatalogGeneration = 0;
  let skillMutationGeneration = 0;

  const canRun = computed(() => {
    const status = runtimeStatus.value;
    const phaseAllowsRun =
      status?.phase === "ready" ||
      status?.phase === "unprovisioned" ||
      status?.phase === "error";
    return Boolean(
      commandDraft.value.trim() &&
      status &&
      phaseAllowsRun &&
      status.running_jobs < status.max_concurrent_jobs &&
      !runtimeLoading.value &&
      !runBusy.value,
    );
  });

  const hasInternalDetail = computed(
    () => selectedJobId.value !== null || selectedSkillId.value !== null,
  );

  function isCurrentView(generation: number): boolean {
    return viewVisible.value && viewGeneration.value === generation;
  }

  function invalidatePolling(): void {
    pollGeneration.value += 1;
    if (pollTimer !== null) {
      clearTimeout(pollTimer);
      pollTimer = null;
    }
  }

  function clearStatusRefreshTimer(): void {
    if (statusRefreshTimer === null) return;
    clearTimeout(statusRefreshTimer);
    statusRefreshTimer = null;
  }

  function schedulePreparingStatusRefresh(generation: number): void {
    if (!isCurrentView(generation) || statusRefreshTimer !== null) return;
    statusRefreshTimer = setTimeout(() => {
      statusRefreshTimer = null;
      void (async () => {
        const ready = await refreshStatus(generation);
        if (!isCurrentView(generation)) return;
        if (ready) {
          await refreshJobs(generation);
          return;
        }
        if (
          (runBusy.value || skillsLoading.value) &&
          runtimeStatus.value?.phase === "unprovisioned"
        ) {
          schedulePreparingStatusRefresh(generation);
        }
      })();
    }, 750);
  }

  function resetJobDetail(): void {
    invalidatePolling();
    selectedJobId.value = null;
    selectedJob.value = null;
    jobDetailError.value = null;
    cancelBusy.value = false;
    lastPollFingerprint = "";
    pollRetryAttempt = 0;
  }

  function resetSkillDetail(): void {
    skillGeneration += 1;
    selectedSkillId.value = null;
    selectedSkill.value = null;
    selectedSkillContent.value = "";
    skillLoading.value = false;
    skillError.value = null;
  }

  function adoptRuntimeGeneration(next: number): boolean {
    const current = runtimeGeneration.value;
    if (current !== null && next < current) return false;
    if (current === next) return true;

    runtimeGeneration.value = next;
    runtimeStatus.value = null;
    jobs.value = [];
    resetJobDetail();
    resetSkillDetail();
    skills.value = [];
    skillsLoaded.value = false;
    skillCatalog.value = null;
    skillImportCandidate.value = null;
    skillImportError.value = null;
    pendingSkillImportOperationId.value = null;
    pendingSkillCommitOperationId.value = null;
    return true;
  }

  function mergeJobSummaries(incoming: VcpCliJobSummary[]): void {
    const existing = new Map(jobs.value.map((job) => [job.id, job]));
    jobs.value = incoming.map((job) => {
      const previous = existing.get(job.id);
      if (
        previous &&
        isTerminal(previous.state) &&
        previous.state !== job.state
      ) {
        return { ...job, state: previous.state };
      }
      return job;
    });
  }

  async function invokeAction(
    operationId: string,
    action: VcpCliAction,
  ): Promise<VcpCliActionResponse> {
    const response = await invoke<VcpCliActionResponse>(
      VCP_CLI_ACTION_COMMAND,
      {
        request: {
          operation_id: operationId,
          action,
        } satisfies VcpCliActionRequest,
      },
    );
    if (response.operation_id !== operationId) {
      throw new Error("CLI operation_id 回执不匹配");
    }
    return response;
  }

  function reconcileRuntimeGeneration(next: number): boolean {
    const current = runtimeGeneration.value;
    if (!adoptRuntimeGeneration(next)) return false;
    if (current === null || current === next) return true;
    runtimeLoading.value = true;
    if (!generationRefreshPromise && viewVisible.value) {
      const generation = viewGeneration.value;
      const owner = ++generationRefreshOwner;
      const refresh = (async () => {
        const ready = await refreshStatus(generation);
        if (ready && isCurrentView(generation)) await refreshJobs(generation);
      })().finally(() => {
        if (owner === generationRefreshOwner) generationRefreshPromise = null;
      });
      generationRefreshPromise = refresh;
    }
    return false;
  }

  function acceptActionResponse(response: VcpCliActionResponse): boolean {
    return reconcileRuntimeGeneration(response.runtime_generation);
  }

  async function refreshStatus(
    generation = viewGeneration.value,
  ): Promise<boolean> {
    runtimeLoading.value = true;
    runtimeError.value = null;
    try {
      const status = await invoke<VcpCliRuntimeStatus>(VCP_CLI_STATUS_COMMAND);
      if (!isCurrentView(generation)) return false;
      if (!adoptRuntimeGeneration(status.runtime_generation)) return false;
      runtimeStatus.value = status;
      mergeJobSummaries(status.jobs);
      if (status.phase === "preparing") {
        schedulePreparingStatusRefresh(generation);
      } else {
        clearStatusRefreshTimer();
      }
      return status.available && status.phase === "ready";
    } catch (error) {
      if (!isCurrentView(generation)) return false;
      runtimeError.value = { code: "ipc_error", message: describeError(error) };
      return false;
    } finally {
      if (isCurrentView(generation)) runtimeLoading.value = false;
    }
  }

  async function refreshJobs(generation = viewGeneration.value): Promise<void> {
    if (!isCurrentView(generation)) return;
    jobsLoading.value = true;
    jobsError.value = null;
    const operationId = newOperationId("list");
    try {
      const response = await invokeAction(operationId, { action: "list" });
      if (!isCurrentView(generation) || !acceptActionResponse(response)) return;
      if (response.envelope.status === "error") {
        jobsError.value = errorFromEnvelope(response.envelope);
        return;
      }
      mergeJobSummaries(response.envelope.result.jobs ?? []);
    } catch (error) {
      if (!isCurrentView(generation)) return;
      jobsError.value = { code: "ipc_error", message: describeError(error) };
    } finally {
      if (isCurrentView(generation)) jobsLoading.value = false;
    }
  }

  async function openView(): Promise<void> {
    clearStatusRefreshTimer();
    const generation = ++viewGeneration.value;
    viewVisible.value = true;
    runtimeError.value = null;
    jobsError.value = null;
    await refreshView(generation);
  }

  async function refreshView(generation = viewGeneration.value): Promise<void> {
    if (!isCurrentView(generation)) return;
    const ready = await refreshStatus(generation);
    if (ready && isCurrentView(generation)) await refreshJobs(generation);
  }

  function closeView(): void {
    viewVisible.value = false;
    viewGeneration.value += 1;
    runGeneration += 1;
    generationRefreshOwner += 1;
    generationRefreshPromise = null;
    clearStatusRefreshTimer();
    resetJobDetail();
    resetSkillDetail();
    runtimeLoading.value = false;
    jobsLoading.value = false;
    skillsLoading.value = false;
    runBusy.value = false;
  }

  function setCommandDraft(value: string): void {
    if (value === commandDraft.value) return;
    commandDraft.value = value;
    pendingRunOperationId.value = null;
    runError.value = null;
  }

  function jobResultFingerprint(job: VcpCliJobResult): string {
    return [
      job.id,
      job.cursor ?? "",
      job.state,
      job.stdout,
      job.stderr,
      job.exit_code ?? "",
    ].join("\u0000");
  }

  function applySelectedJobResult(job: VcpCliJobResult): boolean {
    if (job.id !== selectedJobId.value) return false;
    const current = selectedJob.value;
    if (current && isTerminal(current.state) && current.state !== job.state) {
      return false;
    }

    const fingerprint = jobResultFingerprint(job);
    const duplicate = Boolean(
      (current?.cursor && job.cursor === current.cursor) ||
      fingerprint === lastPollFingerprint,
    );
    const tails = appendBoundedCliTail(
      current?.stdout ?? "",
      current?.stderr ?? "",
      duplicate ? "" : job.stdout,
      duplicate ? "" : job.stderr,
    );
    lastPollFingerprint = fingerprint;
    selectedJob.value = {
      ...job,
      state: current && isTerminal(current.state) ? current.state : job.state,
      stdout: tails.stdout,
      stderr: tails.stderr,
      truncated:
        Boolean(current?.truncated) || job.truncated || tails.locallyTruncated,
      artifact: job.artifact ?? current?.artifact ?? null,
      exit_code: job.exit_code ?? current?.exit_code ?? null,
      reason: job.reason ?? current?.reason ?? null,
    };
    return true;
  }

  function scheduleNextPoll(generation: number): void {
    if (
      generation !== pollGeneration.value ||
      !viewVisible.value ||
      !selectedJob.value ||
      isTerminal(selectedJob.value.state)
    ) {
      return;
    }
    pollTimer = setTimeout(() => {
      pollTimer = null;
      void pollSelectedJob(generation);
    }, 100);
  }

  function schedulePollRetry(generation: number): void {
    if (
      generation !== pollGeneration.value ||
      !viewVisible.value ||
      !selectedJob.value ||
      isTerminal(selectedJob.value.state) ||
      pollTimer !== null
    ) {
      return;
    }
    const retryIndex = Math.min(
      pollRetryAttempt,
      VCP_CLI_POLL_RETRY_DELAYS_MS.length - 1,
    );
    const delay = VCP_CLI_POLL_RETRY_DELAYS_MS[retryIndex];
    pollRetryAttempt += 1;
    pollTimer = setTimeout(() => {
      pollTimer = null;
      void pollSelectedJob(generation);
    }, delay);
  }

  async function pollSelectedJob(
    generation = pollGeneration.value,
  ): Promise<void> {
    const jobId = selectedJobId.value;
    if (!jobId || !viewVisible.value || generation !== pollGeneration.value) {
      return;
    }
    const operationId = newOperationId("poll");
    const cursor = selectedJob.value?.cursor ?? undefined;
    try {
      const response = await invokeAction(operationId, {
        action: "poll",
        job_id: jobId,
        ...(cursor ? { cursor } : {}),
        max_output_bytes: VCP_CLI_READ_BYTES,
        wait_ms: VCP_CLI_POLL_WAIT_MS,
      });
      if (
        generation !== pollGeneration.value ||
        selectedJobId.value !== jobId ||
        !viewVisible.value ||
        !acceptActionResponse(response)
      ) {
        return;
      }
      if (response.envelope.status === "error") {
        jobDetailError.value = errorFromEnvelope(response.envelope);
        return;
      }
      const job = response.envelope.result.job;
      if (!job || !applySelectedJobResult(job)) return;
      jobDetailError.value = null;
      pollRetryAttempt = 0;
      scheduleNextPoll(generation);
    } catch (error) {
      if (
        generation !== pollGeneration.value ||
        selectedJobId.value !== jobId ||
        !viewVisible.value
      ) {
        return;
      }
      jobDetailError.value = {
        code: "ipc_error",
        message: `${describeError(error)}\n将在前台自动重试输出读取。`,
      };
      schedulePollRetry(generation);
    }
  }

  function openJob(
    job: VcpCliJobSummary,
    initialResult?: VcpCliJobResult,
  ): void {
    resetSkillDetail();
    invalidatePolling();
    selectedJobId.value = job.id;
    selectedJob.value = {
      id: job.id,
      state: job.state,
      stdout: "",
      stderr: "",
      exit_code: null,
      cursor: null,
      truncated: false,
      artifact: null,
      reason: null,
    };
    jobDetailError.value = null;
    lastPollFingerprint = "";
    pollRetryAttempt = 0;
    if (initialResult) applySelectedJobResult(initialResult);
    if (initialResult && isTerminal(initialResult.state)) return;
    const generation = pollGeneration.value;
    void pollSelectedJob(generation);
  }

  function closeJob(): void {
    resetJobDetail();
  }

  async function runDraft(): Promise<void> {
    if (!canRun.value) return;
    const command = commandDraft.value;
    const view = viewGeneration.value;
    const run = ++runGeneration;
    const operationId = pendingRunOperationId.value ?? newOperationId("run");
    pendingRunOperationId.value = operationId;
    runBusy.value = true;
    runError.value = null;
    if (runtimeStatus.value?.phase !== "ready") {
      schedulePreparingStatusRefresh(view);
    }
    try {
      const response = await invokeAction(operationId, {
        action: "run",
        command,
        cwd: VCP_CLI_WORKSPACE,
        timeout_ms: VCP_CLI_DEFAULT_TIMEOUT_MS,
        run_in_background: false,
      });
      if (
        !isCurrentView(view) ||
        run !== runGeneration ||
        commandDraft.value !== command ||
        pendingRunOperationId.value !== operationId ||
        !acceptActionResponse(response)
      ) {
        return;
      }
      if (response.envelope.status === "error") {
        pendingRunOperationId.value = null;
        runError.value = errorFromEnvelope(response.envelope);
        void refreshStatus(view);
        return;
      }
      const job = response.envelope.result.job;
      if (!job) {
        runError.value = {
          code: "invalid_response",
          message: "CLI run 未返回 Job 快照",
        };
        return;
      }
      pendingRunOperationId.value = null;
      commandDraft.value = "";
      const existing = jobs.value.find((item) => item.id === job.id);
      openJob(
        existing ?? {
          id: job.id,
          attempt_id: "",
          state: job.state,
          command_preview: "",
          description: null,
          created_at_ms: Date.now(),
          updated_at_ms: Date.now(),
        },
        job,
      );
      void refreshStatus(view);
      void refreshJobs(view);
    } catch (error) {
      if (
        !isCurrentView(view) ||
        run !== runGeneration ||
        commandDraft.value !== command ||
        pendingRunOperationId.value !== operationId
      ) {
        return;
      }
      // Keep the same operation id for retry after an ambiguous IPC failure.
      runError.value = { code: "ipc_error", message: describeError(error) };
      void refreshStatus(view);
    } finally {
      if (isCurrentView(view) && run === runGeneration) runBusy.value = false;
    }
  }

  async function cancelSelectedJob(): Promise<void> {
    const jobId = selectedJobId.value;
    const current = selectedJob.value;
    if (!jobId || !current || isTerminal(current.state) || cancelBusy.value)
      return;
    const view = viewGeneration.value;
    const poll = pollGeneration.value;
    const operationId = newOperationId("cancel");
    cancelBusy.value = true;
    jobDetailError.value = null;
    try {
      const response = await invokeAction(operationId, {
        action: "cancel",
        job_id: jobId,
      });
      if (
        !isCurrentView(view) ||
        poll !== pollGeneration.value ||
        selectedJobId.value !== jobId ||
        !acceptActionResponse(response)
      ) {
        return;
      }
      if (response.envelope.status === "error") {
        jobDetailError.value = errorFromEnvelope(response.envelope);
        return;
      }
      const job = response.envelope.result.job;
      if (job) applySelectedJobResult(job);
      void refreshJobs(view);
    } catch (error) {
      if (
        isCurrentView(view) &&
        poll === pollGeneration.value &&
        selectedJobId.value === jobId
      ) {
        jobDetailError.value = {
          code: "ipc_error",
          message: describeError(error),
        };
      }
    } finally {
      if (
        isCurrentView(view) &&
        poll === pollGeneration.value &&
        selectedJobId.value === jobId
      ) {
        cancelBusy.value = false;
      }
    }
  }

  async function loadSkills(force = false): Promise<void> {
    if ((skillsLoaded.value && !force) || skillsLoading.value) return;
    const view = viewGeneration.value;
    skillsLoading.value = true;
    skillsError.value = null;
    if (runtimeStatus.value?.phase !== "ready") {
      schedulePreparingStatusRefresh(view);
    }
    const operationId = newOperationId("list-skills");
    const catalogOwner = ++skillCatalogGeneration;
    try {
      const response = await invokeAction(operationId, {
        action: "list_skills",
      });
      if (!isCurrentView(view) || !acceptActionResponse(response)) return;
      if (response.envelope.status === "error") {
        skillsError.value = errorFromEnvelope(response.envelope);
        return;
      }
      skills.value = response.envelope.result.skills ?? [];
      skillsLoaded.value = true;
      try {
        const catalog = await invoke<VcpCliSkillCatalogSnapshot>(
          VCP_CLI_SKILL_CATALOG_COMMAND,
        );
        if (isCurrentView(view) && catalogOwner === skillCatalogGeneration) {
          skillCatalog.value = catalog;
        }
      } catch (error) {
        if (isCurrentView(view) && catalogOwner === skillCatalogGeneration) {
          skillsError.value = {
            code: "catalog_error",
            message: `Skill 索引可用，但 catalog 诊断加载失败：${describeError(error)}`,
          };
        }
      }
    } catch (error) {
      if (!isCurrentView(view)) return;
      skillsError.value = { code: "ipc_error", message: describeError(error) };
    } finally {
      if (isCurrentView(view)) {
        skillsLoading.value = false;
        void refreshStatus(view);
      }
    }
  }

  async function inspectSkillImport(): Promise<void> {
    if (skillImportBusy.value) return;
    const mutation = ++skillMutationGeneration;
    const view = viewGeneration.value;
    const operationId =
      pendingSkillImportOperationId.value ?? newOperationId("skill-inspect");
    pendingSkillImportOperationId.value = operationId;
    skillImportBusy.value = true;
    skillImportError.value = null;
    try {
      const picked = await invoke<NativePickedFile | null>(
        VCP_CLI_NATIVE_PICK_FILE_COMMAND,
        { mode: "file" },
      );
      if (!isCurrentView(view) || !picked?.path) {
        if (isCurrentView(view)) pendingSkillImportOperationId.value = null;
        return;
      }
      const candidate = await invoke<VcpCliSkillImportCandidate>(
        VCP_CLI_SKILL_IMPORT_INSPECT_COMMAND,
        {
          request: {
            operation_id: operationId,
            picked: {
              path: picked.path,
              name: picked.name,
              mime: picked.mime,
              size: picked.size,
              hash: picked.hash,
            },
          },
        },
      );
      if (
        !isCurrentView(view) ||
        pendingSkillImportOperationId.value !== operationId
      )
        return;
      skillImportCandidate.value = candidate;
      pendingSkillImportOperationId.value = null;
      pendingSkillCommitOperationId.value = null;
    } catch (error) {
      if (
        !isCurrentView(view) ||
        pendingSkillImportOperationId.value !== operationId
      )
        return;
      skillImportError.value = {
        code: "skill_import_error",
        message: describeError(error),
      };
      // Preserve the operation id so an ambiguous inspect IPC can replay its owned candidate.
    } finally {
      if (mutation === skillMutationGeneration) skillImportBusy.value = false;
    }
  }

  async function commitSkillImport(): Promise<void> {
    const candidate = skillImportCandidate.value;
    if (!candidate || skillImportBusy.value) return;
    const mutation = ++skillMutationGeneration;
    const view = viewGeneration.value;
    const operationId =
      pendingSkillCommitOperationId.value ?? newOperationId("skill-commit");
    pendingSkillCommitOperationId.value = operationId;
    skillImportBusy.value = true;
    skillImportError.value = null;
    try {
      await invoke(VCP_CLI_SKILL_IMPORT_COMMIT_COMMAND, {
        request: {
          operation_id: operationId,
          token: candidate.token,
          candidate_sha256: candidate.candidate_sha256,
          expected_catalog_generation: candidate.catalog_generation,
        },
      });
      if (
        !isCurrentView(view) ||
        skillImportCandidate.value?.token !== candidate.token
      )
        return;
      skillImportCandidate.value = null;
      pendingSkillCommitOperationId.value = null;
      skillsLoaded.value = false;
      await loadSkills(true);
    } catch (error) {
      if (
        isCurrentView(view) &&
        skillImportCandidate.value?.token === candidate.token
      ) {
        skillImportError.value = {
          code: "skill_import_error",
          message: describeError(error),
        };
      }
    } finally {
      if (mutation === skillMutationGeneration) skillImportBusy.value = false;
    }
  }

  async function discardSkillImport(): Promise<void> {
    const candidate = skillImportCandidate.value;
    if (!candidate || skillImportBusy.value) return;
    const mutation = ++skillMutationGeneration;
    const view = viewGeneration.value;
    skillImportBusy.value = true;
    skillImportError.value = null;
    try {
      await invoke(VCP_CLI_SKILL_IMPORT_DISCARD_COMMAND, {
        request: { token: candidate.token },
      });
      if (
        isCurrentView(view) &&
        skillImportCandidate.value?.token === candidate.token
      ) {
        skillImportCandidate.value = null;
        pendingSkillCommitOperationId.value = null;
      }
    } catch (error) {
      if (isCurrentView(view)) {
        skillImportError.value = {
          code: "skill_import_error",
          message: describeError(error),
        };
      }
    } finally {
      if (mutation === skillMutationGeneration) skillImportBusy.value = false;
    }
  }

  async function materializeSelectedSkill(): Promise<void> {
    const skillId = selectedSkillId.value;
    if (!skillId || skillLoading.value) return;
    const view = viewGeneration.value;
    const generation = skillGeneration;
    skillLoading.value = true;
    skillError.value = null;
    try {
      const response = await invokeAction(newOperationId("materialize-skill"), {
        action: "materialize_skill",
        skill_id: skillId,
      });
      if (
        !isCurrentView(view) ||
        generation !== skillGeneration ||
        selectedSkillId.value !== skillId ||
        !acceptActionResponse(response)
      )
        return;
      if (response.envelope.status === "error") {
        skillError.value = errorFromEnvelope(response.envelope);
        return;
      }
      const metadata = response.envelope.result.skill;
      if (!metadata?.materialized_path) {
        skillError.value = {
          code: "invalid_response",
          message: "materialize_skill 未返回工作区路径",
        };
        return;
      }
      selectedSkill.value = metadata;
    } catch (error) {
      if (isCurrentView(view) && generation === skillGeneration) {
        skillError.value = { code: "ipc_error", message: describeError(error) };
      }
    } finally {
      if (isCurrentView(view) && generation === skillGeneration)
        skillLoading.value = false;
    }
  }

  async function openSkill(skill: VcpCliSkillSummary): Promise<void> {
    resetJobDetail();
    const generation = ++skillGeneration;
    const view = viewGeneration.value;
    selectedSkillId.value = skill.id;
    selectedSkill.value = null;
    selectedSkillContent.value = "";
    skillLoading.value = true;
    skillError.value = null;
    const operationId = newOperationId("read-skill");
    try {
      const response = await invokeAction(operationId, {
        action: "read_skill",
        skill_id: skill.id,
        resource_path: "SKILL.md",
        max_bytes: VCP_CLI_READ_BYTES,
      });
      if (
        !isCurrentView(view) ||
        generation !== skillGeneration ||
        selectedSkillId.value !== skill.id ||
        !acceptActionResponse(response)
      ) {
        return;
      }
      if (response.envelope.status === "error") {
        skillError.value = errorFromEnvelope(response.envelope);
        return;
      }
      const metadata = response.envelope.result.skill;
      if (!metadata) {
        skillError.value = {
          code: "invalid_response",
          message: "read_skill 未返回 Skill 元数据",
        };
        return;
      }
      selectedSkill.value = metadata;
      selectedSkillContent.value = response.envelope.result.content
        .filter((part) => part.type === "text")
        .map((part) => part.text)
        .join("");
    } catch (error) {
      if (
        isCurrentView(view) &&
        generation === skillGeneration &&
        selectedSkillId.value === skill.id
      ) {
        skillError.value = {
          code: "ipc_error",
          message: describeError(error),
        };
      }
    } finally {
      if (
        isCurrentView(view) &&
        generation === skillGeneration &&
        selectedSkillId.value === skill.id
      ) {
        skillLoading.value = false;
      }
    }
  }

  function closeSkill(): void {
    resetSkillDetail();
  }

  function closeInternalDetail(): void {
    if (selectedJobId.value) closeJob();
    else if (selectedSkillId.value) closeSkill();
  }

  return {
    viewVisible,
    viewGeneration,
    runtimeGeneration,
    pollGeneration,
    runtimeStatus,
    runtimeLoading,
    runtimeError,
    jobs,
    jobsLoading,
    jobsError,
    commandDraft,
    runBusy,
    runError,
    pendingRunOperationId,
    selectedJobId,
    selectedJob,
    jobDetailError,
    cancelBusy,
    skills,
    skillsLoading,
    skillsLoaded,
    skillsError,
    selectedSkillId,
    selectedSkill,
    selectedSkillContent,
    skillLoading,
    skillError,
    skillCatalog,
    skillImportCandidate,
    skillImportBusy,
    skillImportError,
    canRun,
    hasInternalDetail,
    openView,
    refreshView,
    closeView,
    refreshStatus,
    refreshJobs,
    setCommandDraft,
    runDraft,
    openJob,
    closeJob,
    pollSelectedJob,
    cancelSelectedJob,
    loadSkills,
    openSkill,
    closeSkill,
    inspectSkillImport,
    commitSkillImport,
    discardSkillImport,
    materializeSelectedSkill,
    closeInternalDetail,
  };
});
