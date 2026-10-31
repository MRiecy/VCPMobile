import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import {
  STAGED_BACKUP_STORAGE_KEY,
  writeStagedEntityBackup,
  readStagedEntityBackup,
} from "../../../features/settings/useConfigBackup";
import { checkAndPromptPendingConfigRestore } from "../../../core/stores/syncSession";
import { useOverlayStore } from "../../../core/stores/overlay";
import { useAssistantStore } from "../../../core/stores/assistant";
import { useSettingsStore } from "../../../core/stores/settings";
import { configBackupWorkflowGuide } from "../../../features/guide/guides/config-backup-workflow";

describe("Adversarial Challenge: M2 Post-Sync & Guide Flows", () => {
  beforeEach(() => {
    const storage = window.localStorage;
    vi.stubGlobal("localStorage", {
      getItem: storage.getItem.bind(storage),
      setItem: storage.setItem.bind(storage),
      removeItem: storage.removeItem.bind(storage),
      clear: storage.clear.bind(storage),
    });
    localStorage.clear();
    setActivePinia(createPinia());
    vi.spyOn(useAssistantStore(), "fetchAgentsAndGroups").mockResolvedValue(undefined as any);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  // =========================================================================
  // Challenge 1: Two-Stage Post-Sync Flow & Cancellation Persistence
  // =========================================================================
  describe("Challenge 1: Two-Stage Post-Sync Flow & Cancellation Persistence", () => {
    it("1.1 User dismisses prompt ('稍后再说' / Cancel): staging payload in localStorage is NOT deleted and remains valid", async () => {
      const agentPrefs = {
        "agent-staged": { mobileSystemPrompt: "移动端专用提示词", useTemperature: true },
      };
      const groupPrefs = {
        "group-staged": { chatMode: "jev" as const },
      };
      writeStagedEntityBackup({
        agentPreferences: agentPrefs,
        groupPreferences: groupPrefs,
      });

      const assistantStore = useAssistantStore();
      assistantStore.agents = [
        { id: "agent-staged", name: "Agent 1", model: "gemini", avatarCalculatedColor: null },
      ];
      assistantStore.groups = [];

      const overlayStore = useOverlayStore();
      const showConfirmSpy = vi.spyOn(overlayStore, "showConfirm").mockResolvedValue(false);

      const settingsStore = useSettingsStore();
      const importSpy = vi.spyOn(settingsStore, "importBackup");

      // First check (user dismisses)
      const firstResult = await checkAndPromptPendingConfigRestore();
      expect(firstResult).toBe(false);
      expect(showConfirmSpy).toHaveBeenCalledTimes(1);
      expect(importSpy).not.toHaveBeenCalled();

      // Verify payload is NOT deleted from localStorage
      const preservedRaw = localStorage.getItem(STAGED_BACKUP_STORAGE_KEY);
      expect(preservedRaw).not.toBeNull();
      const preservedData = readStagedEntityBackup();
      expect(preservedData).not.toBeNull();
      expect(preservedData?.agentPreferences?.["agent-staged"]?.mobileSystemPrompt).toBe("移动端专用提示词");
      expect(preservedData?.groupPreferences?.["group-staged"]?.chatMode).toBe("jev");

      // Subsequent check (e.g. after manual trigger or next sync) where user confirms
      showConfirmSpy.mockResolvedValueOnce(true);
      settingsStore.snapshot = { domains: {} as any, data: {} as any };
      importSpy.mockResolvedValueOnce({
        result: { status: "saved", snapshot: settingsStore.snapshot! },
        missingGroups: false,
        missingAgents: false,
      });

      const secondResult = await checkAndPromptPendingConfigRestore();
      expect(secondResult).toBe(true);
      expect(showConfirmSpy).toHaveBeenCalledTimes(2);
      expect(importSpy).toHaveBeenCalledWith(
        expect.anything(),
        groupPrefs,
        agentPrefs,
      );

      // Now it must be cleared
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();
      expect(readStagedEntityBackup()).toBeNull();
    });

    it("1.2 User syncs multiple times: verify no duplicate prompt storms or corrupted state", async () => {
      const agentPrefs = { "agent-1": { mobileSystemPrompt: "p1" } };
      writeStagedEntityBackup({ agentPreferences: agentPrefs });

      const assistantStore = useAssistantStore();
      assistantStore.agents = [
        { id: "agent-1", name: "Agent 1", model: "gemini", avatarCalculatedColor: null },
      ];
      assistantStore.groups = [];

      const overlayStore = useOverlayStore();
      const showConfirmSpy = vi.spyOn(overlayStore, "showConfirm");
      const settingsStore = useSettingsStore();
      settingsStore.snapshot = { domains: {} as any, data: {} as any };
      const importSpy = vi.spyOn(settingsStore, "importBackup").mockResolvedValue({
        result: { status: "saved", snapshot: settingsStore.snapshot! },
        missingGroups: false,
        missingAgents: false,
      });

      // Sync 1: User confirms prompt
      showConfirmSpy.mockResolvedValueOnce(true);
      const res1 = await checkAndPromptPendingConfigRestore();
      expect(res1).toBe(true);
      expect(showConfirmSpy).toHaveBeenCalledTimes(1);
      expect(importSpy).toHaveBeenCalledTimes(1);
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();

      // Sync 2: Subsequent sync finishes, checkAndPromptPendingConfigRestore is triggered
      const res2 = await checkAndPromptPendingConfigRestore();
      expect(res2).toBe(false);
      // No prompt storm! showConfirm must NOT be called again
      expect(showConfirmSpy).toHaveBeenCalledTimes(1);
      expect(importSpy).toHaveBeenCalledTimes(1);

      // Sync 3: Another sync
      const res3 = await checkAndPromptPendingConfigRestore();
      expect(res3).toBe(false);
      expect(showConfirmSpy).toHaveBeenCalledTimes(1);
      expect(importSpy).toHaveBeenCalledTimes(1);
    });

    it("1.2b User dismisses Sync 1, then Sync 2 completes, user dismisses again, then Sync 3 user accepts", async () => {
      writeStagedEntityBackup({
        agentPreferences: { "agent-2": { mobileSystemPrompt: "prompt-2" } },
      });

      const assistantStore = useAssistantStore();
      assistantStore.agents = [
        { id: "agent-2", name: "Agent 2", model: "gemini", avatarCalculatedColor: null },
      ];

      const overlayStore = useOverlayStore();
      const showConfirmSpy = vi.spyOn(overlayStore, "showConfirm");
      const settingsStore = useSettingsStore();
      settingsStore.snapshot = { domains: {} as any, data: {} as any };
      const importSpy = vi.spyOn(settingsStore, "importBackup").mockResolvedValue({
        result: { status: "saved", snapshot: settingsStore.snapshot! },
        missingGroups: false,
        missingAgents: false,
      });

      // Sync 1: Dismiss
      showConfirmSpy.mockResolvedValueOnce(false);
      expect(await checkAndPromptPendingConfigRestore()).toBe(false);
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).not.toBeNull();

      // Sync 2: Dismiss again
      showConfirmSpy.mockResolvedValueOnce(false);
      expect(await checkAndPromptPendingConfigRestore()).toBe(false);
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).not.toBeNull();

      // Sync 3: User finally accepts
      showConfirmSpy.mockResolvedValueOnce(true);
      expect(await checkAndPromptPendingConfigRestore()).toBe(true);
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();

      // Sync 4: Already accepted and cleared -> returns false without prompt
      expect(await checkAndPromptPendingConfigRestore()).toBe(false);
      expect(showConfirmSpy).toHaveBeenCalledTimes(3);
      expect(importSpy).toHaveBeenCalledTimes(1);
    });

    it("1.3 Assistant presence: checkAndPromptPendingConfigRestore does NOT prompt if agents.length === 0 && groups.length === 0", async () => {
      writeStagedEntityBackup({
        agentPreferences: { "agent-x": { mobileSystemPrompt: "test" } },
        groupPreferences: { "group-x": { chatMode: "jev" } },
      });

      const assistantStore = useAssistantStore();
      assistantStore.agents = [];
      assistantStore.groups = [];

      const overlayStore = useOverlayStore();
      const showConfirmSpy = vi.spyOn(overlayStore, "showConfirm");

      const result = await checkAndPromptPendingConfigRestore();
      expect(result).toBe(false);
      expect(showConfirmSpy).not.toHaveBeenCalled();

      // The staged payload MUST remain in localStorage, waiting for a future sync that brings assistants
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).not.toBeNull();
      expect(readStagedEntityBackup()).not.toBeNull();

      // If only group exists, it DOES prompt
      assistantStore.groups = [
        { id: "group-x", name: "Dev Group", avatarCalculatedColor: null, members: [], mode: "jev" },
      ];
      showConfirmSpy.mockResolvedValueOnce(false);
      expect(await checkAndPromptPendingConfigRestore()).toBe(false);
      expect(showConfirmSpy).toHaveBeenCalledTimes(1);

      // Reset groups, add only agent -> it DOES prompt
      assistantStore.groups = [];
      assistantStore.agents = [
        { id: "agent-x", name: "Agent X", model: "gemini", avatarCalculatedColor: null },
      ];
      showConfirmSpy.mockResolvedValueOnce(false);
      expect(await checkAndPromptPendingConfigRestore()).toBe(false);
      expect(showConfirmSpy).toHaveBeenCalledTimes(2);
    });

    it("1.4 Corrupted payload variations & boundary conditions", async () => {
      const overlayStore = useOverlayStore();
      const showConfirmSpy = vi.spyOn(overlayStore, "showConfirm");

      const assistantStore = useAssistantStore();
      assistantStore.agents = [
        { id: "agent-1", name: "Agent 1", model: "gemini", avatarCalculatedColor: null },
      ];

      // Non-object JSON: string
      localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, JSON.stringify("hello"));
      expect(await checkAndPromptPendingConfigRestore()).toBe(false);
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();

      // Non-object JSON: array
      localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, JSON.stringify([1, 2, 3]));
      expect(await checkAndPromptPendingConfigRestore()).toBe(false);
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();

      // Non-object JSON: number
      localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, JSON.stringify(12345));
      expect(await checkAndPromptPendingConfigRestore()).toBe(false);
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();

      // Future timestamp beyond negative skew
      localStorage.setItem(
        STAGED_BACKUP_STORAGE_KEY,
        JSON.stringify({
          timestamp: Date.now() + 100000,
          version: 2,
        }),
      );
      expect(await checkAndPromptPendingConfigRestore()).toBe(false);
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();

      // Missing version (undefined)
      localStorage.setItem(
        STAGED_BACKUP_STORAGE_KEY,
        JSON.stringify({
          timestamp: Date.now(),
        }),
      );
      expect(await checkAndPromptPendingConfigRestore()).toBe(false);

      expect(showConfirmSpy).not.toHaveBeenCalled();
    });
  });

  // =========================================================================
  // Challenge 2: Guide Flow Predicates & Trigger Evaluation
  // =========================================================================
  describe("Challenge 2: Guide Flow Predicates & Trigger Evaluation", () => {
    it("2.1 config-backup-workflow predicates: only activates if server URL is configured and user has no assistants yet", () => {
      const guide = configBackupWorkflowGuide;
      expect(guide.id).toBe("config-backup-workflow");
      expect(guide.trigger?.predicates).toBeDefined();

      const hasServerUrlPred = guide.trigger!.predicates!.find((p) => p.name === "hasServerUrl");
      const noAssistantsYetPred = guide.trigger!.predicates!.find((p) => p.name === "noAssistantsYet");
      expect(hasServerUrlPred).toBeDefined();
      expect(noAssistantsYetPred).toBeDefined();

      const settingsStore = useSettingsStore();
      const assistantStore = useAssistantStore();

      // Matrix of 4 states:
      // State 1: Server URL empty, Assistants 0 -> should NOT activate (hasServerUrl: false, noAssistants: true)
      settingsStore.snapshot = null;
      assistantStore.agents = [];
      assistantStore.groups = [];
      expect(hasServerUrlPred!.check()).toBe(false);
      expect(noAssistantsYetPred!.check()).toBe(true);

      // State 2: Server URL empty, Assistants > 0 -> should NOT activate (hasServerUrl: false, noAssistants: false)
      assistantStore.agents = [
        { id: "agent-1", name: "Agent 1", model: "gemini", avatarCalculatedColor: null },
      ];
      expect(hasServerUrlPred!.check()).toBe(false);
      expect(noAssistantsYetPred!.check()).toBe(false);

      // State 3: Server URL configured, Assistants > 0 -> should NOT activate (hasServerUrl: true, noAssistants: false)
      settingsStore.snapshot = {
        domains: {} as any,
        data: { server: { url: "https://chat.vcp.internal:8443" } } as any,
      };
      expect(hasServerUrlPred!.check()).toBe(true);
      expect(noAssistantsYetPred!.check()).toBe(false);

      // State 4: Server URL configured, Assistants 0 -> ACTIVATES! (hasServerUrl: true, noAssistants: true)
      assistantStore.agents = [];
      assistantStore.groups = [];
      expect(hasServerUrlPred!.check()).toBe(true);
      expect(noAssistantsYetPred!.check()).toBe(true);
    });

    it("2.2 Guide predicate robustness with whitespace, partial fields and store teardown", () => {
      const guide = configBackupWorkflowGuide;
      const hasServerUrlPred = guide.trigger!.predicates!.find((p) => p.name === "hasServerUrl")!;
      const noAssistantsYetPred = guide.trigger!.predicates!.find((p) => p.name === "noAssistantsYet")!;

      const settingsStore = useSettingsStore();
      const assistantStore = useAssistantStore();

      // Empty string URL
      settingsStore.snapshot = {
        domains: {} as any,
        data: { server: { url: "" } } as any,
      };
      expect(hasServerUrlPred.check()).toBe(false);

      // Server domain missing entirely
      settingsStore.snapshot = {
        domains: {} as any,
        data: {} as any,
      };
      expect(hasServerUrlPred.check()).toBe(false);

      // Only groups present, 0 agents
      assistantStore.agents = [];
      assistantStore.groups = [
        { id: "group-1", name: "G1", avatarCalculatedColor: null, members: [], mode: "sequential" },
      ];
      expect(noAssistantsYetPred.check()).toBe(false);

      // Both agents and groups empty
      assistantStore.groups = [];
      expect(noAssistantsYetPred.check()).toBe(true);
    });
  });
});
