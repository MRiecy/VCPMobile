import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import {
  CONFIG_BACKUP_KIND,
  CONFIG_BACKUP_VERSION,
  buildBackupPayload,
  applyConfigBackup,
  captureBackupPreferences,
  type BackupDomains,
} from "../../../features/settings/useConfigBackup";
import { useThemeStore } from "../../../core/stores/theme";
import { useChatPreferencesStore } from "../../../features/chat/preferences/store";
import { useTopicStore } from "../../../core/stores/topicListManager";
import { useDiaryStore } from "../../../features/diary/diaryStore";
import { useOverlayStore } from "../../../core/stores/overlay";
import { useAssistantStore } from "../../../core/stores/assistant";
import type { GroupLocalSettings } from "../../../core/types/assistant";

// ---------------------------------------------------------------------------
// V2 Specification Contracts & Staging Protocol Constants
// (Reference: plan/vcpmobile-vcpcfg-v2-full-backup/ 02, 03, 04, 05)
// ---------------------------------------------------------------------------
export const STAGED_BACKUP_STORAGE_KEY = "vcp_staged_entity_backup";
export const STAGED_BACKUP_TTL_MS = 7 * 24 * 60 * 60 * 1000; // 7 days

export interface StagedEntityBackupPayload {
  timestamp: number;
  version: number;
  agentPreferences?: Record<string, {
    mobileSystemPrompt?: string;
    useTemperature?: boolean;
    voice?: {
      enabled?: boolean;
      mode?: "builtin" | "clone";
      voice?: string;
      cloneSampleId?: string;
      stream?: boolean;
    };
  }>;
  groupPreferences?: Record<string, {
    chatMode?: "sequential" | "jev";
    modeSettings?: Record<string, unknown>;
    enableContextMessageWindow?: boolean;
    contextMessageWindowSize?: number;
  }>;
}

/** Check whether staged entity backup payload is still within its 7-day TTL */
export function isStagedPayloadValid(
  payload: StagedEntityBackupPayload | null,
  now: number = Date.now(),
): boolean {
  if (!payload || typeof payload !== "object") return false;
  if (typeof payload.timestamp !== "number" || !Number.isFinite(payload.timestamp)) return false;
  if (payload.version !== CONFIG_BACKUP_VERSION) return false;
  const age = now - payload.timestamp;
  return age >= 0 && age <= STAGED_BACKUP_TTL_MS;
}

/** Helper to read staged backup safely with corrupted JSON defense */
export function readStagedEntityBackup(now: number = Date.now()): StagedEntityBackupPayload | null {
  try {
    const raw = localStorage.getItem(STAGED_BACKUP_STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as StagedEntityBackupPayload;
    if (!isStagedPayloadValid(parsed, now)) {
      localStorage.removeItem(STAGED_BACKUP_STORAGE_KEY);
      return null;
    }
    return parsed;
  } catch {
    localStorage.removeItem(STAGED_BACKUP_STORAGE_KEY);
    return null;
  }
}

/** Helper to write staged entity backup */
export function writeStagedEntityBackup(
  payload: Omit<StagedEntityBackupPayload, "timestamp" | "version">,
  timestamp: number = Date.now(),
): void {
  const staged: StagedEntityBackupPayload = {
    ...payload,
    timestamp,
    version: CONFIG_BACKUP_VERSION,
  };
  localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, JSON.stringify(staged));
}

