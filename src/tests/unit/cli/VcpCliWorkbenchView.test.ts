import { beforeEach, describe, expect, it } from "vitest";
import VcpCliManifestView from "@/features/cli/components/VcpCliManifestView.vue";
import { useModalHistory } from "@/core/composables/useModalHistory";
import {
  VCP_CLI_ACTION_COMMAND,
  VCP_CLI_NATIVE_PICK_FILE_COMMAND,
  VCP_CLI_SKILL_CATALOG_COMMAND,
  VCP_CLI_SKILL_IMPORT_INSPECT_COMMAND,
  VCP_CLI_STATUS_COMMAND,
  type VcpCliAction,
  type VcpCliActionResponse,
  type VcpCliResultBody,
  type VcpCliRuntimeStatus,
} from "@/features/cli/vcpCliStore";
import { invokeMock, mockInvoke } from "@/tests/mocks/tauri";
import { flushPromises } from "@/tests/utils/flush";
import { mountWithPinia } from "@/tests/utils/mount";

function runtimeStatus(
  patch: Partial<VcpCliRuntimeStatus> = {},
): VcpCliRuntimeStatus {
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

function response(
  operationId: string,
  result: Partial<VcpCliResultBody> = {},
): VcpCliActionResponse {
  return {
    operation_id: operationId,
    runtime_generation: 1,
    envelope: {
      status: "success",
      result: { content: [], ...result },
    },
  };
}

function requestFromArgs(args?: Record<string, unknown>) {
  return args?.request as { operation_id: string; action: VcpCliAction };
}

function mountView() {
  return mountWithPinia(VcpCliManifestView, {
    props: { isOpen: true, zIndex: 44 },
  });
}

describe("VCP CLI mobile workbench", () => {
  beforeEach(() => {
    mockInvoke(VCP_CLI_STATUS_COMMAND, () => runtimeStatus());
    mockInvoke(VCP_CLI_ACTION_COMMAND, (args) => {
      const request = requestFromArgs(args);
      if (request.action.action === "list") {
        return response(request.operation_id, { jobs: [] });
      }
      throw new Error(`unexpected action ${request.action.action}`);
    });
  });

  it("shows the first-install reason without deadlocking command execution", async () => {
    mockInvoke(VCP_CLI_STATUS_COMMAND, () =>
      runtimeStatus({
        available: false,
        availability_reason: "本地 rootfs 尚未准备",
        phase: "unprovisioned",
      }),
    );
    const wrapper = mountView();
    await flushPromises();

    await wrapper
      .get('[data-vcp-cli-role="command-input"]')
      .setValue("echo first-run");
    expect(wrapper.text()).toContain("本地 rootfs 尚未准备");
    const runButton = wrapper.get('[data-vcp-cli-action="run"]');
    expect(runButton.text()).toContain("准备并执行");
    expect(runButton.attributes("disabled")).toBeUndefined();
    expect(
      invokeMock.mock.calls.some(
        ([command, args]) =>
          command === VCP_CLI_ACTION_COMMAND &&
          requestFromArgs(args).action.action === "run",
      ),
    ).toBe(false);
    wrapper.unmount();
  });

  it("shows active preparation and prevents a duplicate run", async () => {
    mockInvoke(VCP_CLI_STATUS_COMMAND, () =>
      runtimeStatus({
        available: false,
        availability_reason: null,
        phase: "preparing",
      }),
    );
    const wrapper = mountView();
    await flushPromises();
    await wrapper
      .get('[data-vcp-cli-role="command-input"]')
      .setValue("echo wait");

    expect(wrapper.get('[data-vcp-cli-phase="preparing"]').text()).toContain(
      "正在准备本地 Runtime",
    );
    expect(wrapper.text()).toContain("正在校验并解包内置 PRoot / Alpine");
    expect(
      wrapper.get('[data-vcp-cli-action="run"]').attributes("disabled"),
    ).toBeDefined();
    expect(wrapper.get('[data-vcp-cli-action="run"]').text()).toContain(
      "准备中",
    );
    wrapper.unmount();
  });

  it("keeps a failed preparation reason visible and retryable", async () => {
    mockInvoke(VCP_CLI_STATUS_COMMAND, () =>
      runtimeStatus({
        available: false,
        availability_reason: "内置 rootfs SHA-256 校验失败",
        phase: "error",
      }),
    );
    const wrapper = mountView();
    await flushPromises();
    await wrapper
      .get('[data-vcp-cli-role="command-input"]')
      .setValue("echo retry");

    expect(wrapper.get('[data-vcp-cli-phase="error"]').text()).toContain(
      "准备失败",
    );
    expect(wrapper.text()).toContain("内置 rootfs SHA-256 校验失败");
    const runButton = wrapper.get('[data-vcp-cli-action="run"]');
    expect(runButton.text()).toContain("重试准备");
    expect(runButton.attributes("disabled")).toBeUndefined();
    wrapper.unmount();
  });

  it("keeps Enter as a newline and runs once only through the explicit button", async () => {
    mockInvoke(VCP_CLI_ACTION_COMMAND, (args) => {
      const request = requestFromArgs(args);
      if (request.action.action === "list") {
        return response(request.operation_id, { jobs: [] });
      }
      if (request.action.action === "run") {
        return response(request.operation_id, {
          job: {
            id: "job-command",
            state: "completed",
            stdout: "first\nsecond\n",
            stderr: "",
            exit_code: 0,
            cursor: "cursor-final",
            truncated: false,
            artifact: null,
          },
        });
      }
      throw new Error(`unexpected action ${request.action.action}`);
    });
    const wrapper = mountView();
    await flushPromises();
    const input = wrapper.get('[data-vcp-cli-role="command-input"]');
    await input.setValue("printf 'first\\n'\nprintf 'second\\n'");
    await input.trigger("keydown", { key: "Enter" });
    await flushPromises();

    const runCalls = () =>
      invokeMock.mock.calls.filter(
        ([command, args]) =>
          command === VCP_CLI_ACTION_COMMAND &&
          requestFromArgs(args).action.action === "run",
      );
    expect(runCalls()).toHaveLength(0);

    const runButton = wrapper.get('[data-vcp-cli-action="run"]');
    expect(runButton.attributes("disabled")).toBeUndefined();
    await runButton.trigger("click");
    await flushPromises();
    expect(runCalls()).toHaveLength(1);
    expect(requestFromArgs(runCalls()[0][1]).operation_id).toMatch(
      /^vcp-cli-run-/,
    );
    expect(requestFromArgs(runCalls()[0][1]).action).toMatchObject({
      action: "run",
      command: "printf 'first\\n'\nprintf 'second\\n'",
      cwd: "/workspace",
      run_in_background: false,
    });
    expect(wrapper.get('[data-vcp-cli-role="job-stdout"]').text()).toContain(
      "first\nsecond",
    );
    wrapper.unmount();
  });

  it("renders SKILL.md as inert text and returns to the list before closing", async () => {
    const seenActions: VcpCliAction[] = [];
    mockInvoke(VCP_CLI_ACTION_COMMAND, (args) => {
      const request = requestFromArgs(args);
      seenActions.push(request.action);
      if (request.action.action === "list") {
        return response(request.operation_id, { jobs: [] });
      }
      if (request.action.action === "list_skills") {
        return response(request.operation_id, {
          skills: [
            {
              id: "safe-skill",
              name: "Safe Skill",
              version: "1.0.0",
              source: "bundled",
              sha256: "abc123",
            },
          ],
        });
      }
      if (request.action.action === "read_skill") {
        return response(request.operation_id, {
          content: [
            {
              type: "text",
              text: "# Skill\n<script>window.evil = true</script>",
            },
          ],
          skill: {
            id: "safe-skill",
            name: "Safe Skill",
            resource_path: "SKILL.md",
            skill_root: "vcp-skill://safe-skill",
            sha256: "abc123",
            truncated: false,
          },
        });
      }
      throw new Error(`unexpected action ${request.action.action}`);
    });
    const wrapper = mountView();
    await flushPromises();

    await wrapper.get('[data-vcp-cli-tab="skills"]').trigger("click");
    await flushPromises();
    await wrapper.get('[data-vcp-cli-role="skill-row"]').trigger("click");
    await flushPromises();

    const content = wrapper.get('[data-vcp-cli-role="skill-content"]');
    expect(content.text()).toContain("<script>window.evil = true</script>");
    expect(content.find("script").exists()).toBe(false);
    expect(seenActions).toContainEqual({
      action: "read_skill",
      skill_id: "safe-skill",
      resource_path: "SKILL.md",
      max_bytes: 65_536,
    });
    expect(seenActions.some((action) => action.action === "run")).toBe(false);

    expect(useModalHistory().closeTopModal()).toBe(true);
    await flushPromises();
    expect(wrapper.find('[data-vcp-cli-role="skill-detail"]').exists()).toBe(
      false,
    );
    expect(wrapper.find('[data-vcp-cli-tab="skills"]').exists()).toBe(true);
    expect(wrapper.emitted("close")).toBeUndefined();
    wrapper.unmount();
  });

  it("shows catalog warnings and requires a second click after ZIP inspection", async () => {
    mockInvoke(VCP_CLI_ACTION_COMMAND, (args) => {
      const request = requestFromArgs(args);
      if (request.action.action === "list") {
        return response(request.operation_id, { jobs: [] });
      }
      if (request.action.action === "list_skills") {
        return response(request.operation_id, { skills: [] });
      }
      throw new Error(`unexpected action ${request.action.action}`);
    });
    mockInvoke(VCP_CLI_SKILL_CATALOG_COMMAND, () => ({
      schema_version: 2,
      generation: 7,
      skills: [],
      warnings: ["Invalid Skill broken: integrity failed"],
    }));
    mockInvoke(VCP_CLI_NATIVE_PICK_FILE_COMMAND, () => ({
      path: "/cache/uploads/sample.zip",
      name: "sample.zip",
      mime: "application/zip",
      size: 99,
      hash: "a".repeat(64),
    }));
    mockInvoke(VCP_CLI_SKILL_IMPORT_INSPECT_COMMAND, () => ({
      token: "vcp-skill-import-v1:00000000-0000-4000-8000-000000000002",
      candidate_sha256: "a".repeat(64),
      catalog_generation: 7,
      skill_id: "sample",
      name: "Sample",
      description: "Review me",
      version: null,
      source_name: "sample.zip",
      resource_count: 2,
      total_bytes: 99,
      tree_sha256: "b".repeat(64),
      replaces_existing: false,
      warnings: ["包含 scripts/；导入不会执行脚本。"],
    }));
    const wrapper = mountView();
    await flushPromises();
    await wrapper.get('[data-vcp-cli-tab="skills"]').trigger("click");
    await flushPromises();

    expect(wrapper.text()).toContain("Invalid Skill broken");
    await wrapper.get('[data-vcp-cli-action="import-skill"]').trigger("click");
    await flushPromises();
    const review = wrapper.get('[data-vcp-cli-role="skill-import-review"]');
    expect(review.text()).toContain("Sample");
    expect(review.text()).toContain("导入不会执行脚本");
    expect(
      wrapper.find('[data-vcp-cli-action="commit-skill-import"]').exists(),
    ).toBe(true);
    expect(
      invokeMock.mock.calls.some(
        ([command]) => command === "commit_vcp_mobile_cli_skill_import",
      ),
    ).toBe(false);
    wrapper.unmount();
  });
});
