import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import {
  VCP_CLI_ACTION_COMMAND,
  VCP_CLI_DEFAULT_TIMEOUT_MS,
  VCP_CLI_POLL_WAIT_MS,
  VCP_CLI_POLL_RETRY_DELAYS_MS,
  VCP_CLI_READ_BYTES,
  VCP_CLI_STATUS_COMMAND,
  VCP_CLI_TAIL_BYTES,
  VCP_CLI_WORKSPACE,
  appendBoundedCliTail,
  type VcpCliAction,
  type VcpCliActionResponse,
  type VcpCliJobResult,
  type VcpCliJobSummary,
  type VcpCliResultBody,
  type VcpCliRuntimeStatus,
  useVcpCliStore,
} from "@/features/cli/vcpCliStore";
import { invokeMock, mockInvoke } from "@/tests/mocks/tauri";
import { flushPromises } from "@/tests/utils/flush";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function summary(
  id: string,
  state: VcpCliJobSummary["state"] = "running",
): VcpCliJobSummary {
  return {
    id,
    attempt_id: `${id}-attempt-1`,
    state,
    command_preview: `echo ${id}`,
    description: null,
    created_at_ms: 1,
    updated_at_ms: 2,
  };
}

function job(
  id: string,
  state: VcpCliJobResult["state"] = "running",
  patch: Partial<VcpCliJobResult> = {},
): VcpCliJobResult {
  return {
    id,
    state,
    stdout: "",
    stderr: "",
    exit_code: null,
    cursor: null,
    truncated: false,
    artifact: null,
    reason: null,
    ...patch,
  };
}

function status(patch: Partial<VcpCliRuntimeStatus> = {}): VcpCliRuntimeStatus {
  return {
    available: true,
    availability_reason: null,
    background_reliability: "foreground_only",
    runtime_generation: 1,
    phase: "ready",
    profile_id: "alpine-arm64-v1",
    max_concurrent_jobs: 2,
    running_jobs: 0,
    jobs: [],
    ...patch,
  };
}

function success(
  operationId: string,
  result: Partial<VcpCliResultBody> = {},
  runtimeGeneration = 1,
): VcpCliActionResponse {
  return {
    operation_id: operationId,
    runtime_generation: runtimeGeneration,
    envelope: {
      status: "success",
      result: { content: [], ...result },
    },
  };
}

function requestFromArgs(args?: Record<string, unknown>) {
  return args?.request as { operation_id: string; action: VcpCliAction };
}

