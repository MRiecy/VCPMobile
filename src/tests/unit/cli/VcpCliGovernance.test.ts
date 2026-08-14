import { describe, expect, it } from "vitest";
import cliStoreSource from "@/features/cli/vcpCliStore.ts?raw";
import manifestViewSource from "@/features/cli/components/VcpCliManifestView.vue?raw";
import manifestPanelSource from "@/features/cli/components/VcpCliManifestPanel.vue?raw";
import runPanelSource from "@/features/cli/components/VcpCliRunPanel.vue?raw";
import skillsPanelSource from "@/features/cli/components/VcpCliSkillsPanel.vue?raw";
import terminalPanelSource from "@/features/cli/components/VcpCliTerminalPanel.vue?raw";
import protocolSource from "../../../../src-tauri/src/vcp_modules/cli/protocol.rs?raw";
import resultSource from "../../../../src-tauri/src/vcp_modules/cli/result.rs?raw";
import runtimeSource from "../../../../src-tauri/src/vcp_modules/cli/runtime.rs?raw";
import terminalBackendSource from "../../../../src-tauri/src/vcp_modules/cli/terminal.rs?raw";
import tauriLibSource from "../../../../src-tauri/src/lib.rs?raw";
import settingsManagerSource from "../../../../src-tauri/src/vcp_modules/infra/settings_manager.rs?raw";
import vcpClientSource from "../../../../src-tauri/src/vcp_modules/infra/vcp_client.rs?raw";
import settingsStoreSource from "@/core/stores/settings.ts?raw";
import settingsViewSource from "@/features/settings/SettingsView.vue?raw";
import aiLogicSettingsSource from "@/features/settings/components/AiLogicSettingsSection.vue?raw";
import chatStreamSource from "@/core/stores/chatStreamStore.ts?raw";
import semanticCacheMigrationSource from "../../../../src-tauri/migrations/0008_create_local_semantic_cache.sql?raw";
import semanticCacheRetirementSource from "../../../../src-tauri/migrations/0009_drop_retired_local_semantic_cache.sql?raw";

const productionSources = [
  cliStoreSource,
  manifestViewSource,
  manifestPanelSource,
  runPanelSource,
  skillsPanelSource,
  terminalPanelSource,
].join("\n");

describe("VCP CLI P1 cross-layer governance", () => {
  it("keeps manual PTY separate from Agent Jobs and bounds terminal traffic", () => {
    expect(manifestViewSource).toContain('{ id: \'terminal\', label: \'终端\' }');
    expect(manifestViewSource).toContain('{ id: \'jobs\', label: \'Jobs\' }');
    expect(terminalPanelSource).toContain('new Terminal({');
    expect(terminalPanelSource).toContain('registerOscHandler(52');
    expect(terminalPanelSource).toContain('maxBytes: 65_536');
    expect(terminalPanelSource).toContain('offset += 16_384');
    expect(terminalPanelSource).toContain('Detach only');
    expect(terminalPanelSource).toContain("Agent Jobs 不受影响");
    for (const command of [
      "open_vcp_mobile_cli_terminal",
      "read_vcp_mobile_cli_terminal",
      "write_vcp_mobile_cli_terminal",
      "resize_vcp_mobile_cli_terminal",
      "close_vcp_mobile_cli_terminal",
    ]) {
      expect(terminalBackendSource).toContain(`pub async fn ${command}`);
      expect(tauriLibSource).toContain(`${command},`);
    }
  });

  it("retires the local semantic cache without rewriting its published migration", () => {
    expect(semanticCacheMigrationSource).toContain(
      "CREATE TABLE IF NOT EXISTS local_semantic_embedding_cache",
    );
    expect(semanticCacheRetirementSource).toContain(
      "DROP TABLE IF EXISTS local_semantic_embedding_cache",
    );
    expect(semanticCacheRetirementSource).toContain(
      "DROP INDEX IF EXISTS idx_local_semantic_cache_lru",
    );
  });

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
    expect(cliStoreSource).toContain('action: "materialize_skill"');
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
      "MaterializeSkill",
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
    for (const command of [
      "get_vcp_mobile_cli_skill_catalog",
      "inspect_vcp_mobile_cli_skill_import",
      "commit_vcp_mobile_cli_skill_import",
      "discard_vcp_mobile_cli_skill_import",
    ]) {
      expect(cliStoreSource).toContain(command);
      expect(runtimeSource).toContain(`pub async fn ${command}`);
      expect(tauriLibSource).toContain(`${command},`);
    }
  });

  it("does not add a prompt, implicit Skill execution, raw HTML, or native offload path", () => {
    expect(productionSources).not.toMatch(/PromptCatalog/);
    expect(productionSources).not.toMatch(/SkillBridge/);
    expect(cliStoreSource).toContain('"plugin:vcp-mobile|pick_file"');
    expect(cliStoreSource).not.toContain("attachmentStore");
    expect(productionSources).not.toMatch(/context_assembler/);
    expect(productionSources).not.toContain("v-html");
    expect(productionSources).not.toContain("innerHTML");
    expect(productionSources).not.toMatch(/vcp-device|vcp-clipboard|ServerSocket/);
    expect(skillsPanelSource).toContain("materializeSelectedSkill");
    expect(skillsPanelSource).toContain("另发 run");
  });

  it("keeps the workbench flat, opaque, and within the radius policy", () => {
    expect(manifestViewSource).toContain("bg-[var(--primary-bg)]");
    expect(productionSources).not.toContain(["backdrop", "blur"].join("-"));
    expect(productionSources).not.toContain(["backdrop", "filter"].join("-"));
    expect(productionSources).not.toContain("rounded-2xl");
    expect(productionSources).not.toContain("rounded-3xl");
    expect(productionSources).not.toMatch(/z-\[|\bz-[0-9]+\b/);
  });

  it("converges the Agent loop to a single vcpPlugin route with no turn wire", () => {
    expect(settingsStoreSource).not.toContain("DEFAULT_MOBILE_CLI_AGENT_ROUTE");
    expect(settingsStoreSource).not.toContain("MobileCliAgentRoute");
    expect(settingsViewSource).not.toContain("onMobileCliRouteChange");
    expect(settingsViewSource).not.toContain("@route-change");
    expect(settingsManagerSource).not.toContain("MobileCliAgentRoute");
    expect(settingsManagerSource).not.toContain("mobile_cli_agent_route");

    expect(chatStreamSource).not.toContain("turnAttempt");
    expect(chatStreamSource).not.toContain("stepIndex");
    expect(chatStreamSource).not.toContain("projectionReset");
    expect(vcpClientSource).toContain("pub struct StreamEvent");
    expect(vcpClientSource).not.toContain("turn_attempt");
    expect(vcpClientSource).not.toContain("step_index");
    expect(vcpClientSource).not.toContain("projection_reset");
  });

  it("does not couple route selection to capability toggles or a frontend coordinator", () => {
    expect(aiLogicSettingsSource).not.toContain(
      'invoke("update_enabled_tools"',
    );
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
