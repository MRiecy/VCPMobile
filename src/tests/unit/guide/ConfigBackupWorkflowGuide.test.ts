import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { getGuide } from "../../../features/guide/registry";
import { configBackupWorkflowGuide } from "../../../features/guide/guides/config-backup-workflow";
import { useSettingsStore } from "../../../core/stores/settings";
import { useAssistantStore } from "../../../core/stores/assistant";

describe("Config Backup Workflow Guide Definition & Predicates", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("is registered in guideRegistry with expected metadata and steps", () => {
    const registered = getGuide("config-backup-workflow");
    expect(registered).toBeDefined();
    expect(registered?.id).toBe("config-backup-workflow");
    expect(registered?.title).toBe("配置全量备份与两阶段恢复");
    expect(registered?.steps).toHaveLength(3);
    expect(registered?.steps[0].target).toBe("settings-config-backup-section");
    expect(registered?.steps[1].target).toBe("sidebar-sync-button");
    expect(registered?.steps[2].target).toBeNull();
  });

  it("evaluates hasServerUrl predicate accurately", () => {
    const settingsStore = useSettingsStore();
    const predicate = configBackupWorkflowGuide.trigger?.predicates?.find(
      (p) => p.name === "hasServerUrl",
    );
    expect(predicate).toBeDefined();

    expect(predicate?.check()).toBe(false);

    settingsStore.snapshot = {
      domains: {} as any,
      data: {
        server: { url: "https://chat.vcp.internal" },
      } as any,
    };
    expect(predicate?.check()).toBe(true);

    settingsStore.snapshot = {
      domains: {} as any,
      data: {
        server: { url: "" },
      } as any,
    };
    expect(predicate?.check()).toBe(false);
  });

  it("evaluates noAssistantsYet predicate accurately", () => {
    const assistantStore = useAssistantStore();
    const predicate = configBackupWorkflowGuide.trigger?.predicates?.find(
      (p) => p.name === "noAssistantsYet",
    );
    expect(predicate).toBeDefined();

    assistantStore.agents = [];
    assistantStore.groups = [];
    expect(predicate?.check()).toBe(true);

    assistantStore.agents = [{ id: "agent-1", name: "AI", model: "gemini", avatarCalculatedColor: null }];
    expect(predicate?.check()).toBe(false);

    assistantStore.agents = [];
    assistantStore.groups = [{ id: "group-1", name: "Dev", avatarCalculatedColor: null, members: [], mode: "jev" }];
    expect(predicate?.check()).toBe(false);
  });
});
