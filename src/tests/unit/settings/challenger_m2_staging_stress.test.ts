import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import {
  STAGED_BACKUP_STORAGE_KEY,
  STAGED_BACKUP_TTL_MS,
  isStagedPayloadValid,
  readStagedEntityBackup,
  writeStagedEntityBackup,
  type StagedEntityBackupPayload,
} from "../../../features/settings/useConfigBackup";
import { checkAndPromptPendingConfigRestore } from "../../../core/stores/syncSession";
import { useOverlayStore } from "../../../core/stores/overlay";
import { useAssistantStore } from "../../../core/stores/assistant";
import { useSettingsStore } from "../../../core/stores/settings";

describe("Empirical Challenger M2: Staging Storage Stress & Adversarial Harness", () => {
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

  // =========================================================================
  // Challenge Dimension 1: Malformed & Corrupted localStorage Content
  // =========================================================================
  describe("Challenge 1: Malformed and Corrupted localStorage values", () => {
    const malformedNonJsonValues = [
      { label: "Truncated JSON object", value: '{"timestamp": 12345, "version":' },
      { label: "Truncated JSON string", value: '{"agentPreferences": "unterminated' },
      { label: "Pure random gibberish", value: "random_text_not_json_#%!@*&^" },
      { label: "XML markup", value: "<xml><payload>staged</payload></xml>" },
      { label: "Binary control characters", value: "\x00\x01\x02\xFF\xFE\xFD" },
      { label: "Whitespace only", value: "    \r\n\t   " },
      { label: "JavaScript undefined literal", value: "undefined" },
      { label: "Object toString representation", value: "[object Object]" },
    ];

    malformedNonJsonValues.forEach(({ label, value }) => {
      it(`readStagedEntityBackup gracefully recovers from ${label} and purges key`, () => {
        localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, value);
        let result: StagedEntityBackupPayload | null = null;
        expect(() => {
          result = readStagedEntityBackup();
        }).not.toThrow();
        expect(result).toBeNull();
        expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();
      });

      it(`checkAndPromptPendingConfigRestore gracefully recovers from ${label} and purges key`, async () => {
        localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, value);
        let restored = false;
        await expect(async () => {
          restored = await checkAndPromptPendingConfigRestore();
        }).not.toThrow();
        expect(restored).toBe(false);
        expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();
      });
    });

    it("handles empty string by short-circuiting as falsy without error", async () => {
      localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, "");
      expect(readStagedEntityBackup()).toBeNull();
      const restored = await checkAndPromptPendingConfigRestore();
      expect(restored).toBe(false);
      // Empty string is falsy so !raw triggers early return without parsing
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBe("");
    });

    const nonObjectValidJsonValues = [
      { label: "JSON number primitive", value: "123456789" },
      { label: "JSON boolean true primitive", value: "true" },
      { label: "JSON boolean false primitive", value: "false" },
      { label: "JSON null literal", value: "null" },
      { label: "JSON string literal", value: '"valid json string but not object"' },
      { label: "JSON array", value: '[1, 2, "three", {"version": 2}]' },
      { label: "Empty JSON object", value: "{}" },
    ];

    nonObjectValidJsonValues.forEach(({ label, value }) => {
      it(`handles non-conforming valid JSON ${label} without throwing`, async () => {
        localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, value);
        expect(() => readStagedEntityBackup()).not.toThrow();
        expect(readStagedEntityBackup()).toBeNull();

        localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, value);
        const restored = await checkAndPromptPendingConfigRestore();
        expect(restored).toBe(false);
        expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();
      });
    });

    const corruptedTimestampValues = [
      { label: "Missing timestamp", payload: { version: 2 } },
      { label: "String timestamp", payload: { timestamp: "2026-10-31T08:00:00Z", version: 2 } },
      { label: "Null timestamp", payload: { timestamp: null, version: 2 } },
      { label: "Boolean timestamp", payload: { timestamp: true, version: 2 } },
      { label: "Array timestamp", payload: { timestamp: [Date.now()], version: 2 } },
      { label: "NaN timestamp", payload: { timestamp: NaN, version: 2 } },
      { label: "Infinity timestamp", payload: { timestamp: Infinity, version: 2 } },
      { label: "-Infinity timestamp", payload: { timestamp: -Infinity, version: 2 } },
    ];

    corruptedTimestampValues.forEach(({ label, payload }) => {
      it(`rejects and purges payload with ${label}`, async () => {
        localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, JSON.stringify(payload));
        expect(readStagedEntityBackup()).toBeNull();
        expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();

        localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, JSON.stringify(payload));
        const restored = await checkAndPromptPendingConfigRestore();
        expect(restored).toBe(false);
        expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();
      });
    });
  });

  // =========================================================================
  // Challenge Dimension 2: Future Timestamp Clock Skew Defense
  // =========================================================================
  describe("Challenge 2: Future Timestamp Clock Skew Defense", () => {
    const fixedNow = 1761800000000;

    const clockSkewDeltas = [
      { label: "1 ms in the future", delta: 1 },
      { label: "100 ms in the future", delta: 100 },
      { label: "1 second in the future", delta: 1000 },
      { label: "1 minute in the future", delta: 60 * 1000 },
      { label: "1 hour in the future", delta: 3600 * 1000 },
      { label: "1 day in the future", delta: 86400 * 1000 },
      { label: "1 year in the future", delta: 365 * 86400 * 1000 },
      { label: "Maximum safe integer timestamp", delta: Number.MAX_SAFE_INTEGER - fixedNow },
    ];

    clockSkewDeltas.forEach(({ label, delta }) => {
      it(`isStagedPayloadValid rejects future payload (${label})`, () => {
        const payload: StagedEntityBackupPayload = {
          timestamp: fixedNow + delta,
          version: 2,
          agentPreferences: { "agent-future": { mobileSystemPrompt: "test" } },
        };
        expect(isStagedPayloadValid(payload, fixedNow)).toBe(false);
      });

      it(`readStagedEntityBackup rejects and purges future payload (${label})`, () => {
        const payload: StagedEntityBackupPayload = {
          timestamp: fixedNow + delta,
          version: 2,
          agentPreferences: { "agent-future": { mobileSystemPrompt: "test" } },
        };
        localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, JSON.stringify(payload));
        const readResult = readStagedEntityBackup(fixedNow);
        expect(readResult).toBeNull();
        expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();
      });

      it(`checkAndPromptPendingConfigRestore rejects and purges future payload (${label})`, async () => {
        vi.spyOn(Date, "now").mockReturnValue(fixedNow);
        const payload = {
          timestamp: fixedNow + delta,
          version: 2,
          agentPreferences: { "agent-future": { mobileSystemPrompt: "test" } },
        };
        localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, JSON.stringify(payload));
        const restored = await checkAndPromptPendingConfigRestore();
        expect(restored).toBe(false);
        expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();
      });
    });
  });

  // =========================================================================
  // Challenge Dimension 3: Exact 7-Day TTL Millisecond Boundary
  // =========================================================================
  describe("Challenge 3: Exact 7-day TTL millisecond boundary", () => {
    const fixedNow = 1761800000000;
    const exactSevenDaysMs = 7 * 24 * 60 * 60 * 1000; // 604,800,000 ms

    it("verifies STAGED_BACKUP_TTL_MS constant strictly equals 604,800,000 ms", () => {
      expect(STAGED_BACKUP_TTL_MS).toBe(604800000);
      expect(STAGED_BACKUP_TTL_MS).toBe(7 * 24 * 60 * 60 * 1000);
    });

    it("accepts timestamp exactly on the 7-day boundary (Date.now() - timestamp === 7*24*60*60*1000)", () => {
      const boundaryTimestamp = fixedNow - exactSevenDaysMs;
      const payload: StagedEntityBackupPayload = {
        timestamp: boundaryTimestamp,
        version: 2,
        agentPreferences: { "agent-boundary": { mobileSystemPrompt: "on the boundary" } },
      };

      expect(isStagedPayloadValid(payload, fixedNow)).toBe(true);

      localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, JSON.stringify(payload));
      const readResult = readStagedEntityBackup(fixedNow);
      expect(readResult).not.toBeNull();
      expect(readResult?.agentPreferences?.["agent-boundary"]?.mobileSystemPrompt).toBe("on the boundary");
      // Key must not be removed since it's valid
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).not.toBeNull();
    });

    it("accepts timestamp 1 ms before the 7-day boundary (boundary - 1 ms)", () => {
      const justBeforeBoundary = fixedNow - (exactSevenDaysMs - 1);
      const payload: StagedEntityBackupPayload = {
        timestamp: justBeforeBoundary,
        version: 2,
        agentPreferences: { "agent-almost-expired": { mobileSystemPrompt: "valid" } },
      };

      expect(isStagedPayloadValid(payload, fixedNow)).toBe(true);
    });

    it("rejects and purges timestamp 1 ms after the 7-day boundary (boundary + 1 ms)", () => {
      const justAfterBoundary = fixedNow - (exactSevenDaysMs + 1);
      const payload: StagedEntityBackupPayload = {
        timestamp: justAfterBoundary,
        version: 2,
        agentPreferences: { "agent-expired": { mobileSystemPrompt: "expired" } },
      };

      // 1. isStagedPayloadValid returns false
      expect(isStagedPayloadValid(payload, fixedNow)).toBe(false);

      // 2. readStagedEntityBackup returns null and purges key
      localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, JSON.stringify(payload));
      const readResult = readStagedEntityBackup(fixedNow);
      expect(readResult).toBeNull();
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();
    });

    it("checkAndPromptPendingConfigRestore accepts on boundary and rejects at boundary + 1 ms", async () => {
      vi.spyOn(Date, "now").mockReturnValue(fixedNow);
      const assistantStore = useAssistantStore();
      assistantStore.agents = [
        { id: "agent-boundary", name: "BoundaryAgent", model: "gemini", avatarCalculatedColor: null },
      ];
      assistantStore.groups = [];

      const overlayStore = useOverlayStore();
      vi.spyOn(overlayStore, "showConfirm").mockResolvedValue(true);

      const settingsStore = useSettingsStore();
      settingsStore.snapshot = { domains: {} as any, data: {} as any };
      vi.spyOn(settingsStore, "importBackup").mockResolvedValue({
        result: { status: "saved", snapshot: settingsStore.snapshot },
        missingGroups: false,
        missingAgents: false,
      });

      // Subcase 1: Boundary + 1 ms -> must be purged and return false
      localStorage.setItem(
        STAGED_BACKUP_STORAGE_KEY,
        JSON.stringify({
          timestamp: fixedNow - (exactSevenDaysMs + 1),
          version: 2,
          agentPreferences: { "agent-boundary": { mobileSystemPrompt: "expired" } },
        }),
      );
      const expiredRestore = await checkAndPromptPendingConfigRestore();
      expect(expiredRestore).toBe(false);
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();

      // Subcase 2: Exact boundary -> must be accepted and prompt
      localStorage.setItem(
        STAGED_BACKUP_STORAGE_KEY,
        JSON.stringify({
          timestamp: fixedNow - exactSevenDaysMs,
          version: 2,
          agentPreferences: { "agent-boundary": { mobileSystemPrompt: "boundary active" } },
        }),
      );
      const validRestore = await checkAndPromptPendingConfigRestore();
      expect(validRestore).toBe(true);
      // Cleaned up after successful application
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();
    });

    it("accepts timestamp created right now (age === 0 ms)", () => {
      const payload: StagedEntityBackupPayload = {
        timestamp: fixedNow,
        version: 2,
        agentPreferences: { "agent-fresh": { mobileSystemPrompt: "brand new" } },
      };
      expect(isStagedPayloadValid(payload, fixedNow)).toBe(true);
    });
  });

  // =========================================================================
  // Challenge Dimension 4: Mismatched Version Handling
  // =========================================================================
  describe("Challenge 4: Mismatched Version Handling", () => {
    const mismatchedVersions = [
      { label: "Version 1 (legacy)", version: 1 },
      { label: "Version 3 (future upgrade)", version: 3 },
      { label: "Version 0", version: 0 },
      { label: "Negative version -1", version: -1 },
      { label: "Float version 2.5", version: 2.5 },
      { label: "String version '2'", version: "2" },
      { label: "String version 'v2'", version: "v2" },
      { label: "Null version", version: null },
      { label: "Undefined version", version: undefined },
      { label: "Boolean version true", version: true },
    ];

    mismatchedVersions.forEach(({ label, version }) => {
      it(`isStagedPayloadValid rejects ${label}`, () => {
        const payload = {
          timestamp: Date.now(),
          version,
          agentPreferences: { "agent-1": { mobileSystemPrompt: "test" } },
        } as unknown as StagedEntityBackupPayload;
        expect(isStagedPayloadValid(payload)).toBe(false);
      });

      it(`readStagedEntityBackup rejects and purges ${label}`, () => {
        const payload = {
          timestamp: Date.now(),
          version,
          agentPreferences: { "agent-1": { mobileSystemPrompt: "test" } },
        };
        localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, JSON.stringify(payload));
        expect(readStagedEntityBackup()).toBeNull();
        expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();
      });

      it(`checkAndPromptPendingConfigRestore rejects ${label}`, async () => {
        const payload = {
          timestamp: Date.now(),
          version,
          agentPreferences: { "agent-1": { mobileSystemPrompt: "test" } },
        };
        localStorage.setItem(STAGED_BACKUP_STORAGE_KEY, JSON.stringify(payload));
        const result = await checkAndPromptPendingConfigRestore();
        expect(result).toBe(false);
      });
    });
  });

  // =========================================================================
  // Challenge Dimension 5: Robustness against non-object entity preference trees
  // =========================================================================
  describe("Challenge 5: Robustness against non-object entity preference trees", () => {
    it("handles staging payload where agentPreferences or groupPreferences are corrupted primitives", async () => {
      const corruptedPayload = {
        timestamp: Date.now(),
        version: 2,
        agentPreferences: "corrupted_string_not_map" as any,
        groupPreferences: 12345 as any,
      };

      writeStagedEntityBackup({
        agentPreferences: corruptedPayload.agentPreferences,
        groupPreferences: corruptedPayload.groupPreferences,
      });

      const assistantStore = useAssistantStore();
      assistantStore.agents = [
        { id: "agent-1", name: "Agent1", model: "gemini", avatarCalculatedColor: null },
      ];

      const overlayStore = useOverlayStore();
      vi.spyOn(overlayStore, "showConfirm").mockResolvedValue(true);

      const settingsStore = useSettingsStore();
      settingsStore.snapshot = { domains: {} as any, data: {} as any };
      const importSpy = vi.spyOn(settingsStore, "importBackup").mockResolvedValue({
        result: { status: "saved", snapshot: settingsStore.snapshot },
        missingGroups: false,
        missingAgents: false,
      });

      // Direct await
      const restored = await checkAndPromptPendingConfigRestore();

      expect(restored).toBe(true);
      expect(importSpy).toHaveBeenCalled();
      expect(localStorage.getItem(STAGED_BACKUP_STORAGE_KEY)).toBeNull();
    });
  });
});
