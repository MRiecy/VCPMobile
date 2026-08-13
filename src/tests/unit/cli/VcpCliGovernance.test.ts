import { describe, expect, it } from "vitest";
import cliStoreSource from "@/features/cli/vcpCliStore.ts?raw";
import manifestViewSource from "@/features/cli/components/VcpCliManifestView.vue?raw";
import manifestPanelSource from "@/features/cli/components/VcpCliManifestPanel.vue?raw";
import runPanelSource from "@/features/cli/components/VcpCliRunPanel.vue?raw";
import skillsPanelSource from "@/features/cli/components/VcpCliSkillsPanel.vue?raw";
import protocolSource from "../../../../src-tauri/src/vcp_modules/cli/protocol.rs?raw";
import resultSource from "../../../../src-tauri/src/vcp_modules/cli/result.rs?raw";
import runtimeSource from "../../../../src-tauri/src/vcp_modules/cli/runtime.rs?raw";
import tauriLibSource from "../../../../src-tauri/src/lib.rs?raw";
import settingsManagerSource from "../../../../src-tauri/src/vcp_modules/infra/settings_manager.rs?raw";
import vcpClientSource from "../../../../src-tauri/src/vcp_modules/infra/vcp_client.rs?raw";
import settingsStoreSource from "@/core/stores/settings.ts?raw";
import settingsViewSource from "@/features/settings/SettingsView.vue?raw";
import aiLogicSettingsSource from "@/features/settings/components/AiLogicSettingsSection.vue?raw";
import chatStreamSource from "@/core/stores/chatStreamStore.ts?raw";

const productionSources = [
  cliStoreSource,
  manifestViewSource,
  manifestPanelSource,
  runPanelSource,
  skillsPanelSource,
].join("\n");

