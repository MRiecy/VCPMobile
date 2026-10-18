import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { mount } from "@vue/test-utils";
import { shareFileNative } from "tauri-plugin-vcp-mobile";
import { useSyncSessionStore } from "@/core/stores/syncSession";
import { useOverlayStore } from "@/core/stores/overlay";
import { useNotificationStore } from "@/core/stores/notification";
import SyncSessionView from "@/features/sync/SyncSessionView.vue";
import SyncLogBrowserCore from "@/features/settings/components/SyncLogBrowserCore.vue";
import {
  emitTauriEvent,
  invokeMock,
  listenMock,
  mockInvoke,
} from "@/tests/mocks/tauri";

const testPlatform = vi.hoisted(() => ({ value: 'android' as 'android' | 'ios' }));
vi.mock("@/core/utils/platform", () => ({
  get nativePlatform() { return testPlatform.value; },
  get isAndroid() { return testPlatform.value === 'android'; },
  get isIOS() { return testPlatform.value === 'ios'; },
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

function syncError(
  code: string,
  message = "同步未完成",
  failedTopicIds: string[] = [],
  logFile: string | null = "20260813_120000_000_1_sync.log",
) {
  return {
    code,
    category: "data",
    origin: "desktop_plugin",
    stage: "messages",
    retryAction: "manual",
    message,
    guidance: "可重试一次；若仍失败，请保留最新同步日志。",
    failedTopicIds,
    logFile,
  };
}

describe("sync session ownership", () => {
  beforeEach(() => {
    testPlatform.value = 'android';
    const pinia = createPinia();
    setActivePinia(pinia);
    vi.mocked(shareFileNative).mockClear();
    mockInvoke("stop_sync", () => undefined);
    mockInvoke("plugin:vcp-mobile|get_battery_status", () => ({
      level: 80,
      isPowerSaveMode: false,
    }));
    mockInvoke("start_manual_sync", () => 1);
    mockInvoke("list_sync_log_files", () => []);
    mockInvoke("get_assistants_snapshot", () => ({
      agents: [],
      groups: [],
      unreadCounts: {},
    }));
    mockInvoke("batch_get_avatars", () => []);
  });

  it("preserves an unknown native iOS battery level without inventing a successful check", async () => {
    testPlatform.value = 'ios';
    mockInvoke('plugin:vcp-mobile|get_battery_status', () => ({
      level: null, isPowerSaveMode: false, unavailable: { level: 'not available' },
    }));
    const store = useSyncSessionStore();
    store.open();
    await store.startSync();
    expect(invokeMock.mock.calls.some(([command]) => command === 'plugin:vcp-mobile|get_battery_status')).toBe(true);
    expect(invokeMock.mock.calls.some(([command]) => command === 'start_manual_sync')).toBe(true);
    expect(store.logs.some(log => log.message.includes('无法确认当前电量'))).toBe(true);
    expect(store.logs.some(log => log.message === '设备状态检查完成')).toBe(false);
    emitTauriEvent('vcp-sync-status', { sessionId: 1, status: 'error', error: syncError('END') });
    await store.close();
  });

  it.each([
    { platform: 'android', level: 20, isPowerSaveMode: false, code: 'BATTERY_TOO_LOW' },
    { platform: 'android', level: 80, isPowerSaveMode: true, code: 'POWER_SAVE_MODE' },
    { platform: 'ios', level: 0, isPowerSaveMode: false, code: 'BATTERY_TOO_LOW' },
    { platform: 'ios', level: null, isPowerSaveMode: true, code: 'POWER_SAVE_MODE' },
  ] as const)("uses observed battery state on $platform ($code)", async ({ platform, level, isPowerSaveMode, code }) => {
    testPlatform.value = platform;
    mockInvoke('plugin:vcp-mobile|get_battery_status', () => ({ level, isPowerSaveMode }));
    const store = useSyncSessionStore();
    store.open();
    await store.startSync();
    expect(store.terminalError?.code).toBe(code);
    expect(invokeMock.mock.calls.some(([command]) => command === 'start_manual_sync')).toBe(false);
    await store.close();
  });

  it("unlistens registrations that resolve after the panel closes", async () => {
    const registrations = Array.from({ length: 5 }, () =>
      deferred<() => void>(),
    );
    const unlisteners = registrations.map(() => vi.fn());
    registrations.forEach((registration) => {
      listenMock.mockImplementationOnce(() => registration.promise);
    });

    const store = useSyncSessionStore();
    store.open();
    const closing = store.close();
    registrations.forEach((registration, index) => {
      registration.resolve(unlisteners[index]);
    });

    await closing;
    await Promise.all(
      registrations.map((registration) => registration.promise),
    );
    await Promise.resolve();
    unlisteners.forEach((unlisten) => expect(unlisten).toHaveBeenCalledOnce());
  });

  it("waits for an older close receipt before starting from a reopened panel", async () => {
    let nextSession = 0;
    mockInvoke('start_manual_sync', () => ++nextSession);
    const stop = deferred<void>();
    mockInvoke('stop_sync', () => stop.promise);
    const store = useSyncSessionStore();
    store.open();
    await store.startSync();
    emitTauriEvent('vcp-sync-status', { sessionId: 1, status: 'error', error: syncError('END') });

    const closing = store.close();
    store.open();
    const restarting = store.startSync();
    await Promise.resolve();
    expect(nextSession).toBe(1);
    expect(invokeMock.mock.calls.filter(([command]) => command === 'stop_sync')).toHaveLength(1);
    stop.resolve();
    await Promise.all([closing, restarting]);

    expect(nextSession).toBe(2);
    expect(store.activeSessionId).toBe(2);
  });

  it("retains the reload requirement across STOP_PENDING and consumes it only after real shutdown", async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    let nextSession = 0;
    let drainFinished = false;
    let reloads = 0;
    mockInvoke('start_manual_sync', () => ++nextSession);
    mockInvoke('get_assistants_snapshot', () => {
      reloads += 1;
      return { agents: [], groups: [], unreadCounts: {} };
    });
    mockInvoke('stop_sync', async () => {
      if (!drainFinished) throw `SYNC_ERROR:${JSON.stringify({
        ...syncError('SYNC_STOP_PENDING', '同步仍在收尾'),
        origin: 'mobile_sync', stage: 'shutdown', category: 'storage',
      })}`;
    });
    const store = useSyncSessionStore();
    store.open();
    await store.startSync();
    emitTauriEvent('vcp-sync-status', { sessionId: 1, status: 'open' });
    emitTauriEvent('vcp-sync-status', { sessionId: 1, status: 'error', error: syncError('END') });
    await store.close();
    expect(store.needsReload).toBe(true);
    expect(reloads).toBe(0);

    store.open();
    await store.startSync();
    expect(store.terminalError?.code).toBe('SYNC_STOP_PENDING');
    expect(nextSession).toBe(1);
    expect(reloads).toBe(0);
    drainFinished = true;
    await store.retrySync();

    expect(store.needsReload).toBe(false);
    expect(reloads).toBe(1);
    expect(nextSession).toBe(2);
  });

  it("rejects sessionless events while battery detection is pending", async () => {
    const battery = deferred<{ level: number; isPowerSaveMode: boolean }>();
    mockInvoke("plugin:vcp-mobile|get_battery_status", () => battery.promise);
    mockInvoke("start_manual_sync", () => 7);

    const store = useSyncSessionStore();
    store.open();
    const starting = store.startSync();
    await vi.waitFor(() =>
      expect(
        invokeMock.mock.calls.some(
          ([command]) => command === "plugin:vcp-mobile|get_battery_status",
        ),
      ).toBe(true),
    );
    expect(
      invokeMock.mock.calls.some(
        ([command]) => command === "start_manual_sync",
      ),
    ).toBe(false);

    emitTauriEvent("vcp-sync-status", { status: "error" });
    expect(store.status).toBe("connecting");
    expect(store.canDismiss).toBe(false);
    battery.resolve({ level: 80, isPowerSaveMode: false });
    await starting;

    expect(
      invokeMock.mock.calls.some(
        ([command]) => command === "start_manual_sync",
      ),
    ).toBe(true);
    emitTauriEvent("vcp-sync-status", {
      sessionId: 7,
      status: "error",
      error: syncError("CURRENT"),
    });
    expect(store.canDismiss).toBe(true);
    await store.close();
  });

  it("keeps a connected sync page mounted when system back is pressed", async () => {
    const overlay = useOverlayStore();
    const store = useSyncSessionStore();
    overlay.openSyncSession();

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(5));
    const statusRegistration = listenMock.mock.calls.find(
      ([eventName]) => eventName === "vcp-sync-status",
    );
    expect(statusRegistration).toBeTruthy();
    store.activeSessionId = 11;
    statusRegistration?.[1]({ payload: { sessionId: 10, status: "open" } });
    expect(store.status).toBe("idle");
    statusRegistration?.[1]({ payload: { sessionId: 11, status: "open" } });
    expect(store.canDismiss).toBe(false);

    window.dispatchEvent(new PopStateEvent("popstate", { state: { vcpMain: true } }));

    expect(store.isOpen).toBe(true);
    expect(overlay.isSyncSessionOpen).toBe(true);
    expect(
      invokeMock.mock.calls.some(([command]) => command === "stop_sync"),
    ).toBe(false);

    statusRegistration?.[1]({
      payload: {
        sessionId: 11,
        status: "error",
        error: syncError("TEST"),
      },
    });
    await overlay.closeSyncSession();
  });

  it("shows desktop backend info only for the current connected session", async () => {
    const store = useSyncSessionStore();
    store.open();
    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(5));
    store.activeSessionId = 61;

    emitTauriEvent("vcp-sync-status", {
      sessionId: 60,
      status: "open",
      desktop: { packageVersion: "1.5.0", backendMode: "cds" },
    });
    expect(store.desktopInfo).toBeNull();

    emitTauriEvent("vcp-sync-status", {
      sessionId: 61,
      status: "open",
      desktop: { packageVersion: "1.5.0", backendMode: "cds" },
    });
    expect(store.desktopInfo).toEqual({
      packageVersion: "1.5.0",
      backendMode: "cds",
    });

    const wrapper = mount(SyncSessionView);
    expect(wrapper.text()).toContain("桌面后端 CDS");
    expect(wrapper.text()).toContain("同步插件 v1.5.0");
    expect(wrapper.text()).not.toContain("复制诊断");

    emitTauriEvent("vcp-sync-status", {
      sessionId: 61,
      status: "connecting",
    });
    expect(store.desktopInfo).toBeNull();
  });

  it("buffers transfer snapshots until the start receipt identifies the owning session", async () => {
    const pendingStart = deferred<number>();
    mockInvoke("start_manual_sync", () => pendingStart.promise);
    const store = useSyncSessionStore();
    store.open();
    const starting = store.startSync();
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledWith("start_manual_sync"));

    const transfer = { uploadedBytes: 500, downloadedBytes: 1_500, activeDurationMs: 250.5 };
    emitTauriEvent("vcp-sync-transfer", { sessionId: 70, ...transfer, uploadedBytes: 9_000 });
    emitTauriEvent("vcp-sync-transfer", { sessionId: 71, ...transfer });
    emitTauriEvent("vcp-sync-transfer", transfer);
    expect(store.transfer).toBeNull();

    pendingStart.resolve(71);
    await starting;
    expect(store.transfer).toEqual(transfer);
    emitTauriEvent("vcp-sync-transfer", { sessionId: 70, ...transfer, uploadedBytes: 10_000 });
    expect(store.transfer).toEqual(transfer);
  });

  it("rejects malformed and regressing transfer snapshots without losing the last reading", () => {
    const store = useSyncSessionStore();
    store.open();
    store.activeSessionId = 72;
    const transfer = { uploadedBytes: 100, downloadedBytes: 900, activeDurationMs: 100.5 };
    emitTauriEvent("vcp-sync-transfer", { sessionId: 72, ...transfer });

    for (const patch of [
      { uploadedBytes: -1 },
      { uploadedBytes: Number.MAX_SAFE_INTEGER + 1 },
      { uploadedBytes: 100.1 },
      { downloadedBytes: "900" },
      { downloadedBytes: Number.NaN },
      { activeDurationMs: Number.POSITIVE_INFINITY },
      { activeDurationMs: -1 },
      { uploadedBytes: 99 },
      { downloadedBytes: 899 },
      { activeDurationMs: 100.4 },
    ]) {
      emitTauriEvent("vcp-sync-transfer", { sessionId: 72, ...transfer, ...patch });
      expect(store.transfer).toEqual(transfer);
    }
  });

  it("retains speed through automatic reconnect and local finalization, then resets for a manual retry", async () => {
    let nextSession = 0;
    mockInvoke("start_manual_sync", () => ++nextSession);
    const store = useSyncSessionStore();
    store.open();
    await store.startSync();
    const transfer = { uploadedBytes: 500, downloadedBytes: 1_500, activeDurationMs: 1_000 };
    emitTauriEvent("vcp-sync-transfer", { sessionId: 1, ...transfer });
    emitTauriEvent("vcp-sync-status", { sessionId: 1, status: "connecting" });
    emitTauriEvent("vcp-sync-progress", { sessionId: 1, phase: "finalize" });
    expect(store.transfer).toEqual(transfer);

    emitTauriEvent("vcp-sync-status", { sessionId: 1, status: "error", error: syncError("RETRYABLE") });
    expect(store.transfer).toEqual(transfer);
    await store.retrySync();
    expect(store.activeSessionId).toBe(2);
    expect(store.transfer).toBeNull();
    emitTauriEvent("vcp-sync-transfer", { sessionId: 1, ...transfer });
    expect(store.transfer).toBeNull();
  });

  it.each(["error", "completed", "completed_with_warnings"] as const)(
    "takes the final transfer snapshot from %s and ignores later measurements",
    (status) => {
      const store = useSyncSessionStore();
      store.open();
      store.activeSessionId = 73;
      emitTauriEvent("vcp-sync-transfer", {
        sessionId: 73, uploadedBytes: 100, downloadedBytes: 900, activeDurationMs: 1_000,
      });
      const transfer = { uploadedBytes: 200, downloadedBytes: 1_800, activeDurationMs: 1_500 };
      if (status === "error") {
        emitTauriEvent("vcp-sync-status", { sessionId: 73, status, error: syncError("END"), transfer });
      } else {
        emitTauriEvent("vcp-sync-completed", {
          sessionId: 73, status, transfer,
          summary: {
            successfulTopics: 1, totalTopics: 1, failedTopics: 0, failedTopicIds: [],
            legacyAttachmentWarnings: status === "completed_with_warnings" ? 1 : 0,
          },
        });
      }
      expect(store.status).toBe(status);
      expect(store.transfer).toEqual(transfer);
      emitTauriEvent("vcp-sync-transfer", {
        sessionId: 73, uploadedBytes: 300, downloadedBytes: 2_700, activeDurationMs: 2_000,
      });
      expect(store.transfer).toEqual(transfer);
    },
  );

  it("clears transfer data when reopening and rejects a previous view's listener callback", async () => {
    const store = useSyncSessionStore();
    store.open();
    store.activeSessionId = 74;
    const oldListener = listenMock.mock.calls.find(([name]) => name === "vcp-sync-transfer")?.[1];
    const transfer = { uploadedBytes: 100, downloadedBytes: 900, activeDurationMs: 500 };
    emitTauriEvent("vcp-sync-transfer", { sessionId: 74, ...transfer });
    expect(store.transfer).toEqual(transfer);
    await store.close();
    store.open();
    store.activeSessionId = 74;
    oldListener?.({ payload: { sessionId: 74, ...transfer } });
    expect(store.transfer).toBeNull();
  });

  it.each([
    { uploadedBytes: 0, downloadedBytes: 0, activeDurationMs: 1_000, expected: "—" },
    { uploadedBytes: 100, downloadedBytes: 900, activeDurationMs: 0, expected: "—" },
    { uploadedBytes: 125, downloadedBytes: 375, activeDurationMs: 1_000, expected: "500 B/s" },
    { uploadedBytes: 1_000, downloadedBytes: 4_000, activeDurationMs: 2_000, expected: "2.50 KB/s" },
    { uploadedBytes: 1_000_000, downloadedBytes: 3_000_000, activeDurationMs: 2_000, expected: "2.00 MB/s" },
    { uploadedBytes: 250, downloadedBytes: 750, activeDurationMs: 0.5, expected: "2.00 MB/s" },
  ])("renders application-byte average as $expected", async ({ expected, ...transfer }) => {
    const store = useSyncSessionStore();
    store.open();
    await store.startSync();
    const wrapper = mount(SyncSessionView);
    expect(wrapper.get('[aria-label="平均同步速度"]').text()).toContain("—");
    emitTauriEvent("vcp-sync-transfer", { sessionId: 1, ...transfer });
    await Promise.resolve();
    expect(wrapper.get('[aria-label="平均同步速度"]').text()).toContain(expected);
  });

  it("keeps a connecting sync page mounted while start_manual_sync is pending", async () => {
    const pendingStart = deferred<number>();
    mockInvoke("plugin:vcp-mobile|get_battery_status", () => ({
      level: 80,
      isPowerSaveMode: false,
    }));
    mockInvoke("start_manual_sync", () => pendingStart.promise);

    const overlay = useOverlayStore();
    const store = useSyncSessionStore();
    overlay.openSyncSession();
    const starting = store.startSync();

    await vi.waitFor(() =>
      expect(
        invokeMock.mock.calls.some(
          ([command]) => command === "start_manual_sync",
        ),
      ).toBe(true),
    );
    expect(store.status).toBe("connecting");
    expect(store.canDismiss).toBe(false);

    window.dispatchEvent(
      new PopStateEvent("popstate", { state: { vcpMain: true } }),
    );

    expect(store.isOpen).toBe(true);
    expect(overlay.isSyncSessionOpen).toBe(true);
    expect(
      invokeMock.mock.calls.some(([command]) => command === "stop_sync"),
    ).toBe(false);

    emitTauriEvent("vcp-sync-status", { sessionId: 23, status: "open" });
    expect(store.status).toBe("connecting");
    pendingStart.resolve(23);
    await starting;
    expect(store.status).toBe("connected");
    emitTauriEvent("vcp-sync-status", {
      sessionId: 23,
      status: "error",
      error: syncError("TEST"),
    });
    await overlay.closeSyncSession();
  });

  it.each(["idle", "error", "completed", "completed_with_warnings"] as const)(
    "dismisses a %s panel through system back and still requests backend stop",
    async (terminalStatus) => {
      const overlay = useOverlayStore();
      const store = useSyncSessionStore();
      overlay.openSyncSession();
      await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(5));

      if (terminalStatus === "error") {
        store.activeSessionId = 31;
        emitTauriEvent("vcp-sync-status", {
          sessionId: 31,
          status: "error",
          error: syncError("TEST"),
        });
      } else if (
        terminalStatus === "completed" ||
        terminalStatus === "completed_with_warnings"
      ) {
        store.activeSessionId = 31;
        emitTauriEvent("vcp-sync-completed", {
          sessionId: 31,
          status: terminalStatus,
          summary: {
            successfulTopics: 1,
            totalTopics: 1,
            failedTopics: 0,
            legacyAttachmentWarnings:
              terminalStatus === "completed_with_warnings" ? 1 : 0,
            failedTopicIds: [],
          },
        });
      }

      expect(store.status).toBe(terminalStatus);
      expect(store.canDismiss).toBe(true);
      window.dispatchEvent(
        new PopStateEvent("popstate", { state: { vcpMain: true } }),
      );

      expect(store.isOpen).toBe(false);
      expect(overlay.isSyncSessionOpen).toBe(false);
      await vi.waitFor(() =>
        expect(
          invokeMock.mock.calls.some(([command]) => command === "stop_sync"),
        ).toBe(true),
      );
    },
  );

  it("keeps error terminal when a late completion event arrives", () => {
    const store = useSyncSessionStore();
    store.open();
    store.activeSessionId = 41;

    emitTauriEvent("vcp-sync-status", {
      sessionId: 41,
      status: "error",
      error: syncError("DESKTOP_DB", "部分数据未能完成处理", ["topic-a"]),
    });
    emitTauriEvent("vcp-sync-completed", {
      sessionId: 41,
      status: "completed",
      summary: {
        successfulTopics: 1,
        totalTopics: 1,
        failedTopics: 0,
        legacyAttachmentWarnings: 0,
        failedTopicIds: [],
      },
    });

    expect(store.status).toBe("error");
    expect(store.needsReload).toBe(false);
    expect(store.terminalError?.code).toBe("DESKTOP_DB");
  });

  it.each([
    {
      status: "not-completed",
      summary: {
        successfulTopics: 1,
        totalTopics: 1,
        failedTopics: 0,
        legacyAttachmentWarnings: 0,
        failedTopicIds: [],
      },
    },
    {
      status: "completed",
      summary: {
        successfulTopics: 1,
        totalTopics: "1",
        failedTopics: 0,
        legacyAttachmentWarnings: 0,
        failedTopicIds: [],
      },
    },
  ])("fails closed on a malformed completion event", ({ status, summary }) => {
    const store = useSyncSessionStore();
    store.open();
    store.activeSessionId = 42;

    emitTauriEvent("vcp-sync-completed", {
      sessionId: 42,
      status,
      summary,
    });

    expect(store.status).toBe("error");
    expect(store.needsReload).toBe(false);
    expect(store.terminalError?.code).toBe("INVALID_COMPLETION_EVENT");
  });

  it("uses only the validated completed event as the success transition", () => {
    const store = useSyncSessionStore();
    store.open();
    store.activeSessionId = 43;

    emitTauriEvent("vcp-sync-status", {
      sessionId: 43,
      status: "completed",
    });
    expect(store.status).toBe("idle");
    expect(store.needsReload).toBe(false);

    emitTauriEvent("vcp-sync-completed", {
      sessionId: 43,
      status: "completed",
      summary: {
        successfulTopics: 2,
        totalTopics: 2,
        failedTopics: 0,
        legacyAttachmentWarnings: 0,
        failedTopicIds: [],
      },
    });
    expect(store.status).toBe("completed");
    expect(store.needsReload).toBe(true);
  });

  it("retains the latest valid topic counts when the attempt ends in error", () => {
    const store = useSyncSessionStore();
    store.open();
    store.activeSessionId = 44;

    emitTauriEvent("vcp-sync-progress", {
      sessionId: 44,
      phase: "messages",
      total: 5,
      completed: 3,
      message: "3/5",
      successfulTopics: 3,
      totalTopics: 5,
      failedTopics: 0,
      legacyAttachmentWarnings: 2,
    });
    emitTauriEvent("vcp-sync-status", {
      sessionId: 44,
      status: "error",
      error: syncError("PULL_FAILED", "部分数据未能完成处理", ["topic-d"]),
    });

    expect(store.summary).toMatchObject({
      successfulTopics: 3,
      totalTopics: 5,
      failedTopics: 1,
      legacyAttachmentWarnings: 2,
      failedTopicIds: ["topic-d"],
    });
  });

  it("awaits stop before retrying and preserves an attempt separator", async () => {
    let nextSession = 0;
    mockInvoke("start_manual_sync", () => ++nextSession);
    const store = useSyncSessionStore();
    store.open();
    await store.startSync();
    emitTauriEvent("vcp-sync-status", {
      sessionId: 1,
      status: "error",
      error: syncError("RETRYABLE"),
    });

    await store.retrySync();

    const syncCommands = invokeMock.mock.calls
      .map(([command]) => command)
      .filter(
        (command) => command === "start_manual_sync" || command === "stop_sync",
      );
    expect(syncCommands).toEqual([
      "start_manual_sync",
      "stop_sync",
      "start_manual_sync",
    ]);
    expect(store.activeSessionId).toBe(2);
    expect(store.logs.some((log) => log.message.includes("新同步尝试"))).toBe(
      true,
    );
  });

  it("joins the old session before separating attempts and ignores its late logs", async () => {
    let nextSession = 0;
    const stopping = deferred<void>();
    mockInvoke("start_manual_sync", () => ++nextSession);
    const store = useSyncSessionStore();
    store.open();
    await store.startSync();
    emitTauriEvent("vcp-sync-status", {
      sessionId: 1,
      status: "error",
      error: syncError("RETRYABLE"),
    });
    mockInvoke("stop_sync", () => stopping.promise);

    const retrying = store.retrySync();
    await vi.waitFor(() =>
      expect(
        invokeMock.mock.calls.some(([command]) => command === "stop_sync"),
      ).toBe(true),
    );
    emitTauriEvent("vcp-log", {
      category: "sync",
      audience: "operator",
      sessionId: 1,
      level: "warning",
      message: "旧会话正在退出",
    });
    expect(store.logs.some((log) => log.message.includes("新同步尝试"))).toBe(
      false,
    );

    stopping.resolve();
    await retrying;
    const separatorIndex = store.logs.findIndex((log) =>
      log.message.includes("新同步尝试"),
    );
    expect(
      store.logs.some((log) => log.message === "旧会话正在退出"),
    ).toBe(false);
    expect(separatorIndex).toBeGreaterThanOrEqual(0);
  });

  it("shows only owned operator notices and keeps numeric progress nonterminal", async () => {
    const store = useSyncSessionStore();
    store.open();
    await store.startSync();

    emitTauriEvent("vcp-log", {
      category: "sync",
      level: "error",
      message: "unowned diagnostic",
    });
    emitTauriEvent("vcp-log", {
      category: "sync",
      audience: "operator",
      sessionId: 2,
      level: "warning",
      message: "其他会话正在重试",
    });
    emitTauriEvent("vcp-log", {
      category: "sync",
      audience: "operator",
      sessionId: 1,
      level: "warning",
      message: "连接中断，正在进行第 1/3 次自动重试",
    });
    for (const completed of [0, 1, 2, 2]) {
      emitTauriEvent("vcp-sync-progress", {
        sessionId: 1,
        phase: "topic_metadata",
        total: 2,
        completed,
        message: `raw progress ${completed}`,
        successfulTopics: completed,
        totalTopics: 2,
        failedTopics: 0,
        legacyAttachmentWarnings: 0,
      });
    }
    emitTauriEvent("vcp-sync-progress", {
      sessionId: 1,
      phase: "unknown_phase",
      total: 2,
      completed: 2,
      message: "unknown phase progress",
    });

    expect(store.logs.some((log) => log.message.includes("unowned diagnostic"))).toBe(
      false,
    );
    expect(store.logs.some((log) => log.message === "其他会话正在重试")).toBe(
      false,
    );
    expect(
      store.logs.filter((log) => log.message === "开始会话主题同步"),
    ).toHaveLength(1);
    expect(
      store.logs.filter((log) => log.message === "会话主题同步完成"),
    ).toHaveLength(0);
    expect(store.progressData.phase).toBe("topic_metadata");
    expect(store.logs.some((log) => log.message.includes("unknown phase progress"))).toBe(
      false,
    );
  });

  it.each([
    {
      battery: { level: 80, isPowerSaveMode: true },
      message: "系统省电模式已阻止本次同步",
      guidance: "关闭系统省电模式后再试。",
    },
    {
      battery: { level: 29, isPowerSaveMode: false },
      message: "当前电量不足，已暂停同步",
      guidance: "电量达到 30% 后再试。",
    },
  ])("uses fixed device guidance for $message", async ({ battery, message, guidance }) => {
    mockInvoke("plugin:vcp-mobile|get_battery_status", () => battery);
    const store = useSyncSessionStore();
    store.open();
    await store.startSync();

    expect(store.terminalError).toMatchObject({ message, guidance });
    expect(
      invokeMock.mock.calls.some(([command]) => command === "start_manual_sync"),
    ).toBe(false);
  });

  it("mounts only the selected sync panel and reloads history on each entry", async () => {
    const store = useSyncSessionStore();
    store.open();
    const wrapper = mount(SyncSessionView);
    const historyLoads = () =>
      invokeMock.mock.calls.filter(
        ([command]) => command === "list_sync_log_files",
      ).length;

    expect(wrapper.get("#sync-live-panel").attributes("role")).toBe(
      "tabpanel",
    );
    expect(wrapper.find("#sync-history-panel").exists()).toBe(false);
    expect(wrapper.findComponent(SyncLogBrowserCore).exists()).toBe(false);
    expect(historyLoads()).toBe(0);
    expect(wrapper.get("#sync-live-tab").attributes("aria-selected")).toBe(
      "true",
    );

    await wrapper.get("#sync-history-tab").trigger("click");
    await vi.waitFor(() => expect(historyLoads()).toBe(1));
    expect(wrapper.find("#sync-live-panel").exists()).toBe(false);
    expect(wrapper.get("#sync-history-panel").attributes("role")).toBe(
      "tabpanel",
    );
    expect(wrapper.findComponent(SyncLogBrowserCore).exists()).toBe(true);
    expect(wrapper.get("#sync-history-tab").attributes("aria-selected")).toBe(
      "true",
    );

    await wrapper.get("#sync-live-tab").trigger("click");
    expect(wrapper.find("#sync-history-panel").exists()).toBe(false);
    expect(wrapper.findComponent(SyncLogBrowserCore).exists()).toBe(false);

    await wrapper.get("#sync-history-tab").trigger("click");
    await vi.waitFor(() => expect(historyLoads()).toBe(2));
  });

  it.each(["connecting", "connected"] as const)(
    "keeps history unavailable while sync is %s",
    async (status) => {
      const store = useSyncSessionStore();
      store.open();
      store.status = status;
      const wrapper = mount(SyncSessionView);

      expect(wrapper.get("#sync-history-tab").attributes()).toHaveProperty(
        "disabled",
      );
      store.switchTab("history");
      expect(store.activeTab).toBe("live");
      expect(wrapper.find("#sync-history-panel").exists()).toBe(false);
    },
  );

  it("scrolls the live terminal to the latest log after remounting", async () => {
    const scrollHeight = vi
      .spyOn(HTMLElement.prototype, "scrollHeight", "get")
      .mockReturnValue(640);
    try {
      const store = useSyncSessionStore();
      store.open();
      store.status = "completed";
      store.logs.push({
        id: "terminal",
        level: "success",
        message: "同步完成",
        time: "12:00:00",
      });
      const wrapper = mount(SyncSessionView);

      await vi.waitFor(() =>
        expect(
          (wrapper.get("#sync-live-panel .overflow-y-auto")
            .element as HTMLElement).scrollTop,
        ).toBe(640),
      );

      await wrapper.get("#sync-history-tab").trigger("click");
      await wrapper.get("#sync-live-tab").trigger("click");
      await vi.waitFor(() =>
        expect(
          (wrapper.get("#sync-live-panel .overflow-y-auto")
            .element as HTMLElement).scrollTop,
        ).toBe(640),
      );
      expect(wrapper.text()).toContain("同步完成");
    } finally {
      scrollHeight.mockRestore();
    }
  });

  it("uses the contract retry action instead of offering every terminal error a retry", async () => {
    const store = useSyncSessionStore();
    store.open();
    store.activeSessionId = 52;
    emitTauriEvent("vcp-sync-status", {
      sessionId: 52,
      status: "error",
      error: {
        ...syncError("WIRE_VERSION_MISMATCH"),
        category: "compatibility",
        stage: "handshake",
        retryAction: "after_user_action",
        message: "手机端与电脑端同步版本不兼容",
        guidance: "将两端更新到同一兼容版本后再试。",
      },
    });
    const wrapper = mount(SyncSessionView);
    await Promise.resolve();
    expect(wrapper.text()).toContain("已处理，重新同步");

    store.activeSessionId = 53;
    store.status = "connecting";
    store.terminalError = null;
    emitTauriEvent("vcp-sync-status", {
      sessionId: 53,
      status: "error",
      error: {
        ...syncError("SYNC_ALREADY_RUNNING"),
        category: "internal",
        origin: "mobile_sync",
        stage: "startup",
        retryAction: "never",
        message: "已有同步任务正在运行",
        guidance: "请等待当前同步结束。",
      },
    });
    await Promise.resolve();
    expect(wrapper.text()).not.toContain("已处理，重新同步");
    expect(wrapper.text()).not.toContain("重新同步");
  });

  it("keeps the history list available after a file read failure", async () => {
    mockInvoke("list_sync_log_files", () => [
      {
        filename: "20260813_120000_000_1_sync.log",
        created_at: 1_786_576_800,
        size_bytes: 128,
      },
    ]);
    let attempts = 0;
    mockInvoke("read_sync_log_file", () => {
      attempts += 1;
      return attempts === 1
        ? Promise.reject("read failed")
        : "[2026-08-13T12:00:00.000+08:00] [INFO] retry succeeded";
    });
    const notifications = useNotificationStore();
    const wrapper = mount(SyncLogBrowserCore);

    await vi.waitFor(() =>
      expect(wrapper.text()).toContain("20260813_120000_000_1_sync.log"),
    );
    await wrapper.get('[class*="cursor-pointer"]').trigger("click");
    await vi.waitFor(() =>
      expect(notifications.activeToasts.length).toBeGreaterThan(0),
    );
    expect(wrapper.text()).toContain("20260813_120000_000_1_sync.log");

    await wrapper.get('[class*="cursor-pointer"]').trigger("click");
    await vi.waitFor(() => expect(wrapper.text()).toContain("retry succeeded"));
  });

  it("shares the selected history log as a staged file through Android", async () => {
    const filename = "20260813_120000_000_1_sync.log";
    const stagedPath = `/cache/sync_log_shares/1/${filename}`;
    mockInvoke("list_sync_log_files", () => [
      {
        filename,
        created_at: 1_786_576_800,
        size_bytes: 4_096,
      },
    ]);
    mockInvoke("read_sync_log_file", () => "[INFO] sync completed");
    mockInvoke("prepare_sync_log_share_file", (args) => {
      expect(args).toEqual({ filename });
      return stagedPath;
    });
    const notifications = useNotificationStore();
    const wrapper = mount(SyncLogBrowserCore);

    await vi.waitFor(() => expect(wrapper.text()).toContain(filename));
    await wrapper.get('[class*="cursor-pointer"]').trigger("click");
    await vi.waitFor(() => expect(wrapper.text()).toContain("sync completed"));
    const shareButton = wrapper
      .findAll("button")
      .find((button) => button.text() === "分享");
    expect(shareButton).toBeTruthy();
    await shareButton!.trigger("click");

    await vi.waitFor(() =>
      expect(shareFileNative).toHaveBeenCalledWith(stagedPath),
    );
    expect(
      notifications.activeToasts[notifications.activeToasts.length - 1]
        ?.message,
    ).toBe("已打开系统分享面板");
  });

});
