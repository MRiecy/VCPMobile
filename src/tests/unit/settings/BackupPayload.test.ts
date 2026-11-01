import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, disposePinia, getActivePinia, setActivePinia } from "pinia";
import {
  CONFIG_BACKUP_KIND,
  buildBackupPayload,
  applyConfigBackup,
  captureBackupPreferences,
  type BackupDomains,
} from "../../../features/settings/useConfigBackup";
import { useThemeStore } from "../../../core/stores/theme";
import { useChatPreferencesStore } from "../../../features/chat/preferences/store";
import { useTopicStore, type Topic } from "../../../core/stores/topicListManager";
import { useDiaryStore } from "../../../features/diary/diaryStore";
import type { GroupLocalSettings } from "../../../core/types/assistant";

function makeSampleGroupSettings(): GroupLocalSettings {
  return {
    chatMode: "jev",
    modeSettings: {
      sequential: { speakerOrder: ["agent-alpha", "agent-beta"] },
      jev: {
        navigatorPrompt: "选择最匹配的智能体",
        speakerThreshold: 0.65,
        continueThreshold: 0.75,
        stopThreshold: 0.3,
        maxSpeakersPerRound: 3,
        maxAutonomousRounds: 5,
        historyWindow: 30,
        continueDebounceMs: 800,
        fallbackPolicy: "top_speaker",
        memberStyles: {},
      },
    },
    enableContextMessageWindow: true,
    contextMessageWindowSize: 60,
  };
}

beforeEach(() => {
  const storage = window.localStorage;
  vi.stubGlobal("localStorage", {
    getItem: storage.getItem.bind(storage),
    setItem: storage.setItem.bind(storage),
    removeItem: storage.removeItem.bind(storage),
    clear: storage.clear.bind(storage),
  });
  localStorage.clear();
  document.documentElement.style.cssText = "";
  document.documentElement.className = "";
  setActivePinia(createPinia());
});

