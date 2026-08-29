import { describe, expect, it } from "vitest";

import backendCommands from "../../../../../src-tauri/src/commands.rs?raw";
import settingsStoreSource from "../../../../core/stores/settings.ts?raw";
import vcpCoreSettingsSource from "../../../../features/settings/components/VcpCoreSettingsSection.vue?raw";
import {
  diffSettingsPatch,
  type AppSettings,
} from "@/core/stores/settings";

const rustModuleSources = import.meta.glob(
  "../../../../../src-tauri/src/vcp_modules/**/*.rs",
  { eager: true, query: "?raw", import: "default" },
) as Record<string, string>;

const baseSettings = (): AppSettings => ({
  userName: "before",
  vcpServerUrl: "",
  chatEndpointMode: "standard",
  vcpApiKey: "",
  vcpLogUrl: "",
  vcpLogKey: "",
  syncServerUrl: "",
  syncHttpUrl: "",
  syncToken: "",
  adminUsername: "",
  adminPassword: "",
  fileKey: "",
  topicSummaryModel: "model-a",
  syncLogLevel: "INFO",
  agentOrder: ["agent-a"],
  groupOrder: [],
  currentThemeMode: null,
  syncPrerenderEnabled: false,
  enableAssistant: false,
  assistantAgentId: "",
  distributedEnabled: false,
  distributedWsUrl: "",
  distributedVcpKey: "",
  distributedDeviceName: "",
});

describe("settings patch ownership", () => {
  it("submits only fields changed in the open settings editor", () => {
    const baseline = baseSettings();
    const edited = structuredClone(baseline);
    edited.userName = "after";

    expect(diffSettingsPatch(baseline, edited)).toEqual({ userName: "after" });

    const concurrentlyUpdated = {
      ...baseline,
      agentOrder: ["agent-b", "agent-a"],
    };
    expect({
      ...concurrentlyUpdated,
      ...diffSettingsPatch(baseline, edited),
    }).toMatchObject({
      userName: "after",
      agentOrder: ["agent-b", "agent-a"],
    });
  });

  it("keeps the stale full-object writer outside the production IPC surface", () => {
    expect(settingsStoreSource).not.toContain('invoke("write_settings"');
    expect(backendCommands).not.toMatch(/^\s+write_settings,\s*$/m);
  });

  it("uses the Rust endpoint preview and keeps Chat path rewriting under one owner", () => {
    expect(vcpCoreSettingsSource).toContain("preview_chat_endpoint");
    expect(vcpCoreSettingsSource).not.toContain("/v1/chat/completions");
    expect(vcpCoreSettingsSource).not.toContain("/v1/chatvcp/completions");
    expect(backendCommands).toMatch(/^\s+preview_chat_endpoint,\s*$/m);

    for (const [path, source] of Object.entries(rustModuleSources)) {
      if (path.endsWith("/infra/vcp_client.rs")) continue;
      expect(source, path).not.toContain('set_path("/v1/chat');
    }
  });
});
