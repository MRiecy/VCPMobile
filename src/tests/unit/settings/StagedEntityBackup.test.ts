import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import {
  STAGED_BACKUP_STORAGE_KEY,
  STAGED_BACKUP_TTL_MS,
  writeStagedEntityBackup,
  readStagedEntityBackup,
  applyImport,
  type ParsedConfigBackup,
} from "../../../features/settings/useConfigBackup";
import { checkAndPromptPendingConfigRestore } from "../../../core/stores/syncSession";
import { useOverlayStore } from "../../../core/stores/overlay";
import { useAssistantStore } from "../../../core/stores/assistant";
import { useSettingsStore } from "../../../core/stores/settings";
import { useNotificationStore } from "../../../core/stores/notification";

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
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("Staged Entity Backup & Two-Stage Restore Hook", () => {
  describe("checkAndPromptPendingConfigRestore", () => {
    it("returns false when no staged backup key exists", async () => {
      const result = await checkAndPromptPendingConfigRestore();
      expect(result).toBe(false);
    });

    it("cleans up corrupted JSON and returns false", async () => {
      localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, "invalid-json-content-{{");
      const result = await checkAndPromptPendingConfigRestore();
      expect(result).toBe(false);
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();
    });

    it("purges expired staged payload (> 7 days) and returns false", async () => {
      const expiredTimestamp = Date.now() - (STAGED_BACKUP_TTL_MS + 5000);
      localStorage.setItem(
        STAGED_BACKUP_STORAGE_KEY,
        JSON.stringify({
          timestamp: expiredTimestamp,
          version: 2,
          agentPreferences: { "agent-1": { mobileSystemPrompt: "expired" } },
        }),
      );

      const result = await checkAndPromptPendingConfigRestore();
      expect(result).toBe(false);
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();
    });

    it("rejects non-matching version payload", async () => {
      localStorage.setItem(
        STAGED_BACKUP_STORAGE_KEY,
        JSON.stringify({
          timestamp: Date.now(),
          version: 1, // older version
          agentPreferences: {},
        }),
      );

      const result = await checkAndPromptPendingConfigRestore();
      expect(result).toBe(false);
    });

    it("returns false without prompt when assistant store has 0 agents and 0 groups", async () => {
      writeStagedEntityBackup({
        agentPreferences: { "agent-1": { mobileSystemPrompt: "prompt" } },
      });
      const assistantStore = useAssistantStore();
      assistantStore.agents = [];
      assistantStore.groups = [];

      const overlayStore = useOverlayStore();
      const confirmSpy = vi.spyOn(overlayStore, "showConfirm");

      const result = await checkAndPromptPendingConfigRestore();
      expect(result).toBe(false);
      expect(confirmSpy).not.toHaveBeenCalled();
      // Staged key remains intact for later sync
      expect(readStagedEntityBackup()).not.toBeNull();
    });

    it("prompts user and applies staged config when entities exist and user confirms", async () => {
      const agentPrefs = {
        "agent-alpha": { mobileSystemPrompt: "已同步移动端提示词", useTemperature: true },
      };
      const groupPrefs = {
        "group-dev": { chatMode: "jev" as const },
      };
      writeStagedEntityBackup({
        agentPreferences: agentPrefs,
        groupPreferences: groupPrefs,
      });

      const assistantStore = useAssistantStore();
      assistantStore.agents = [
        { id: "agent-alpha", name: "Alpha", model: "gemini", avatarCalculatedColor: null },
      ];
      assistantStore.groups = [];

      const overlayStore = useOverlayStore();
      vi.spyOn(overlayStore, "showConfirm").mockResolvedValue(true);

      const settingsStore = useSettingsStore();
      settingsStore.snapshot = {
        domains: {} as any,
        data: {} as any,
      };
      const importSpy = vi.spyOn(settingsStore, "importBackup").mockResolvedValue({
        result: { status: "saved", snapshot: settingsStore.snapshot! },
        missingGroups: false,
        missingAgents: false,
      });

      const fetchSpy = vi.spyOn(assistantStore, "fetchAgentsAndGroups").mockResolvedValue(undefined as any);
      const notificationStore = useNotificationStore();
      const notifySpy = vi.spyOn(notificationStore, "addNotification");

      const result = await checkAndPromptPendingConfigRestore();
      expect(result).toBe(true);

      expect(overlayStore.showConfirm).toHaveBeenCalledWith(
        expect.objectContaining({
          title: "发现待生效的实体配置",
          message: expect.stringContaining("检测到您之前导入了智能体与群组的本地偏好"),
        }),
      );

      expect(importSpy).toHaveBeenCalledWith(
        expect.anything(),
        groupPrefs,
        agentPrefs,
      );

      // Verify staged key is cleaned up
      expect(readStagedEntityBackup()).toBeNull();
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();

      expect(fetchSpy).toHaveBeenCalled();
      expect(notifySpy).toHaveBeenCalledWith(
        expect.objectContaining({
          type: "success",
          title: "配置已恢复",
          toastOnly: true,
        }),
      );
    });

    it("retains staged key in localStorage when user cancels prompt", async () => {
      writeStagedEntityBackup({
        agentPreferences: { "agent-1": { mobileSystemPrompt: "pending" } },
      });

      const assistantStore = useAssistantStore();
      assistantStore.agents = [
        { id: "agent-1", name: "One", model: "gemini", avatarCalculatedColor: null },
      ];

      const overlayStore = useOverlayStore();
      vi.spyOn(overlayStore, "showConfirm").mockResolvedValue(false);

      const settingsStore = useSettingsStore();
      const importSpy = vi.spyOn(settingsStore, "importBackup");

      const result = await checkAndPromptPendingConfigRestore();
      expect(result).toBe(false);
      expect(importSpy).not.toHaveBeenCalled();

      // Key must be retained
      expect(readStagedEntityBackup()).not.toBeNull();
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).not.toBeNull();
    });
  });

  describe("applyImport Stage 1 empty-device staging", () => {
    it("stages entity preferences when import returns missing_groups or missing_agents", async () => {
      const backup: ParsedConfigBackup = {
        domains: { server: { url: "https://chat.vcp.internal" } },
        agentPreferences: {
          "agent-missing": { mobileSystemPrompt: "待同步提示词" },
        },
        groupPreferences: {
          "group-missing": { chatMode: "jev" },
        },
        exportedAt: new Date().toISOString(),
      };

      const customApply = vi.fn(async () => ({
        missingGroups: true,
        missingAgents: true,
      }));

      const notificationStore = useNotificationStore();
      const notifySpy = vi.spyOn(notificationStore, "addNotification");

      const result = await applyImport(backup, customApply);
      expect(result.applied).toContain("主服务器连接");
      expect(result.applied).toContain("群聊设置");
      expect(result.applied).toContain("智能体专属设置");

      // Verify Stage 1 written to localStorage
      const staged = readStagedEntityBackup();
      expect(staged).not.toBeNull();
      expect(staged?.version).toBe(2);
      expect(staged?.agentPreferences?.["agent-missing"].mobileSystemPrompt).toBe("待同步提示词");
      expect(staged?.groupPreferences?.["group-missing"].chatMode).toBe("jev");

      expect(notifySpy).toHaveBeenCalledWith(
        expect.objectContaining({
          type: "info",
          title: "实体配置已暂存",
        }),
      );
    });
  });
});