afterEach(() => {
  const pinia = getActivePinia();
  if (pinia) disposePinia(pinia);
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function makeSampleDomains(): BackupDomains {
  return {
    identity: { userName: "指挥官" },
    server: { url: "https://chat.vcp.internal:8443", apiKey: "sk-vcp-prod-secret" },
    log: { url: "https://log.vcp.internal:9000", key: "log-key" },
    sync: { token: "sync-token-xyz123" },
    navigation: {
      agentOrder: ["agent-001", "agent-002"],
      groupOrder: ["group-dev", "group-life"],
    },
    voice: {
      asr: { stream: true, inputEncoding: "mp3_64", language: "zh" },
      tts: { mode: "builtin", voice: "Mia", stream: true },
      vad: { endSilenceMs: 600 },
      guidance: { rules: "语音交互时尽量简短回复" },
    },
    jev: { timeoutMs: 15000, maxRetries: 2 },
  };
}

describe("Backup payload and application contracts", () => {
  describe("exported payload", () => {
    it("exports the versioned envelope, navigation domain and supplied timestamp", () => {
      const domains = makeSampleDomains();
      const prefs = captureBackupPreferences();
      const payload = buildBackupPayload(domains, prefs, new Date("2026-10-31T08:30:00.000Z"));
      expect(payload.exportedAt).toBe("2026-10-31T08:30:00.000Z");

      expect(payload.app).toBe("vcp-mobile");
      expect(payload.kind).toBe(CONFIG_BACKUP_KIND);
      expect(payload.version).toBe(2);
      expect(payload.domains.navigation).toBeDefined();
      expect(payload.domains.navigation?.agentOrder).toEqual(["agent-001", "agent-002"]);
      expect(payload.domains.navigation?.groupOrder).toEqual(["group-dev", "group-life"]);
    });

    it("captures the current Theme, Chat, Topic, and Diary preferences", () => {
      const theme = useThemeStore();
      theme.mode = "dark";
      const chat = useChatPreferencesStore();
      chat.setPresentationMode("panel");
      chat.setMessageHapticsEnabled(true);
      chat.setSmoothStreamingEnabled(false);
      chat.setCollapseToolsWhileStreaming(true);

      const topicStore = useTopicStore();
      topicStore.sortMode = "updated";
      topicStore.pinnedTopicKeys = [
        JSON.stringify(["agent", "agent-001", "topic-101"]),
        JSON.stringify(["group", "group-dev", "topic-202"]),
      ];

      const diaryStore = useDiaryStore();
      diaryStore.hiddenFolders = ["archive", "drafts"];
      diaryStore.folderOrder = ["work", "personal"];
      diaryStore.collapsedCategories = ["cluster"];

      const capturedPrefs = captureBackupPreferences();
      expect(capturedPrefs.theme.mode).toBe("dark");
      expect(capturedPrefs.chat.presentationMode).toBe("panel");
      expect(capturedPrefs.chat.messageHapticsEnabled).toBe(true);
      expect(capturedPrefs.chat.smoothStreamingEnabled).toBe(false);
      expect(capturedPrefs.chat.collapseToolsWhileStreaming).toBe(true);

      expect(capturedPrefs.topic).toEqual({ sortMode: "updated", pinnedTopicKeys: topicStore.pinnedTopicKeys });
      expect(capturedPrefs.diary).toEqual({ hiddenFolders: ["archive", "drafts"], folderOrder: ["work", "personal"], collapsedCategories: ["cluster"] });
    });

    it("excludes volatile stores and diagnostic keys from backup", () => {
      // Simulate presence of volatile keys in localStorage
      localStorage.setItem("vcp-developer-mode", "true");
      localStorage.setItem("vcp_last_update_check", "1761800000000");
      localStorage.setItem("vcp_notification_intro_v1", "seen");
      localStorage.setItem("guide", JSON.stringify({ completed: ["welcome-guide", "tour-guide"] }));
      localStorage.setItem("chatSession", JSON.stringify({ currentSelectedItem: "agent-001", currentTopicId: "t-1" }));

      const payload = buildBackupPayload(makeSampleDomains(), captureBackupPreferences());
      const payloadString = JSON.stringify(payload);

      expect(payloadString).not.toContain("vcp-developer-mode");
      expect(payloadString).not.toContain("1761800000000");
      expect(payloadString).not.toContain("welcome-guide");
      expect(payloadString).not.toContain("currentSelectedItem");
      expect(payloadString).not.toContain("vcp_notification_intro_v1");
    });
  });

  describe("Partial backup application", () => {
    it("reorders already loaded topics when restoring the sort preference", async () => {
      const store = useTopicStore();
      const older: Topic = {
        id: "older", name: "Older", ownerId: "agent-alpha", ownerType: "agent",
        createdAt: 1, updatedAt: 3, locked: false, unread: false, unreadCount: 0, msgCount: 0,
      };
      const newer: Topic = { ...older, id: "newer", name: "Newer", createdAt: 2, updatedAt: 2 };
      store.topics = [newer, older];

      const result = await applyConfigBackup({
        domains: {}, exportedAt: "", preferences: { topic: { sortMode: "updated" } },
      }, vi.fn());

      expect(result.failed).toEqual([]);
      expect(store.topicSections.regular.map(topic => topic.id)).toEqual(["older", "newer"]);
      await applyConfigBackup({
        domains: {}, exportedAt: "", preferences: { topic: { sortMode: "created" } },
      }, vi.fn());
      expect(store.topicSections.regular.map(topic => topic.id)).toEqual(["newer", "older"]);
    });

    it("passes only provided voice fields to the settings owner", async () => {
      const applySettings = vi.fn(async () => ({ missingGroups: false, missingAgents: false }));
      const backup = {
        domains: { voice: { asr: { stream: true } } },
        exportedAt: new Date().toISOString(),
      };
      const result = await applyConfigBackup(backup, applySettings);
      expect(applySettings).toHaveBeenCalledExactlyOnceWith({ 'voice.asr.stream': true }, {}, {}, undefined);
      expect(result.applied).toContain("语音服务");
      expect(result.failed).toHaveLength(0);
    });
  });

  describe("combined domains", () => {
    it("exports navigation, connection and group settings together", () => {
      const domains = makeSampleDomains();
      domains.navigation = {
        agentOrder: ["agent-alpha", "agent-beta"],
        groupOrder: ["group-primary"],
      };

      const groupPreferences: Record<string, GroupLocalSettings> = {
        "group-primary": makeSampleGroupSettings(),
      };

      const payload = buildBackupPayload(
        domains,
        captureBackupPreferences(),
        new Date(),
        groupPreferences,
      );

      // Verify composite payload holds all domains and entity configurations
      expect(payload.domains.navigation?.agentOrder).toEqual(["agent-alpha", "agent-beta"]);
      expect(payload.groupPreferences).toEqual(groupPreferences);
      expect(payload.domains.server?.url).toBe("https://chat.vcp.internal:8443");
    });

    it("reports missing groups independently from the applied settings", async () => {
      const applySettings = vi.fn(async (_changes, groupPrefs) => {
        // Simulates that group-dev is missing in local DB
        const hasMissing = Object.keys(groupPrefs).includes("group-dev");
        return { missingGroups: hasMissing, missingAgents: false };
      });

      const backup = {
        domains: {
          server: { url: "https://chat.vcp.internal" },
          navigation: { agentOrder: ["a-1"], groupOrder: ["group-dev"] },
        },
        groupPreferences: {
          "group-dev": { chatMode: "jev" as const, modeSettings: {} },
        },
        exportedAt: "",
      };

      const result = await applyConfigBackup(backup, applySettings);
      expect(result.applied).toContain("主服务器连接");
      expect(result.applied).toContain("群聊设置");
      expect(result.skipped).toContain("部分群聊尚未同步到本机，请先同步，再次导入");
    });
  });
});
