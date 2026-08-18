import { describe, expect, it } from "vitest";
import coreStatusSource from "@/components/ui/CoreStatusIndicator.vue?raw";
import featureOverlaysSource from "@/components/FeatureOverlays.vue?raw";
import rightSidebarSource from "@/components/layout/RightSidebar.vue?raw";
import ragObserverSource from "@/features/rag/RagObserver.vue?raw";
import toolBlockSource from "@/features/chat/blocks/ToolBlock.vue?raw";
import settingsViewSource from "@/features/settings/SettingsView.vue?raw";

describe("mobile performance governance", () => {
  it("keeps the steady Core-ready indicator free of perpetual animation", () => {
    expect(coreStatusSource).not.toContain("vcpCorePulse");
    expect(coreStatusSource).not.toContain("vcp-core-pulse");
    expect(coreStatusSource).not.toContain("will-change");
    expect(coreStatusSource).not.toContain("animate-pulse");
    expect(coreStatusSource).not.toContain("animate-bounce");
    expect(rightSidebarSource).not.toContain("animate-pulse");
  });

  it("mounts low-frequency feature pages only after their first open", () => {
    const latches = [
      "isSettingsOpen",
      "isAgentSettingsOpen",
      "isGroupSettingsOpen",
      "isTarvenSettingsOpen",
      "isDistributedOpen",
      "isRagObserverOpen",
      "isDiaryCenterOpen",
      "isCliManifestOpen",
      "isLogCenterOpen",
    ];

    for (const openState of latches) {
      expect(featureOverlaysSource).toContain(
        `createFirstOpenLatch(() => overlayStore.${openState})`,
      );
    }
    expect(featureOverlaysSource).toContain(
      "defineAsyncComponent(() => import('../features/chat/components/TarvenSettings.vue'))",
    );
  });

  it("lazy-loads settings sections so first open does not parse the whole feature", () => {
    for (const sectionPath of [
      "./components/UserProfileSection.vue",
      "./components/SyncSettingsSection.vue",
      "./components/VcpCoreSettingsSection.vue",
      "./ThemePicker.vue",
      "./components/AboutSection.vue",
      "../../components/ModelSelector.vue",
    ]) {
      expect(settingsViewSource).toContain(
        `defineAsyncComponent(() => import("${sectionPath}"))`,
      );
    }
    expect(settingsViewSource).not.toContain("import UserProfileSection from");
    expect(settingsViewSource).not.toContain("import ThemePicker from");
    expect(settingsViewSource).not.toContain("import ModelSelector from");
    expect(settingsViewSource).not.toContain("import AboutSection from");
    expect(settingsViewSource).toContain("summarySelectorMounted");
  });

  it("allows only the explicit loading spinner to animate inside ToolBlock", () => {
    expect(toolBlockSource).not.toContain("IntersectionObserver");
    expect(toolBlockSource).toContain(
      `v-if="type === 'tool-use' && !block.is_complete"`,
    );
    expect(toolBlockSource).toContain('class="custom-spin"');
  });

  it("stops the RAG spectrum scheduler after its bars settle", () => {
    expect(ragObserverSource.match(/requestAnimationFrame\(render\)/g)).toHaveLength(1);
    expect(ragObserverSource).toContain("if (needsAnotherFrame)");
    expect(ragObserverSource).toContain("drawSpectrum(true)");
  });
});
