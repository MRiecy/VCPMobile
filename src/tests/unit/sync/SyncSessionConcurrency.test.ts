import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { mount } from "@vue/test-utils";
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

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: vi.fn(() => Promise.resolve("1.1.5")),
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
    setActivePinia(createPinia());
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
      error: syncError("CURRENT"),
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
        error: syncError("TEST"),
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
      await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(4));

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

  it("parses structured command errors without exposing the transport detail", async () => {
    const commandError = {
      code: "TOKEN_MISMATCH",
      category: "configuration",
      origin: "mobile_sync",
      stage: "connect",
      retryAction: "after_user_action",
      message: "手机端与电脑端的同步令牌不一致",
      guidance: "重新核对两端令牌后再试。",
      failedTopicIds: [],
      logFile: null,
    };
    mockInvoke("start_manual_sync", () =>
      Promise.reject(`SYNC_ERROR:${JSON.stringify(commandError)}`),
    );

    const store = useSyncSessionStore();
    store.open();
    await store.startSync();

    expect(store.status).toBe("error");
    expect(store.terminalError).toMatchObject(commandError);
    expect(store.logs.some((log) => log.message.includes("SYNC_ERROR"))).toBe(
      false,
    );
  });

  it("fails closed when a legacy raw terminal error lacks the safe copy contract", () => {
    const store = useSyncSessionStore();
    store.open();
    store.activeSessionId = 49;

    emitTauriEvent("vcp-sync-status", {
      sessionId: 49,
      status: "error",
      error: {
        code: "TOKEN_MISMATCH",
        message: "raw token=secret-value from transport",
        failedTopicIds: [],
      },
    });

    expect(store.terminalError?.code).toBe("SYNC_ATTEMPT_FAILED");
    expect(store.terminalError?.message).toBe("同步未能完成");
    expect(store.logs.some((log) => log.message.includes("secret-value"))).toBe(
      false,
    );
  });

  it("shows only owned operator notices and keeps numeric progress nonterminal", async () => {
    const store = useSyncSessionStore();
    store.open();
    await store.startSync();

    emitTauriEvent("vcp-log", {
      category: "sync",
      level: "error",
      message: "raw diagnostic token=secret-value",
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
      phase: "internal_secret_phase",
      total: 2,
      completed: 2,
      message: "raw progress token=phase-secret",
    });

    expect(store.logs.some((log) => log.message.includes("secret-value"))).toBe(
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
    expect(store.logs.some((log) => log.message.includes("phase-secret"))).toBe(
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

  it("renders fixed copy plus the safe stage, origin, and code tuple", async () => {
    const store = useSyncSessionStore();
    store.open();
    store.activeSessionId = 48;
    emitTauriEvent("vcp-sync-status", {
      sessionId: 48,
      status: "error",
      error: syncError(
        "DESKTOP_DB_SECRET_CODE",
        "部分数据未能完成处理，系统未将其标记为成功",
        ["private-topic-id"],
      ),
    });

    const wrapper = mount(SyncSessionView);
    await Promise.resolve();

    expect(wrapper.text()).toContain("部分数据未能完成处理，系统未将其标记为成功");
    expect(wrapper.text()).toContain("可重试一次；若仍失败，请保留最新同步日志。");
    expect(wrapper.text()).toContain("详细记录已保存至历史日志");
    expect(wrapper.text()).toContain("消息同步 · 电脑同步插件 · DESKTOP_DB_SECRET_CODE");
    expect(wrapper.text()).not.toContain("private-topic-id");
    expect(wrapper.text()).toContain("重新同步");
  });

  it("uses the contract retry action instead of offering every terminal error a retry", async () => {
    const store = useSyncSessionStore();
    store.open();
    store.activeSessionId = 52;
    emitTauriEvent("vcp-sync-status", {
      sessionId: 52,
      status: "error",
      error: {
        ...syncError("PLUGIN_VERSION_MISMATCH"),
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

  it("shows a fixed history error instead of the raw command failure", async () => {
    let attempts = 0;
    mockInvoke("list_sync_log_files", () => {
      attempts += 1;
      return attempts === 1
        ? Promise.reject("SYNC_ERROR:raw token=history-secret")
        : [];
    });
    const wrapper = mount(SyncLogBrowserCore);

    await vi.waitFor(() =>
      expect(wrapper.text()).toContain("无法加载同步日志，请稍后再试。"),
    );
    expect(wrapper.text()).not.toContain("history-secret");
    await wrapper.get("button").trigger("click");
    await vi.waitFor(() => expect(wrapper.text()).toContain("暂无同步日志"));
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
        ? Promise.reject("SYNC_ERROR:raw token=read-secret")
        : "[2026-08-13T12:00:00.000+08:00] [INFO] retry succeeded";
    });
    const notifications = useNotificationStore();
    const wrapper = mount(SyncLogBrowserCore);

    await vi.waitFor(() =>
      expect(wrapper.text()).toContain("20260813_120000_000_1_sync.log"),
    );
    await wrapper.get('[class*="cursor-pointer"]').trigger("click");
    await vi.waitFor(() =>
      expect(
        notifications.activeToasts[notifications.activeToasts.length - 1]
          ?.message,
      ).toBe("无法打开此同步日志，请稍后再试"),
    );
    expect(wrapper.text()).toContain("20260813_120000_000_1_sync.log");
    expect(wrapper.text()).not.toContain("read-secret");

    await wrapper.get('[class*="cursor-pointer"]').trigger("click");
    await vi.waitFor(() => expect(wrapper.text()).toContain("retry succeeded"));
  });

  it("copies bounded diagnostics without tokens or absolute paths", async () => {
    const store = useSyncSessionStore();
    store.open();
    store.activeSessionId = 51;
    emitTauriEvent("vcp-sync-status", {
      sessionId: 51,
      status: "error",
      error: {
        code: "UPLOAD_FAILED",
        category: "data",
        origin: "desktop_plugin",
        stage: "messages",
        retryAction: "manual",
        message:
          'Bearer secret-token; Bearer "alpha beta"; token=also-secret; C:\\Users\\me with space\\file.txt; upload failed at /home/me/file.txt; file:///mnt/private/cache.bin',
        guidance: "可重试一次；若仍失败，请保留最新同步日志。",
        failedTopicIds: [
          "topic-a_sync_token=id-secret",
          "/mnt/private folder/topic-b",
          "/Users/me/topic-c",
          "ERR(/root/private/file)",
          "topic-/Users/me/secret",
        ],
        logFile: "20260813_120000_000_51_sync.log",
      },
    });

    await store.copyDiagnostics();

    const writeText = vi.mocked(navigator.clipboard.writeText);
    const diagnostic = String(
      writeText.mock.calls[writeText.mock.calls.length - 1]?.[0],
    );
    expect(diagnostic).toContain("VCP Mobile: 1.1.5");
    expect(diagnostic).toMatch(/Wire protocol: \d+\.\d+/);
    expect(diagnostic).toContain("Session: 51");
    expect(diagnostic).not.toContain("secret-token");
    expect(diagnostic).not.toContain("also-secret");
    expect(diagnostic).not.toContain("alpha beta");
    expect(diagnostic).not.toContain("id-secret");
    expect(diagnostic).not.toContain("C:\\Users");
    expect(diagnostic).not.toContain("/home/me");
    expect(diagnostic).not.toContain("/root");
    expect(diagnostic).not.toContain("/mnt");
    expect(diagnostic).not.toContain("/Users");
    expect(diagnostic).not.toContain("private/file");
    expect(diagnostic).toContain("Error code: UPLOAD_FAILED");
    expect(diagnostic).toContain("Error origin: desktop_plugin");
    expect(diagnostic).toContain("Error stage: messages");
    expect(diagnostic).toContain("Retry action: manual");
    expect(diagnostic).toContain("Failed topic IDs:");
    expect(diagnostic).toContain("[path]");
    expect(diagnostic).toContain("20260813_120000_000_51_sync.log");
  });
});
