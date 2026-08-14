import { beforeEach, describe, expect, it, vi } from "vitest";
import { openFileNative, writeTempFile } from "tauri-plugin-vcp-mobile";
import VcpCliManifestView from "@/features/cli/components/VcpCliManifestView.vue";
import featureOverlaysSource from "@/components/FeatureOverlays.vue?raw";
import rightSidebarSource from "@/components/layout/RightSidebar.vue?raw";
import overlaySource from "@/core/stores/overlay.ts?raw";
import manifestViewSource from "@/features/cli/components/VcpCliManifestView.vue?raw";
import cliManifestGoldenSource from "../../../../src-tauri/src/vcp_modules/cli/fixtures/vcp_mobile_cli_manifest.golden.json?raw";
import tauriLibSource from "../../../../src-tauri/src/lib.rs?raw";
import {
  LOCAL_ROUTE_GUIDE_STORAGE_KEY,
  VCP_CLI_MANIFEST_COMMAND,
  parseCanonicalVcpCliManifest,
} from "@/features/cli/manifest";
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

const canonicalManifest = cliManifestGoldenSource.trimEnd();
const manifestFixture = JSON.parse(canonicalManifest) as Record<
  string,
  unknown
>;

function mountView() {
  return mountWithPinia(VcpCliManifestView, {
    props: { isOpen: true, zIndex: 44 },
    global: { stubs: { VcpCliTerminalPanel: true } },
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
    vi.mocked(openFileNative).mockResolvedValue(undefined);
    mockInvoke(VCP_CLI_STATUS_COMMAND, () => readyStatus);
    mockInvoke(VCP_CLI_ACTION_COMMAND, (args) => {
      const request = args?.request as { operation_id: string };
      return successResponse(request.operation_id);
    });
    mockInvoke(VCP_CLI_MANIFEST_COMMAND, () => canonicalManifest);
  });

  it("preserves backend canonical text byte-for-byte and rejects object payloads", () => {
    const parsed = parseCanonicalVcpCliManifest(canonicalManifest);

    expect(parsed.rawJson).toBe(canonicalManifest);
    expect(parsed.byteLength).toBe(
      new TextEncoder().encode(canonicalManifest).byteLength,
    );
    expect(parsed.manifest.name).toBe("VCPMobileCLI");
    expect(() => parseCanonicalVcpCliManifest(manifestFixture)).toThrow(
      "无法保证复制与注册内容逐字一致",
    );
    expect(tauriLibSource).toContain(VCP_CLI_MANIFEST_COMMAND);
  });

  it("loads through the canonical command and explains prompt ownership", async () => {
    const wrapper = mountView();
    await openManifestTab(wrapper);
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith(VCP_CLI_MANIFEST_COMMAND);
    expect(wrapper.get('[data-vcp-cli-role="manifest-json"]').text()).toBe(
      canonicalManifest.trim(),
    );
    expect(wrapper.text()).toContain("提示词由用户 / VCPToolBox 所有");
    expect(wrapper.text()).toContain("不会自动注入、追加或改写 Agent 提示词");
    expect(wrapper.text()).toContain("当前可用性与 Job 状态以运行页的 Rust");
  });

  it("copies and exports the exact backend text with controlled Android APIs", async () => {
    const wrapper = mountView();
    await openManifestTab(wrapper);
    await flushPromises();

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
    expect(wrapper.text()).toContain("已生成临时 JSON");
  });

  it("records only that the one-time guide was read and allows reopening it", async () => {
    const wrapper = mountView();
    await openManifestTab(wrapper);
    await flushPromises();

    const guideToggle = wrapper.get(
      'button[aria-controls="vcp-cli-local-route-guide"]',
    );
    expect(guideToggle.attributes("aria-expanded")).toBe("true");

    const acknowledge = wrapper
      .findAll("button")
      .find((button) => button.text().includes("不再自动展开"));
    expect(acknowledge).toBeDefined();
    await acknowledge!.trigger("click");

    expect(localStorage.getItem(LOCAL_ROUTE_GUIDE_STORAGE_KEY)).toBe("1");
    expect(guideToggle.attributes("aria-expanded")).toBe("false");
    expect(wrapper.text()).toContain("已读");

    await guideToggle.trigger("click");
    expect(guideToggle.attributes("aria-expanded")).toBe("true");
    expect(wrapper.text()).toContain("[[VCPToolUse=Forbidden]]");
  });

  it("enables system sharing only when the WebView exposes a real share function", async () => {
    const unavailable = mountView();
    await openManifestTab(unavailable);
    await flushPromises();
    expect(
      unavailable.get('[data-vcp-cli-action="share"]').attributes("disabled"),
    ).toBeDefined();
    expect(unavailable.text()).toContain("ACTION_SEND");
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

    expect(wrapper.text()).toContain("manifest 暂不可用");
    expect(wrapper.text()).toContain("逐字一致");
    expect(
      wrapper.get('[data-vcp-cli-action="copy"]').attributes("disabled"),
    ).toBeDefined();
    expect(
      wrapper.get('[data-vcp-cli-action="export"]').attributes("disabled"),
    ).toBeDefined();
  });

  it("wires the More tray into the semantic SlidePage stack with a plain opaque surface", () => {
    expect(rightSidebarSource).toContain("overlayStore.openCliManifest()");
    expect(rightSidebarSource).toContain("label: 'VCP CLI'");
    expect(overlaySource).toContain("| 'cliManifest'");
    expect(overlaySource).toContain(
      "pageStackTop.value?.type !== 'cliManifest'",
    );
    expect(featureOverlaysSource).toContain(
      "defineAsyncComponent(() => import('../features/cli/components/VcpCliManifestView.vue'))",
    );
    expect(featureOverlaysSource).toContain(
      "createFirstOpenLatch(() => overlayStore.isCliManifestOpen)",
    );
    expect(featureOverlaysSource).toContain(
      "overlayStore.getPageZIndex('cliManifest')",
    );
    expect(manifestViewSource).toContain("bg-[var(--primary-bg)]");
    expect(manifestViewSource).not.toContain("rounded-2xl");
  });
});
