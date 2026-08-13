import { describe, expect, it } from "vitest";
import distributedViewSource from "@/features/distributed/DistributedView.vue?raw";
import tauriLibSource from "../../../../src-tauri/src/lib.rs?raw";
import distributedCommandsSource from "../../../../src-tauri/src/distributed/mod.rs?raw";
import registrySource from "../../../../src-tauri/src/distributed/tool_registry.rs?raw";

describe("distributed tool authorization contract", () => {
  it("submits an explicit enabled allowlist and never reconstructs disabled complements", () => {
    expect(distributedViewSource).toContain(
      'await invoke("update_enabled_tools", { enabledNames: currentEnabled })',
    );
    expect(distributedViewSource).toContain(".filter(p => p.enabled)");
    expect(distributedViewSource).toContain("enabled: tool.enabled === true");
    expect(distributedViewSource).not.toContain("update_disabled_tools");
    expect(distributedViewSource).not.toContain("disabledNames");
  });

  it("registers only the enabled-name command and keeps new tools fail-closed", () => {
    const productionLib = tauriLibSource.split("#[cfg(test)]")[0];
    expect(productionLib).toContain("distributed::update_enabled_tools,");
    expect(productionLib).not.toContain("distributed::update_disabled_tools,");
    expect(distributedCommandsSource).toContain("enabled_names: Vec<String>");
    expect(registrySource).toContain("enabled_names: RwLock<HashSet<String>>");
    expect(registrySource).toContain("guard.contains(name)");
    expect(registrySource).not.toContain("!guard.contains(name)");
  });
});