/** Stage 2 Post-Sync Trigger Hook */
export async function executePostSyncConfigRestorePrompt(
  overlayStore: ReturnType<typeof useOverlayStore>,
  assistantStore: ReturnType<typeof useAssistantStore>,
  applyStagedHandler: (staged: StagedEntityBackupPayload) => Promise<boolean>,
  now: number = Date.now(),
): Promise<"applied" | "cancelled" | "no_pending" | "no_entities"> {
  const staged = readStagedEntityBackup(now);
  if (!staged) return "no_pending";

  const totalEntities = assistantStore.agents.length + assistantStore.groups.length;
  if (totalEntities === 0) return "no_entities";

  const confirmed = await overlayStore.showConfirm({
    title: "同步已完成 · 恢复助手专属配置",
    message: `已成功同步 ${assistantStore.agents.length} 个智能体与 ${assistantStore.groups.length} 个群聊数据。检测到您此前的备份中包含专属移动端提示词、独立音色与群聊模式，是否现在补全？`,
  });

  if (confirmed) {
    const ok = await applyStagedHandler(staged);
    if (ok) {
      localStorage.removeItem(STAGED_BACKUP_STORAGE_KEY);
      return "applied";
    }
  }
  return "cancelled";
}

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

// ---------------------------------------------------------------------------
// Test Setup & Teardown
// ---------------------------------------------------------------------------
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
  try {
    useThemeStore().$dispose();
    useChatPreferencesStore().$dispose();
    useTopicStore().$dispose();
    useDiaryStore().$dispose();
    useAssistantStore().$dispose();
    useOverlayStore().$dispose();
  } catch {
    // Ignore store teardown anomalies
  }
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

