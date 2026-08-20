import { describe, expect, it } from "vitest";
import distributedViewSource from "@/features/distributed/DistributedView.vue?raw";
import aiLogicSettingsSource from "@/features/settings/components/AiLogicSettingsSection.vue?raw";
import commandsSource from "../../../../src-tauri/src/commands.rs?raw";
import distributedCommandsSource from "../../../../src-tauri/src/distributed/mod.rs?raw";
import registrySource from "../../../../src-tauri/src/distributed/tool_registry.rs?raw";

describe("distributed tool authorization contract", () => {
  it("submits an explicit enabled allowlist and never reconstructs disabled complements", () => {
    expect(distributedViewSource.match(/update_enabled_tools/g)).toHaveLength(
      1,
    );
    expect(distributedViewSource).toContain(
      'await invoke("update_enabled_tools", {',
    );
    expect(distributedViewSource).toContain(
      "pluginsList.value.filter((tool) => tool.enabled)",
    );
    expect(distributedViewSource).toContain(
      "enabledNames: [...enabledNames].sort",
    );
    expect(distributedViewSource).toContain("enabled: tool.enabled === true");
    expect(distributedViewSource).not.toContain("update_disabled_tools");
    expect(distributedViewSource).not.toContain("disabledNames");
  });

  it("keeps authorization mutation in the catalog owner and uses accessible explicit controls", () => {
    expect(aiLogicSettingsSource).not.toContain("update_enabled_tools");
    expect(aiLogicSettingsSource).not.toContain("update_disabled_tools");
    expect(distributedViewSource).toContain('role="switch"');
    expect(distributedViewSource).toContain(':aria-checked="plugin.enabled"');
    expect(distributedViewSource).toContain(':data-tool-read="plugin.id"');
    expect(distributedViewSource).toContain(
      ':disabled="!plugin.enabled || pluginLoading[plugin.id] || authorizationMutationPending"',
    );
    expect(distributedViewSource).toContain(
      "v-if=\"plugin.type === 'streaming'\"",
    );
    expect(distributedViewSource).toContain('plugin.type !== "streaming"');
    expect(distributedViewSource).toContain("需由 VCP 请求明确调用");
  });

  it("reads only the dedicated distributed connection fields", () => {
    const loadSettingsBody = distributedViewSource
      .split("const loadSettings =")[1]
      .split("const handleConnect =")[0];
    expect(loadSettingsBody).toContain(
      "settingsStore.settings.distributedWsUrl",
    );
    expect(loadSettingsBody).toContain(
      "settingsStore.settings.distributedVcpKey",
    );
    expect(loadSettingsBody).not.toContain("vcpLogUrl");
    expect(loadSettingsBody).not.toContain("vcpServerUrl");
    expect(loadSettingsBody).not.toContain("vcpLogKey");
    expect(loadSettingsBody).not.toContain("vcpApiKey");
  });

  it("registers only the enabled-name command and keeps new tools fail-closed", () => {
    expect(commandsSource).toContain("distributed::update_enabled_tools,");
    expect(commandsSource).not.toContain("distributed::update_disabled_tools,");
    expect(distributedCommandsSource).toContain("enabled_names: Vec<String>");
    expect(registrySource).toContain("enabled_names: RwLock<HashSet<String>>");
    expect(registrySource).toContain(
      "config_update_lock: tokio::sync::Mutex<()>",
    );
    expect(registrySource).toContain("guard.contains(name)");
    expect(registrySource).not.toContain("!guard.contains(name)");

    const catalogProjection = registrySource
      .split("pub fn get_tools_metadata")[1]
      .split("pub fn get_all_placeholder_values")[0];
    expect(catalogProjection).toContain("self.tools");
    expect(catalogProjection).not.toContain(".filter(");
  });
});
