import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useSyncSessionStore } from "@/core/stores/syncSession";
import { useOverlayStore } from "@/core/stores/overlay";
import {
  emitTauriEvent,
  invokeMock,
  listenMock,
  mockInvoke,
} from "@/tests/mocks/tauri";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

describe("sync session ownership", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mockInvoke("stop_sync", () => undefined);
    mockInvoke("plugin:vcp-mobile|get_battery_status", () => ({
      level: 80,
      isPowerSaveMode: false,
    }));
    mockInvoke("start_manual_sync", () => 1);
  });

  it("unlistens registrations that resolve after the panel closes", async () => {
    const registrations = Array.from({ length: 4 }, () =>
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
      error: { code: "CURRENT", message: "failed", failedTopicIds: [] },
    });
    expect(store.canDismiss).toBe(true);
    await store.close();
  });

  it("keeps a connected sync page mounted when system back is pressed", async () => {
    const overlay = useOverlayStore();
    const store = useSyncSessionStore();
    overlay.openSyncSession();

    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(4));
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
        error: { code: "TEST", message: "done", failedTopicIds: [] },
      },
    });
    await overlay.closeSyncSession();
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
      error: { code: "TEST", message: "failed", failedTopicIds: [] },
    });
    await overlay.closeSyncSession();
  });

  it.each(["idle", "error", "completed", "completed_with_warnings"] as const)(
    "dismisses a %s panel through system back and still requests backend stop",
    async (terminalStatus) => {
      const overlay = useOverlayStore();
      const store = useSyncSessionStore();
      overlay.openSyncSession();
      await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(4));

      if (terminalStatus === "error") {
        store.activeSessionId = 31;
        emitTauriEvent("vcp-sync-status", {
          sessionId: 31,
          status: "error",
          error: { code: "TEST", message: "failed", failedTopicIds: [] },
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
      error: {
        code: "DESKTOP_DB",
        message: "write failed",
        failedTopicIds: ["topic-a"],
      },
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
      error: {
        code: "PULL_FAILED",
        message: "failed",
        failedTopicIds: ["topic-d"],
      },
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
      error: { code: "RETRYABLE", message: "retry", failedTopicIds: [] },
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

  it("inserts the retry separator only after the old session has joined", async () => {
    let nextSession = 0;
    const stopping = deferred<void>();
    mockInvoke("start_manual_sync", () => ++nextSession);
    const store = useSyncSessionStore();
    store.open();
    await store.startSync();
    emitTauriEvent("vcp-sync-status", {
      sessionId: 1,
      status: "error",
      error: { code: "RETRYABLE", message: "retry", failedTopicIds: [] },
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
      level: "warning",
      message: "旧会话正在退出",
    });
    expect(store.logs.some((log) => log.message.includes("新同步尝试"))).toBe(
      false,
    );

    stopping.resolve();
    await retrying;
    const oldLogIndex = store.logs.findIndex(
      (log) => log.message === "旧会话正在退出",
    );
    const separatorIndex = store.logs.findIndex((log) =>
      log.message.includes("新同步尝试"),
    );
    expect(oldLogIndex).toBeGreaterThanOrEqual(0);
    expect(separatorIndex).toBeGreaterThan(oldLogIndex);
  });

  it("copies bounded diagnostics without tokens or absolute paths", async () => {
    const store = useSyncSessionStore();
    store.open();
    store.activeSessionId = 51;
    emitTauriEvent("vcp-sync-status", {
      sessionId: 51,
      status: "error",
      error: {
        code: "UPLOAD_token=code-secret_/root/code path/error",
        message:
          'Bearer secret-token; Bearer "alpha beta"; token=also-secret; C:\\Users\\me with space\\file.txt; upload failed at /home/me/file.txt; file:///mnt/private/cache.bin',
        failedTopicIds: [
          "topic-a_sync_token=id-secret",
          "/mnt/private folder/topic-b",
          "/Users/me/topic-c",
          "ERR(/root/private/file)",
          "topic-/Users/me/secret",
        ],
      },
    });

    await store.copyDiagnostics();

    const writeText = vi.mocked(navigator.clipboard.writeText);
    const diagnostic = String(
      writeText.mock.calls[writeText.mock.calls.length - 1]?.[0],
    );
    expect(diagnostic).toContain("VCP Mobile: 1.1.4");
    expect(diagnostic).toContain("Wire protocol: 1.1");
    expect(diagnostic).toContain("Session: 51");
    expect(diagnostic).not.toContain("secret-token");
    expect(diagnostic).not.toContain("also-secret");
    expect(diagnostic).not.toContain("alpha beta");
    expect(diagnostic).not.toContain("code-secret");
    expect(diagnostic).not.toContain("id-secret");
    expect(diagnostic).not.toContain("C:\\Users");
    expect(diagnostic).not.toContain("/home/me");
    expect(diagnostic).not.toContain("/root");
    expect(diagnostic).not.toContain("/mnt");
    expect(diagnostic).not.toContain("/Users");
    expect(diagnostic).not.toContain("private/file");
  });
});