describe("VCP CLI P1 cross-layer governance", () => {
  it("keeps the structured Tauri command and snake_case action contract", () => {
    expect(cliStoreSource).toContain(
      'VCP_CLI_STATUS_COMMAND = "get_vcp_mobile_cli_status"',
    );
    expect(cliStoreSource).toContain(
      'VCP_CLI_ACTION_COMMAND = "execute_vcp_mobile_cli_action"',
    );
    expect(cliStoreSource).toContain("operation_id: operationId");
    expect(cliStoreSource).toContain('action: "run"');
    expect(cliStoreSource).toContain('action: "poll"');
    expect(cliStoreSource).toContain('action: "cancel"');
    expect(cliStoreSource).toContain('action: "list"');
    expect(cliStoreSource).toContain('action: "list_skills"');
    expect(cliStoreSource).toContain('action: "read_skill"');
    expect(cliStoreSource).toContain('resource_path: "SKILL.md"');
    expect(cliStoreSource).toContain("max_output_bytes: VCP_CLI_READ_BYTES");
    expect(cliStoreSource).toContain("wait_ms: VCP_CLI_POLL_WAIT_MS");

    expect(protocolSource).toContain(
      '#[serde(tag = "action", rename_all = "snake_case")]',
    );
    for (const variant of [
      "Run",
      "ListSkills",
      "ReadSkill",
      "Poll",
      "Cancel",
      "List",
    ]) {
      expect(protocolSource).toMatch(new RegExp(`\\b${variant}\\b`));
    }
    expect(resultSource).toMatch(/enum VcpCliJobState[\s\S]*\bStarting\b/);
    expect(resultSource).toContain("pub command_preview: String");
    expect(runtimeSource).toContain("pub struct MobileCliStatus");
    expect(runtimeSource).toContain("pub availability_reason: Option<String>");
    expect(runtimeSource).toContain("pub background_reliability: String");
    expect(runtimeSource).toContain("pub runtime_generation: u64");
    expect(runtimeSource).toContain("pub struct ExecuteVcpMobileCliRequest");
    expect(runtimeSource).toContain("pub operation_id: String");
    expect(runtimeSource).toContain("pub action: VcpCliAction");
    expect(runtimeSource).toContain("pub envelope: VcpCliResultEnvelope");
    expect(runtimeSource).toContain("pub async fn get_vcp_mobile_cli_status");
    expect(runtimeSource).toContain(
      "pub async fn execute_vcp_mobile_cli_action",
    );
    expect(tauriLibSource).toContain("MobileCliRuntimeState::new()");
    expect(tauriLibSource).toContain("get_vcp_mobile_cli_status,");
    expect(tauriLibSource).toContain("execute_vcp_mobile_cli_action,");
  });

  it("does not add a prompt, Skill execution, PTY, HTML, or terminal-emulator path", () => {
    expect(productionSources).not.toMatch(/PromptCatalog/);
    expect(productionSources).not.toMatch(/SkillBridge/);
    expect(productionSources).not.toMatch(/context_assembler/);
    expect(productionSources).not.toMatch(/\bPTY\b/);
    expect(productionSources).not.toMatch(/xterm|TerminalEmulator/);
    expect(productionSources).not.toContain("v-html");
    expect(productionSources).not.toContain("innerHTML");
  });

  it("keeps the workbench flat, opaque, and within the radius policy", () => {
    expect(manifestViewSource).toContain("bg-[var(--primary-bg)]");
    expect(productionSources).not.toContain(["backdrop", "blur"].join("-"));
    expect(productionSources).not.toContain(["backdrop", "filter"].join("-"));
    expect(productionSources).not.toContain("rounded-2xl");
    expect(productionSources).not.toContain("rounded-3xl");
    expect(productionSources).not.toMatch(/z-\[|\bz-[0-9]+\b/);
  });

  it("keeps the P2 route explicit and the stream-step contract top-level", () => {
    expect(settingsStoreSource).toContain(
      'DEFAULT_MOBILE_CLI_AGENT_ROUTE = "localLoopback"',
    );
    expect(settingsStoreSource).toMatch(
      /type MobileCliAgentRoute\s*=\s*[\s\S]*"vcpPlugin"/,
    );
    expect(settingsStoreSource).toContain(
      "mobileCliAgentRoute: MobileCliAgentRoute",
    );
    expect(settingsViewSource).toContain(
      "@route-change=\"onMobileCliRouteChange\"",
    );
    expect(settingsManagerSource).toMatch(
      /enum MobileCliAgentRoute[\s\S]*\bLocalLoopback\b[\s\S]*\bVcpPlugin\b/,
    );
    expect(settingsManagerSource).toContain(
      "pub mobile_cli_agent_route: MobileCliAgentRoute",
    );

    expect(chatStreamSource).toContain("turnAttempt?: string");
    expect(chatStreamSource).toContain("stepIndex?: number");
    expect(chatStreamSource).toContain("projectionReset?: boolean");
    expect(chatStreamSource).not.toContain("localCliTurnAttempt");
    expect(chatStreamSource).not.toContain("localCliStep");
    expect(chatStreamSource).not.toContain("localCliProjectionReset");
    expect(vcpClientSource).toContain('#[serde(rename_all = "camelCase")]');
    expect(vcpClientSource).toContain("pub struct StreamEvent");
    expect(vcpClientSource).toContain("pub turn_attempt: Option<String>");
    expect(vcpClientSource).toContain("pub step_index: Option<u32>");
    expect(vcpClientSource).toContain("pub projection_reset: Option<bool>");
  });

  it("does not couple route selection to capability toggles or a frontend coordinator", () => {
    expect(aiLogicSettingsSource).not.toContain('invoke("update_enabled_tools"');
    expect(aiLogicSettingsSource).not.toContain('invoke("update_settings"');
    expect(aiLogicSettingsSource).not.toMatch(/\.distributedEnabled\s*=/);
    expect(aiLogicSettingsSource).not.toMatch(/enableVcpToolInjection\s*=/);

    const p2Sources = [
      settingsStoreSource,
      settingsViewSource,
      aiLogicSettingsSource,
      chatStreamSource,
    ].join("\n");
    expect(p2Sources).not.toMatch(/PromptCatalog|SkillBridge/);
    expect(p2Sources).not.toMatch(/FrontendCliCoordinator|CliJobCard/);
  });
});