describe("VCP CLI view snapshot ownership", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("keeps execution disabled when the Rust status is unavailable", async () => {
    mockInvoke(VCP_CLI_STATUS_COMMAND, () =>
      status({
        available: false,
        availability_reason: "当前设备不支持本地 Runtime",
        phase: "unavailable",
      }),
    );
    const store = useVcpCliStore();

    await store.openView();
    store.setCommandDraft("echo blocked");
    await store.runDraft();

    expect(store.canRun).toBe(false);
    expect(store.runtimeStatus?.availability_reason).toBe(
      "当前设备不支持本地 Runtime",
    );
    expect(
      invokeMock.mock.calls.some(
        ([command, args]) =>
          command === VCP_CLI_ACTION_COMMAND &&
          requestFromArgs(args).action.action === "run",
      ),
    ).toBe(false);
  });

  it("allows first-run provisioning and follows preparing through ready", async () => {
    vi.useFakeTimers();
    const statuses = [
      status({
        available: false,
        availability_reason: null,
        phase: "unprovisioned",
      }),
      status({
        available: false,
        availability_reason: null,
        phase: "preparing",
      }),
      status(),
    ];
    let statusRead = 0;
    mockInvoke(
      VCP_CLI_STATUS_COMMAND,
      () => statuses[Math.min(statusRead++, statuses.length - 1)],
    );
    const run = deferred<VcpCliActionResponse>();
    mockInvoke(VCP_CLI_ACTION_COMMAND, (args) => {
      const request = requestFromArgs(args);
      if (request.action.action === "run") return run.promise;
      if (request.action.action === "list") {
        return success(request.operation_id, { jobs: [] });
      }
      throw new Error(`unexpected action ${request.action.action}`);
    });
    const store = useVcpCliStore();
    await store.openView();
    store.setCommandDraft("printf ready");

    expect(store.canRun).toBe(true);
    const pending = store.runDraft();
    expect(store.runBusy).toBe(true);

    await vi.advanceTimersByTimeAsync(750);
    expect(store.runtimeStatus?.phase).toBe("preparing");
    expect(store.canRun).toBe(false);

    await vi.advanceTimersByTimeAsync(750);
    expect(store.runtimeStatus?.phase).toBe("ready");
    expect(statusRead).toBeGreaterThanOrEqual(3);

    const runCall = invokeMock.mock.calls.find(
      ([command, args]) =>
        command === VCP_CLI_ACTION_COMMAND &&
        requestFromArgs(args).action.action === "run",
    );
    const operationId = requestFromArgs(runCall?.[1]).operation_id;
    run.resolve(
      success(operationId, {
        job: job("job-first-run", "completed", {
          stdout: "ready",
          exit_code: 0,
          cursor: "final",
        }),
      }),
    );
    await pending;
    await flushPromises();
    expect(store.selectedJob?.stdout).toBe("ready");
    store.closeView();
  });

  it("keeps an error phase retryable through the same explicit run action", async () => {
    mockInvoke(VCP_CLI_STATUS_COMMAND, () =>
      status({
        available: false,
        availability_reason: "内置 rootfs 完整性校验失败",
        phase: "error",
      }),
    );
    const store = useVcpCliStore();
    await store.openView();
    store.setCommandDraft("echo retry");

    expect(store.canRun).toBe(true);
    expect(store.runtimeStatus?.availability_reason).toContain(
      "完整性校验失败",
    );
    store.closeView();
  });

  it("reuses one operation_id after an ambiguous run failure", async () => {
    mockInvoke(VCP_CLI_STATUS_COMMAND, () => status());
    const runOperationIds: string[] = [];
    let runAttempt = 0;
    mockInvoke(VCP_CLI_ACTION_COMMAND, (args) => {
      const request = requestFromArgs(args);
      if (request.action.action === "list") {
        return success(request.operation_id, { jobs: [] });
      }
      if (request.action.action === "run") {
        runOperationIds.push(request.operation_id);
        runAttempt += 1;
        if (runAttempt === 1) throw new Error("IPC result uncertain");
        return success(request.operation_id, {
          job: job("job-run", "completed", {
            stdout: "ok\n",
            exit_code: 0,
            cursor: "cursor-1",
          }),
        });
      }
      throw new Error(`unexpected action ${request.action.action}`);
    });
    const store = useVcpCliStore();
    await store.openView();
    store.setCommandDraft("printf 'ok\\n'");

    await store.runDraft();
    const retainedOperationId = store.pendingRunOperationId;
    expect(retainedOperationId).toBe(runOperationIds[0]);
    expect(store.runError?.code).toBe("ipc_error");

    await store.runDraft();
    expect(runOperationIds).toEqual([retainedOperationId, retainedOperationId]);
    expect(store.pendingRunOperationId).toBeNull();
    expect(store.commandDraft).toBe("");
    expect(store.selectedJob?.stdout).toBe("ok\n");

    const runAction = requestFromArgs(
      invokeMock.mock.calls.find(
        ([command, args]) =>
          command === VCP_CLI_ACTION_COMMAND &&
          requestFromArgs(args).action.action === "run",
      )?.[1],
    ).action;
    expect(runAction).toMatchObject({
      action: "run",
      cwd: VCP_CLI_WORKSPACE,
      timeout_ms: VCP_CLI_DEFAULT_TIMEOUT_MS,
      run_in_background: false,
    });
  });

  it("refreshes snapshots without abandoning an in-flight run owner", async () => {
    mockInvoke(VCP_CLI_STATUS_COMMAND, () => status());
    const run = deferred<VcpCliActionResponse>();
    mockInvoke(VCP_CLI_ACTION_COMMAND, (args) => {
      const request = requestFromArgs(args);
      if (request.action.action === "list") {
        return success(request.operation_id, { jobs: [] });
      }
      if (request.action.action === "run") return run.promise;
      throw new Error(`unexpected action ${request.action.action}`);
    });
    const store = useVcpCliStore();
    await store.openView();
    store.setCommandDraft("echo owned");
    const view = store.viewGeneration;
    const pending = store.runDraft();

    await store.refreshView();
    expect(store.viewGeneration).toBe(view);
    expect(store.runBusy).toBe(true);

    const runCall = invokeMock.mock.calls.find(
      ([command, args]) =>
        command === VCP_CLI_ACTION_COMMAND &&
        requestFromArgs(args).action.action === "run",
    );
    run.resolve(
      success(requestFromArgs(runCall?.[1]).operation_id, {
        job: job("job-owned", "completed", {
          stdout: "owned\n",
          cursor: "final",
          exit_code: 0,
        }),
      }),
    );
    await pending;
    await flushPromises();

    expect(store.selectedJob?.id).toBe("job-owned");
    expect(store.selectedJob?.stdout).toBe("owned\n");
    store.closeView();
  });

  it("rejects a late poll from Job A after selection moves to Job B", async () => {
    mockInvoke(VCP_CLI_STATUS_COMMAND, () =>
      status({ jobs: [summary("job-a"), summary("job-b")] }),
    );
    const pollA = deferred<VcpCliActionResponse>();
    const pollB = deferred<VcpCliActionResponse>();
    mockInvoke(VCP_CLI_ACTION_COMMAND, (args) => {
      const request = requestFromArgs(args);
      if (request.action.action === "list") {
        return success(request.operation_id, {
          jobs: [summary("job-a"), summary("job-b")],
        });
      }
      if (request.action.action === "poll") {
        return request.action.job_id === "job-a"
          ? pollA.promise
          : pollB.promise;
      }
      throw new Error(`unexpected action ${request.action.action}`);
    });
    const store = useVcpCliStore();
    await store.openView();

    store.openJob(store.jobs[0]);
    store.openJob(store.jobs[1]);
    // Responses must echo their caller's operation id.
    const pollCalls = invokeMock.mock.calls.filter(
      ([command, args]) =>
        command === VCP_CLI_ACTION_COMMAND &&
        requestFromArgs(args).action.action === "poll",
    );
    const pollAOperationId = requestFromArgs(pollCalls[0][1]).operation_id;
    const pollBOperationId = requestFromArgs(pollCalls[1][1]).operation_id;
    pollA.resolve(
      success(pollAOperationId, {
        job: job("job-a", "completed", { stdout: "stale A", cursor: "a-1" }),
      }),
    );
    await flushPromises();
    expect(store.selectedJobId).toBe("job-b");
    expect(store.selectedJob?.stdout).toBe("");

    pollB.resolve(
      success(pollBOperationId, {
        job: job("job-b", "completed", { stdout: "current B", cursor: "b-1" }),
      }),
    );
    await flushPromises();
    expect(store.selectedJobId).toBe("job-b");
    expect(store.selectedJob?.stdout).toBe("current B");
  });

  it("deduplicates a repeated cursor and never regresses a terminal Job", async () => {
    vi.useFakeTimers();
    mockInvoke(VCP_CLI_STATUS_COMMAND, () =>
      status({ jobs: [summary("job-1")] }),
    );
    const pollResults = [
      job("job-1", "running", { stdout: "once\n", cursor: "c-1" }),
      job("job-1", "running", { stdout: "once\n", cursor: "c-1" }),
      job("job-1", "completed", {
        stdout: "done\n",
        cursor: "c-2",
        exit_code: 0,
      }),
      job("job-1", "running", { stdout: "late\n", cursor: "c-3" }),
    ];
    mockInvoke(VCP_CLI_ACTION_COMMAND, (args) => {
      const request = requestFromArgs(args);
      if (request.action.action === "list") {
        return success(request.operation_id, { jobs: [summary("job-1")] });
      }
      if (request.action.action === "poll") {
        const next = pollResults.shift();
        if (!next) throw new Error("unexpected extra poll");
        return success(request.operation_id, { job: next });
      }
      throw new Error(`unexpected action ${request.action.action}`);
    });
    const store = useVcpCliStore();
    await store.openView();
    store.openJob(store.jobs[0]);
    await flushPromises();

    await store.pollSelectedJob(store.pollGeneration);
    expect(store.selectedJob?.stdout).toBe("once\n");

    await store.pollSelectedJob(store.pollGeneration);
    expect(store.selectedJob?.state).toBe("completed");
    expect(store.selectedJob?.stdout).toBe("once\ndone\n");

    await store.pollSelectedJob(store.pollGeneration);
    expect(store.selectedJob?.state).toBe("completed");
    expect(store.selectedJob?.stdout).toBe("once\ndone\n");
    store.closeView();
  });

  it("waits for backend cancellation confirmation before changing state", async () => {
    vi.useFakeTimers();
    mockInvoke(VCP_CLI_STATUS_COMMAND, () =>
      status({ jobs: [summary("job-cancel")] }),
    );
    const cancel = deferred<VcpCliActionResponse>();
    mockInvoke(VCP_CLI_ACTION_COMMAND, (args) => {
      const request = requestFromArgs(args);
      if (request.action.action === "list") {
        return success(request.operation_id, { jobs: [summary("job-cancel")] });
      }
      if (request.action.action === "poll") {
        return success(request.operation_id, {
          job: job("job-cancel", "running", { cursor: "c-1" }),
        });
      }
      if (request.action.action === "cancel") return cancel.promise;
      throw new Error(`unexpected action ${request.action.action}`);
    });
    const store = useVcpCliStore();
    await store.openView();
    store.openJob(store.jobs[0]);
    await flushPromises();

    const pending = store.cancelSelectedJob();
    expect(store.cancelBusy).toBe(true);
    expect(store.selectedJob?.state).toBe("running");
    const cancelCall = invokeMock.mock.calls.find(
      ([command, args]) =>
        command === VCP_CLI_ACTION_COMMAND &&
        requestFromArgs(args).action.action === "cancel",
    );
    const cancelRequest = requestFromArgs(cancelCall?.[1]);
    expect(cancelRequest.action).toEqual({
      action: "cancel",
      job_id: "job-cancel",
    });

    cancel.resolve(
      success(cancelRequest.operation_id, {
        job: job("job-cancel", "cancelled", {
          cursor: "c-1",
          reason: "user_cancelled",
        }),
      }),
    );
    await pending;
    expect(store.selectedJob?.state).toBe("cancelled");
    expect(store.cancelBusy).toBe(false);
    store.closeView();
  });

  it("reads only SKILL.md without creating or mutating Jobs", async () => {
    mockInvoke(VCP_CLI_STATUS_COMMAND, () =>
      status({ jobs: [summary("existing", "completed")] }),
    );
    const actions: VcpCliAction[] = [];
    mockInvoke(VCP_CLI_ACTION_COMMAND, (args) => {
      const request = requestFromArgs(args);
      actions.push(request.action);
      if (request.action.action === "list") {
        return success(request.operation_id, {
          jobs: [summary("existing", "completed")],
        });
      }
      if (request.action.action === "list_skills") {
        return success(request.operation_id, {
          skills: [
            {
              id: "skill-one",
              name: "Skill One",
              version: "1.0.0",
              source: "bundled",
              sha256: "abc123",
            },
          ],
        });
      }
      if (request.action.action === "read_skill") {
        return success(request.operation_id, {
          content: [{ type: "text", text: "# Safe instructions" }],
          skill: {
            id: "skill-one",
            name: "Skill One",
            resource_path: "SKILL.md",
            skill_root: "vcp-skill://skill-one",
            sha256: "abc123",
            truncated: false,
          },
        });
      }
      throw new Error(`unexpected action ${request.action.action}`);
    });
    const store = useVcpCliStore();
    await store.openView();
    const jobsBefore = store.jobs.map((item) => item.id);

    await store.loadSkills();
    await store.openSkill(store.skills[0]);

    expect(store.selectedSkillContent).toBe("# Safe instructions");
    expect(store.jobs.map((item) => item.id)).toEqual(jobsBefore);
    expect(actions).toContainEqual({ action: "list_skills" });
    expect(actions).toContainEqual({
      action: "read_skill",
      skill_id: "skill-one",
      resource_path: "SKILL.md",
      max_bytes: VCP_CLI_READ_BYTES,
    });
    expect(actions.some((action) => action.action === "run")).toBe(false);
  });

  it("keeps the combined stdout/stderr tail within the WebView byte budget", () => {
    const result = appendBoundedCliTail(
      "",
      "",
      "a".repeat(VCP_CLI_TAIL_BYTES),
      "错".repeat(VCP_CLI_TAIL_BYTES),
    );
    const total =
      new TextEncoder().encode(result.stdout).byteLength +
      new TextEncoder().encode(result.stderr).byteLength;

    expect(result.locallyTruncated).toBe(true);
    expect(total).toBeLessThanOrEqual(VCP_CLI_TAIL_BYTES + 3);
  });

  it("sends bounded long-poll defaults", async () => {
    vi.useFakeTimers();
    mockInvoke(VCP_CLI_STATUS_COMMAND, () =>
      status({ jobs: [summary("job-poll")] }),
    );
    mockInvoke(VCP_CLI_ACTION_COMMAND, (args) => {
      const request = requestFromArgs(args);
      if (request.action.action === "list") {
        return success(request.operation_id, { jobs: [summary("job-poll")] });
      }
      if (request.action.action === "poll") {
        return success(request.operation_id, {
          job: job("job-poll", "completed", { cursor: "cursor-final" }),
        });
      }
      throw new Error(`unexpected action ${request.action.action}`);
    });
    const store = useVcpCliStore();
    await store.openView();
    store.openJob(store.jobs[0]);
    await flushPromises();

    const pollCall = invokeMock.mock.calls.find(
      ([command, args]) =>
        command === VCP_CLI_ACTION_COMMAND &&
        requestFromArgs(args).action.action === "poll",
    );
    expect(requestFromArgs(pollCall?.[1]).action).toMatchObject({
      action: "poll",
      job_id: "job-poll",
      max_output_bytes: VCP_CLI_READ_BYTES,
      wait_ms: VCP_CLI_POLL_WAIT_MS,
    });
  });

  it("recovers one poll owner after a transient IPC failure", async () => {
    vi.useFakeTimers();
    mockInvoke(VCP_CLI_STATUS_COMMAND, () =>
      status({ jobs: [summary("job-retry")] }),
    );
    let pollAttempt = 0;
    mockInvoke(VCP_CLI_ACTION_COMMAND, (args) => {
      const request = requestFromArgs(args);
      if (request.action.action === "list") {
        return success(request.operation_id, { jobs: [summary("job-retry")] });
      }
      if (request.action.action === "poll") {
        pollAttempt += 1;
        if (pollAttempt === 1) throw new Error("temporary transport failure");
        return success(request.operation_id, {
          job: job("job-retry", "completed", {
            stdout: "recovered\n",
            cursor: "retry-cursor",
            exit_code: 0,
          }),
        });
      }
      throw new Error(`unexpected action ${request.action.action}`);
    });
    const store = useVcpCliStore();
    await store.openView();
    store.openJob(store.jobs[0]);
    await flushPromises();

    expect(store.jobDetailError?.message).toContain("自动重试");
    expect(pollAttempt).toBe(1);
    await vi.advanceTimersByTimeAsync(VCP_CLI_POLL_RETRY_DELAYS_MS[0]);
    expect(pollAttempt).toBe(2);
    expect(store.selectedJob?.state).toBe("completed");
    expect(store.selectedJob?.stdout).toBe("recovered\n");
  });

  it("stops a pending poll retry when the view closes", async () => {
    vi.useFakeTimers();
    mockInvoke(VCP_CLI_STATUS_COMMAND, () =>
      status({ jobs: [summary("job-close")] }),
    );
    let pollAttempt = 0;
    mockInvoke(VCP_CLI_ACTION_COMMAND, (args) => {
      const request = requestFromArgs(args);
      if (request.action.action === "list") {
        return success(request.operation_id, { jobs: [summary("job-close")] });
      }
      if (request.action.action === "poll") {
        pollAttempt += 1;
        throw new Error("temporary transport failure");
      }
      throw new Error(`unexpected action ${request.action.action}`);
    });
    const store = useVcpCliStore();
    await store.openView();
    store.openJob(store.jobs[0]);
    await flushPromises();
    expect(pollAttempt).toBe(1);

    store.closeView();
    await vi.advanceTimersByTimeAsync(VCP_CLI_POLL_RETRY_DELAYS_MS[0] * 2);
    expect(pollAttempt).toBe(1);
    expect(
      invokeMock.mock.calls.some(
        ([command, args]) =>
          command === VCP_CLI_ACTION_COMMAND &&
          requestFromArgs(args).action.action === "cancel",
      ),
    ).toBe(false);
  });

  it("refreshes status on a higher runtime generation and rejects old generation revival", async () => {
    const statuses = [
      status({ runtime_generation: 1, jobs: [summary("old-job")] }),
      status({ runtime_generation: 2, jobs: [summary("new-job", "queued")] }),
    ];
    let statusIndex = 0;
    const oldPoll = deferred<VcpCliActionResponse>();
    mockInvoke(VCP_CLI_STATUS_COMMAND, () => statuses[statusIndex++]);
    mockInvoke(VCP_CLI_ACTION_COMMAND, (args) => {
      const request = requestFromArgs(args);
      if (request.action.action === "list") {
        const generation = statusIndex >= 2 ? 2 : 1;
        const jobs =
          generation === 2
            ? [summary("new-job", "queued")]
            : [summary("old-job")];
        return success(request.operation_id, { jobs }, generation);
      }
      if (request.action.action === "poll") {
        if (request.action.job_id === "old-job") return oldPoll.promise;
        throw new Error("unexpected new-job poll");
      }
      if (request.action.action === "list_skills") {
        return success(request.operation_id, { skills: [] }, 2);
      }
      throw new Error(`unexpected action ${request.action.action}`);
    });
    const store = useVcpCliStore();
    await store.openView();
    store.openJob(store.jobs[0]);
    await store.loadSkills();
    await flushPromises();

    expect(store.runtimeGeneration).toBe(2);
    expect(store.selectedJobId).toBeNull();
    expect(store.runtimeLoading).toBe(false);
    expect(store.jobs.map((item) => item.id)).toEqual(["new-job"]);

    const oldPollCall = invokeMock.mock.calls.find(
      ([command, args]) =>
        command === VCP_CLI_ACTION_COMMAND &&
        requestFromArgs(args).action.action === "poll",
    );
    oldPoll.resolve(
      success(
        requestFromArgs(oldPollCall?.[1]).operation_id,
        {
          job: job("old-job", "completed", {
            stdout: "must not revive",
            cursor: "old-cursor",
          }),
        },
        1,
      ),
    );
    await flushPromises();
    expect(store.runtimeGeneration).toBe(2);
    expect(store.selectedJobId).toBeNull();
    expect(store.jobs.map((item) => item.id)).toEqual(["new-job"]);
  });
});
