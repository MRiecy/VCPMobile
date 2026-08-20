import { describe, expect, it } from "vitest";

import backendCommands from "../../../../../src-tauri/src/commands.rs?raw";
import settingsStoreSource from "../../../../core/stores/settings.ts?raw";
import {
  diffSettingsPatch,
  type AppSettings,
} from "@/core/stores/settings";

const baseSettings = (): AppSettings => ({
  userName: "before",
  vcpServerUrl: "",
  vcpApiKey: "",
  vcpLogUrl: "",
  vcpLogKey: "",
  syncServerUrl: "",
  syncHttpUrl: "",
  syncToken: "",
  topicSummaryModel: "model-a",
  syncLogLevel: "INFO",
  agentOrder: ["agent-a"],
  groupOrder: [],
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
});
