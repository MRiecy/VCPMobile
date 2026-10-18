import { beforeEach, describe, expect, it, vi } from "vitest";
import { openFileNative, writeTempFile } from "tauri-plugin-vcp-mobile";
import VcpCliManifestView from "@/features/cli/components/VcpCliManifestView.vue";
import { VCP_CLI_MANIFEST_COMMAND, type VcpCliManifest } from "@/features/cli/manifest";
import {
  VCP_CLI_ACTION_COMMAND,
  VCP_CLI_STATUS_COMMAND,
  type VcpCliActionResponse,
  type VcpCliResultBody,
  type VcpCliRuntimeStatus,
} from "@/features/cli/vcpCliStore";
import { invokeMock, mockInvoke } from "@/tests/mocks/tauri";
import { flushPromises } from "@/tests/utils/flush";
import { mountWithPinia } from "@/tests/utils/mount";

vi.mock("@/core/utils/platform", () => ({
  nativePlatform: "android", isAndroid: true, isIOS: false,
}));

const manifestFixture = {
  manifestVersion: "1.0.0",
  name: "VCPMobileCLI",
  version: "1.0.0",
  displayName: "VCP Mobile CLI",
  description: "测试工具说明",
  author: "VCPMobile",
  pluginType: "synchronous",
  entryPoint: { type: "mobile", command: "native" },
  communication: { protocol: "mobile", timeout: 30_000 },
  capabilities: {
    invocationCommands: [
      {
        commandIdentifier: "VCPMobileCLI",
        description: "执行命令",
        example: "示例命令",
      },
    ],
  },
} satisfies VcpCliManifest;
// Deliberately use tabs to catch copy/export reserialization without freezing production prose.
const canonicalManifest = JSON.stringify(manifestFixture, null, "\t");

function mountView() {
  return mountWithPinia(VcpCliManifestView, {
    props: { isOpen: true, zIndex: 44 },
    global: { stubs: { VcpCliTerminalPanel: true } },
  });
}

function mountViewWithAllPanelsStubbed() {
  return mountWithPinia(VcpCliManifestView, {
    props: { isOpen: true, zIndex: 44 },
    global: {
      stubs: {
        VcpCliTerminalPanel: true,
        VcpCliRunPanel: true,
        VcpCliSkillsPanel: true,
      },
    },
  });
}

const readyStatus: VcpCliRuntimeStatus = {
  available: true,
  availability_reason: null,
  background_reliability: "foreground_only",
  runtime_generation: 1,
  phase: "ready",
  profile_id: "alpine-arm64-v1",
  max_concurrent_jobs: 2,
  running_jobs: 0,
  jobs: [],
};

function successResponse(
  operationId: string,
  result: Partial<VcpCliResultBody> = {},
): VcpCliActionResponse {
  return {
    operation_id: operationId,
    runtime_generation: 1,
    envelope: { status: "success", result: { content: [], ...result } },
  };
}

async function openManifestTab(
  wrapper: ReturnType<typeof mountView>,
): Promise<void> {
  await wrapper.get('[data-vcp-cli-action="open-info"]').trigger("click");
  await flushPromises();
}

describe("VCP CLI canonical manifest boundary", () => {
  beforeEach(() => {
    localStorage.clear();
    Object.defineProperty(navigator, "share", {
      configurable: true,
      writable: true,
      value: undefined,
    });
    vi.mocked(navigator.clipboard.writeText).mockResolvedValue(undefined);
    vi.mocked(writeTempFile).mockResolvedValue(
      "/cache/VCPMobileCLI.manifest.json",
    );
    vi.mocked(openFileNative).mockResolvedValue({ outcome: 'opened' });
    mockInvoke(VCP_CLI_STATUS_COMMAND, () => readyStatus);
    mockInvoke(VCP_CLI_ACTION_COMMAND, (args) => {
      const request = args?.request as { operation_id: string };
      return successResponse(request.operation_id);
    });
    mockInvoke(VCP_CLI_MANIFEST_COMMAND, () => canonicalManifest);
  });


  it("shows the manifest help on Jobs and Skills tabs instead of the tab panels", async () => {
    for (const tab of ["jobs", "skills"] as const) {
      const wrapper = mountViewWithAllPanelsStubbed();
      await wrapper.get(`[data-vcp-cli-tab="${tab}"]`).trigger("click");
      await flushPromises();
      // 打开工具说明前：对应页签面板在渲染
      const tabStub =
        tab === "jobs" ? "vcp-cli-run-panel-stub" : "vcp-cli-skills-panel-stub";
      expect(wrapper.findAll(tabStub).length).toBe(1);

      await openManifestTab(wrapper);
      expect(wrapper.findAll('[data-vcp-cli-role="manifest-json"]').length).toBe(1);
      // 工具说明优先于页签面板，正文不再停留在 jobs/skills
      expect(wrapper.findAll("vcp-cli-run-panel-stub").length).toBe(0);
      expect(wrapper.findAll("vcp-cli-skills-panel-stub").length).toBe(0);
      wrapper.unmount();
    }
  });

  it("copies and exports the exact backend text with controlled Android APIs", async () => {
    const wrapper = mountView();
    await openManifestTab(wrapper);
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith(VCP_CLI_MANIFEST_COMMAND);
    await wrapper.get('[data-vcp-cli-action="copy"]').trigger("click");
    await flushPromises();
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith(
      canonicalManifest,
    );

    await wrapper.get('[data-vcp-cli-action="export"]').trigger("click");
    await flushPromises();

    expect(writeTempFile).toHaveBeenCalledTimes(1);
    const [bytes, fileName] = vi.mocked(writeTempFile).mock.calls[0];
    expect(new TextDecoder().decode(bytes)).toBe(canonicalManifest);
    expect(fileName).toBe("VCPMobileCLI-1.0.0.manifest.json");
    expect(openFileNative).toHaveBeenCalledWith(
      "/cache/VCPMobileCLI.manifest.json",
    );
  });

  it("enables system sharing only when the WebView exposes a real share function", async () => {
    const unavailable = mountView();
    await openManifestTab(unavailable);
    await flushPromises();
    expect(
      unavailable.get('[data-vcp-cli-action="share"]').attributes("disabled"),
    ).toBeDefined();
    unavailable.unmount();

    const share = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "share", {
      configurable: true,
      writable: true,
      value: share,
    });
    const available = mountView();
    await openManifestTab(available);
    await flushPromises();
    await available.get('[data-vcp-cli-action="share"]').trigger("click");
    await flushPromises();

    expect(share).toHaveBeenCalledWith({
      title: "VCP Mobile CLI manifest",
      text: canonicalManifest,
    });
  });

  it("does not enable delivery actions for a non-canonical backend shape", async () => {
    mockInvoke(VCP_CLI_MANIFEST_COMMAND, () => manifestFixture);
    const wrapper = mountView();
    await openManifestTab(wrapper);
    await flushPromises();

    expect(
      wrapper.get('[data-vcp-cli-action="copy"]').attributes("disabled"),
    ).toBeDefined();
    expect(
      wrapper.get('[data-vcp-cli-action="export"]').attributes("disabled"),
    ).toBeDefined();
  });
});