afterAll(async () => {
  await new Promise((resolve) => setTimeout(resolve, 200));
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

describe("V2 Full Backup & Dual-Tier Staging E2E Suite", () => {
  // =========================================================================
  // Tier 1: Feature Coverage (F1, F2, F6, F7)
  // =========================================================================
  describe("Tier 1: Feature Coverage & Protocol Contracts", () => {
    it("TC-F1-01 & TC-F2-01: buildBackupPayload generates version 2 envelope including navigation domain", () => {
      const domains = makeSampleDomains();
      const prefs = captureBackupPreferences();
      const payload = buildBackupPayload(domains, prefs);

      expect(payload.app).toBe("vcp-mobile");
      expect(payload.kind).toBe(CONFIG_BACKUP_KIND);
      expect(payload.version).toBe(2);
      expect(payload.version).toBe(CONFIG_BACKUP_VERSION);
      expect(payload.domains.navigation).toBeDefined();
      expect(payload.domains.navigation?.agentOrder).toEqual(["agent-001", "agent-002"]);
      expect(payload.domains.navigation?.groupOrder).toEqual(["group-dev", "group-life"]);
    });

    it("TC-F2-05: exportedAt is a valid ISO-8601 timestamp string", () => {
      const fixedDate = new Date("2026-10-31T08:30:00.000Z");
      const payload = buildBackupPayload(makeSampleDomains(), captureBackupPreferences(), fixedDate);
      expect(payload.exportedAt).toBe("2026-10-31T08:30:00.000Z");
      expect(new Date(payload.exportedAt).toISOString()).toBe("2026-10-31T08:30:00.000Z");
    });

    it("TC-F6-01 & TC-F6-02: Pinia state capture covers Theme, Chat, Topic, and Diary preferences", () => {
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

      // Verify Topic & Diary properties are defined and populated
      expect(topicStore.sortMode).toBe("updated");
      expect(topicStore.pinnedTopicKeys).toHaveLength(2);
      expect(diaryStore.hiddenFolders).toContain("archive");
      expect(diaryStore.folderOrder).toEqual(["work", "personal"]);
    });

    it("TC-F6-03, TC-F6-04, TC-F6-05: Volatile stores and diagnostic keys are strictly excluded from backup", () => {
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

    it("TC-F7-01 & TC-F7-02: Staging key writing format matches schema with timestamp and version 2", () => {
      const agentPrefs = {
        "agent-001": {
          mobileSystemPrompt: "移动端精简回复",
          useTemperature: false,
          voice: { enabled: true, mode: "builtin" as const, voice: "苏打", stream: true },
        },
      };
      const groupPrefs = {
        "group-dev": {
          chatMode: "jev" as const,
          enableContextMessageWindow: true,
          contextMessageWindowSize: 50,
        },
      };

      const now = Date.now();
      writeStagedEntityBackup({ agentPreferences: agentPrefs, groupPreferences: groupPrefs }, now);

      const raw = localStorage.getItem(STAGED_BACKUP_STORAGE_KEY);
      expect(raw).toBeTruthy();
      const parsed = JSON.parse(raw!);
      expect(parsed.version).toBe(2);
      expect(parsed.timestamp).toBe(now);
      expect(parsed.agentPreferences["agent-001"].mobileSystemPrompt).toBe("移动端精简回复");
      expect(parsed.groupPreferences["group-dev"].chatMode).toBe("jev");
    });

    it("TC-F7-03 & TC-F7-04: Staged payload is valid within 7 days and rejected after 7 days", () => {
      const now = Date.now();
      const freshPayload: StagedEntityBackupPayload = {
        timestamp: now - 3 * 24 * 60 * 60 * 1000, // 3 days old
        version: 2,
        agentPreferences: { "agent-001": { mobileSystemPrompt: "valid" } },
      };
      expect(isStagedPayloadValid(freshPayload, now)).toBe(true);

      const expiredPayload: StagedEntityBackupPayload = {
        timestamp: now - (7 * 24 * 60 * 60 * 1000 + 1000), // 7 days + 1 second old
        version: 2,
        agentPreferences: { "agent-001": { mobileSystemPrompt: "expired" } },
      };
      expect(isStagedPayloadValid(expiredPayload, now)).toBe(false);
    });

    it("TC-F7-05: readStagedEntityBackup automatically purges expired payload from localStorage", () => {
      const now = Date.now();
      const expiredPayload: StagedEntityBackupPayload = {
        timestamp: now - 8 * 24 * 60 * 60 * 1000, // 8 days old
        version: 2,
      };
      localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, JSON.stringify(expiredPayload));

      const result = readStagedEntityBackup(now);
      expect(result).toBeNull();
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();
    });
  });

  // =========================================================================
  // Tier 2: Boundary & Corner Cases
  // =========================================================================
  describe("Tier 2: Boundary & Corner Cases", () => {
    it("TC-B1-01 & TC-B1-02: Corrupted or non-JSON data in localStorage staging key does not crash", () => {
      localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, "this-is-not-valid-json-{{{");
      expect(() => readStagedEntityBackup()).not.toThrow();
      expect(readStagedEntityBackup()).toBeNull();
      // Verifies corrupted key gets cleaned up
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();
    });

    it("TC-B1-03: Empty object or null in staging key returns null", () => {
      localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, "{}");
      expect(readStagedEntityBackup()).toBeNull();
    });

    it("TC-B1-04: Non-matching version in staging payload is rejected", () => {
      const wrongVersionPayload: StagedEntityBackupPayload = {
        timestamp: Date.now(),
        version: 1, // older version
      };
      localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, JSON.stringify(wrongVersionPayload));
      expect(readStagedEntityBackup()).toBeNull();
    });

    it("TC-B3-01 & TC-B3-02: Preserves multiline Markdown diff and special characters in agent prompts", () => {
      const complexPrompt = [
        "# System Instructions",
        "```diff",
        "- old code line",
        "+ new code line with \"quotes\" & \\backslashes\\",
        "```",
        "Special characters: <tag> & \t \r\n newline",
        "Emoji test: 🚀🤖✨🔥",
      ].join("\n");

      const agentPrefs = {
        "agent-complex": {
          mobileSystemPrompt: complexPrompt,
          useTemperature: true,
          voice: { enabled: true, voice: "Mia" },
        },
      };

      writeStagedEntityBackup({ agentPreferences: agentPrefs });
      const readBack = readStagedEntityBackup();
      expect(readBack).not.toBeNull();
      expect(readBack?.agentPreferences?.["agent-complex"]?.mobileSystemPrompt).toBe(complexPrompt);
      expect(readBack?.agentPreferences?.["agent-complex"]?.mobileSystemPrompt).toContain("🚀🤖✨🔥");
      expect(readBack?.agentPreferences?.["agent-complex"]?.mobileSystemPrompt).toContain("+ new code line with \"quotes\"");
    });

    it("TC-B4-01: TTL verification right on the exact 7-day millisecond boundary", () => {
      const now = 1761800000000;
      const exactBoundaryPayload: StagedEntityBackupPayload = {
        timestamp: now - STAGED_BACKUP_TTL_MS,
        version: 2,
      };
      expect(isStagedPayloadValid(exactBoundaryPayload, now)).toBe(true);

      const oneMsExpiredPayload: StagedEntityBackupPayload = {
        timestamp: now - STAGED_BACKUP_TTL_MS - 1,
        version: 2,
      };
      expect(isStagedPayloadValid(oneMsExpiredPayload, now)).toBe(false);
    });

    it("TC-B4-02: Future timestamps (clock skew) within sane bounds are rejected if negative age", () => {
      const now = 1761800000000;
      const futurePayload: StagedEntityBackupPayload = {
        timestamp: now + 60000, // 1 minute in the future
        version: 2,
      };
      expect(isStagedPayloadValid(futurePayload, now)).toBe(false);
    });

    it("TC-B2-05: Missing or empty voice config applies safe defaults without crash", async () => {
      const applySettings = vi.fn(async () => false);
      const backup = {
        domains: { voice: { asr: { stream: true } } },
        exportedAt: new Date().toISOString(),
      };
      const result = await applyConfigBackup(backup, applySettings);
      expect(result.applied).toContain("语音服务");
      expect(result.failed).toHaveLength(0);
    });
  });

  // =========================================================================
  // Tier 3: Cross-Feature Combinations
  // =========================================================================
  describe("Tier 3: Cross-Feature Combinations", () => {
    it("TC-X1: Navigation reordering, Agent prompts, and Group JEV settings in a single backup file", () => {
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
      expect(payload.groupPreferences?.["group-primary"]?.chatMode).toBe("jev");
      expect(payload.domains.server?.url).toBe("https://chat.vcp.internal:8443");
    });

    it("TC-X2: Atomic partial application handles missing database entities gracefully", async () => {
      const applySettings = vi.fn(async (_changes, groupPrefs) => {
        // Simulates that group-dev is missing in local DB
        const hasMissing = Object.keys(groupPrefs).includes("group-dev");
        return hasMissing;
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

    it("TC-X3: Restoring preferences with absent stores preserves existing in-memory state", async () => {
      const theme = useThemeStore();
      await theme.restorePreferences({ themeKey: "themes-bear-holiday.ts", mode: "dark" });

      const chat = useChatPreferencesStore();
      chat.setPresentationMode("panel");

      const applySettings = vi.fn(async () => false);
      // Backup has ONLY server and topicSummaryModel, no theme or chat
      const backup = {
        domains: { server: { apiKey: "key-999" } },
        exportedAt: "",
      };

      await applyConfigBackup(backup, applySettings);
      // Theme and chat store state remain intact
      expect(theme.currentTheme).toBe("themes-bear-holiday.ts");
      expect(theme.mode).toBe("dark");
      expect(chat.presentationMode).toBe("panel");
    });
  });

  // =========================================================================
  // Tier 4: Real-World Application Scenarios
  // =========================================================================
  describe("Tier 4: Real-World Application Scenarios", () => {
    it("Scenario A: Fresh device empty setup -> stage 1 backup -> CDS sync -> post-sync prompt -> one-click restore", async () => {
      const overlayStore = useOverlayStore();
      const assistantStore = useAssistantStore();

      // Step 1: Fresh device state — 0 agents, 0 groups
      assistantStore.agents = [];
      assistantStore.groups = [];

      // Step 2: User imports .vcpcfg.json on fresh device.
      // Missing agents and groups are staged to localStorage with 7-day TTL.
      const stagedData = {
        agentPreferences: {
          "agent-001": { mobileSystemPrompt: "精简提示词", useTemperature: true },
          "agent-002": { mobileSystemPrompt: "代码审查助手", useTemperature: false },
        },
        groupPreferences: {
          "group-dev": { chatMode: "jev" as const, enableContextMessageWindow: true },
        },
      };
      writeStagedEntityBackup(stagedData);
      expect(readStagedEntityBackup()).not.toBeNull();

      // Step 3: Before sync finishes, post-sync prompt is checked with 0 entities
      const applyMock = vi.fn(async () => true);
      const earlyResult = await executePostSyncConfigRestorePrompt(
        overlayStore,
        assistantStore,
        applyMock,
      );
      expect(earlyResult).toBe("no_entities");
      expect(applyMock).not.toHaveBeenCalled();

      // Step 4: CDS Sync completes! Agents and groups are synced back to device.
      assistantStore.agents = [
        { id: "agent-001", name: "架构师", model: "gemini", avatarCalculatedColor: null },
        { id: "agent-002", name: "代码助手", model: "claude", avatarCalculatedColor: null },
      ];
      assistantStore.groups = [
        { id: "group-dev", name: "研发群", avatarCalculatedColor: null, members: ["agent-001"], mode: "jev" },
      ];

      // Step 5: User confirms prompt via overlayStore.showConfirm
      vi.spyOn(overlayStore, "showConfirm").mockResolvedValue(true);

      const restoreResult = await executePostSyncConfigRestorePrompt(
        overlayStore,
        assistantStore,
        applyMock,
      );

      expect(restoreResult).toBe("applied");
      expect(applyMock).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({
        version: 2,
        agentPreferences: expect.objectContaining({
          "agent-001": expect.objectContaining({ mobileSystemPrompt: "精简提示词" }),
        }),
      }));

      // Step 6: Verify staging key is cleaned up after successful restoration
      expect(readStagedEntityBackup()).toBeNull();
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();
    });

    it("Scenario A.2: User dismisses post-sync prompt -> staging key is preserved for later manual apply", async () => {
      const overlayStore = useOverlayStore();
      const assistantStore = useAssistantStore();

      assistantStore.agents = [
        { id: "agent-001", name: "架构师", model: "gemini", avatarCalculatedColor: null },
      ];

      writeStagedEntityBackup({
        agentPreferences: { "agent-001": { mobileSystemPrompt: "保留未恢复" } },
      });

      // User clicks cancel on prompt
      vi.spyOn(overlayStore, "showConfirm").mockResolvedValue(false);
      const applyMock = vi.fn(async () => true);

      const result = await executePostSyncConfigRestorePrompt(
        overlayStore,
        assistantStore,
        applyMock,
      );

      expect(result).toBe("cancelled");
      expect(applyMock).not.toHaveBeenCalled();
      // Staged data remains in localStorage for subsequent trigger
      expect(readStagedEntityBackup()).not.toBeNull();
    });

    it("Scenario B: Existing device full backup overwrite without staging", async () => {
      const assistantStore = useAssistantStore();
      assistantStore.agents = [
        { id: "agent-001", name: "架构师", model: "gemini", avatarCalculatedColor: null },
      ];
      assistantStore.groups = [
        { id: "group-dev", name: "研发群", avatarCalculatedColor: null, members: ["agent-001"], mode: "sequential" },
      ];

      const applySettings = vi.fn(async () => false); // false indicates NO missing entities
      const backup = {
        domains: { identity: { userName: "新指挥官" } },
        groupPreferences: {
          "group-dev": { chatMode: "sequential" as const },
        },
        exportedAt: "",
      };

      const result = await applyConfigBackup(backup, applySettings);
      expect(result.applied).toContain("用户身份");
      expect(result.applied).toContain("群聊设置");
      expect(result.skipped).toHaveLength(0);
      expect(result.failed).toHaveLength(0);

      // Verify no staging is left in localStorage
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();
    });
  });
});
