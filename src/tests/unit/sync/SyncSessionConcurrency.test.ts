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

  it("shows desktop backend info only for the current connected session", async () => {
    const store = useSyncSessionStore();
    store.open();
    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(4));
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

  it("renders CDS compile command and copies to clipboard on CDS/Wire errors", async () => {
    const store = useSyncSessionStore();
    store.isOpen = true;
    store.status = "error";
    store.terminalError = {
      code: "CDS_PROTOCOL_MISMATCH",
      category: "compatibility",
      origin: "desktop_cds",
      stage: "startup",
      retryAction: "after_user_action",
      message: "电脑端 CDS 数据服务协议版本不匹配",
      guidance: "请更新 VChat 桌面端以同步插件代码，并在 VCPChat 根目录执行 node rust_chat_data_service/build-runtime.js 重新编译 CDS，重启电脑端后再试。",
      failedTopicIds: [],
      logFile: null,
    };

    const wrapper = mount(SyncSessionView);
    await Promise.resolve();

    expect(wrapper.text()).toContain("node rust_chat_data_service/build-runtime.js");
    expect(wrapper.text()).toContain("重新编译命令");

    const copyBtn = wrapper.findAll("button").find((b) => b.text().includes("复制命令"));
    expect(copyBtn).toBeTruthy();

    await copyBtn!.trigger("click");
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith("node rust_chat_data_service/build-runtime.js");

    const notifications = useNotificationStore();
    expect(notifications.activeToasts.some((t) => t.message === "已复制 CDS 编译命令")).toBe(true);
  });
});
