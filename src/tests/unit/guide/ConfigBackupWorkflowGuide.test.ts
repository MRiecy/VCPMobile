import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { getGuide, registerTarget, unregisterTarget } from "../../../features/guide/registry";
import { configBackupWorkflowGuide } from "../../../features/guide/guides/config-backup-workflow";
import { useOverlayStore } from "../../../core/stores/overlay";

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
    expect(registered?.title).toBe("配置备份与换机恢复");
    expect(registered?.steps).toHaveLength(3);
    expect(registered?.steps[0].target).toBe("settings-config-import-action");
    expect(registered?.steps[1].target).toBe("settings-config-import-action");
    expect(registered?.steps[2].target).toBe("settings-config-import-action");
    expect(registered?.steps[0].waitTimeoutMs).toBe(6000);
    expect(registered?.steps[1].waitTimeoutMs).toBe(6000);
    expect(registered?.steps[2].waitTimeoutMs).toBe(6000);
    expect(registered?.steps[0].waitFor?.()).toBe(false);
  });

  it("calls prepare to open connection settings page", () => {
    const overlayStore = useOverlayStore();
    expect(overlayStore.isSettingsOpen).toBe(false);
    expect(overlayStore.requestedSettingsSubPage).toBeNull();

    configBackupWorkflowGuide.prepare?.();

    expect(overlayStore.isSettingsOpen).toBe(true);
    expect(overlayStore.requestedSettingsSubPage).toBe("connection");
  });

  it("evaluates connection-page-mounted predicate accurately", () => {
    const predicate = configBackupWorkflowGuide.trigger?.predicates?.find(
      (p) => p.name === "connection-page-mounted",
    );
    expect(predicate).toBeDefined();

    expect(predicate?.check()).toBe(false);

    const dummyEl = document.createElement("div");
    registerTarget("settings-config-import-action", dummyEl);
    expect(predicate?.check()).toBe(true);

    unregisterTarget("settings-config-import-action", dummyEl);
    expect(predicate?.check()).toBe(false);
  });
});
