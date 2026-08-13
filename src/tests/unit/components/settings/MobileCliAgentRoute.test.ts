import { afterEach, describe, expect, it, vi } from "vitest";
import { nextTick } from "vue";
import { createPinia, setActivePinia } from "pinia";
import AiLogicSettingsSection from "@/features/settings/components/AiLogicSettingsSection.vue";
import SettingsView from "@/features/settings/SettingsView.vue";
import {
  normalizeAppSettings,
  useSettingsStore,
  type AppSettings,
} from "@/core/stores/settings";
import { invokeMock, listenMock, mockInvoke } from "@/tests/mocks/tauri";
import { mountWithPinia } from "@/tests/utils/mount";
import { flushPromises } from "@/tests/utils/flush";

const baseSettings = (
  overrides: Partial<AppSettings> = {},
): AppSettings => ({
  userName: "User",
  vcpServerUrl: "",
  vcpApiKey: "",
  vcpLogUrl: "ws://log-only.invalid",
  vcpLogKey: "log-key-only",
  syncServerUrl: "",
  syncHttpUrl: "",
  syncToken: "",
  topicSummaryModel: "model-a",
  syncLogLevel: "INFO",
  agentOrder: [],
  groupOrder: [],
  mobileCliAgentRoute: "localLoopback",
  ...overrides,
});

const disconnectedStatus = {
  state: "disconnected",
  connected: false,
  server_id: null,
  client_id: null,
  registered_tools: 0,
  last_error: null,
  session_id: 0,
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

describe("mobile CLI Agent route settings", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("normalizes absent or unknown persisted values to localLoopback", () => {
    const { mobileCliAgentRoute: _route, ...missing } = baseSettings();

    expect(normalizeAppSettings(missing as AppSettings).mobileCliAgentRoute).toBe(
      "localLoopback",
    );
    expect(
      normalizeAppSettings(
        baseSettings({ mobileCliAgentRoute: "vcpPlugin" }),
      ).mobileCliAgentRoute,
    ).toBe("vcpPlugin");
    expect(
      normalizeAppSettings({
        ...baseSettings(),
        mobileCliAgentRoute: "unexpected",
      } as unknown as AppSettings).mobileCliAgentRoute,
    ).toBe("localLoopback");
  });

  it("shows a read-only VCP preflight without treating VCPLog as Distributed config", async () => {
    mockInvoke("get_distributed_status", () => disconnectedStatus);
    mockInvoke("get_registered_tools_metadata", () => []);

    const settings = baseSettings({ mobileCliAgentRoute: "vcpPlugin" });
    const wrapper = mountWithPinia(AiLogicSettingsSection, {
      props: { settings },
    });
    await vi.waitFor(() => {
      expect(wrapper.get("[data-mobile-cli-preflight]").text()).toContain(
        "本机未扫描到 VCPMobileCLI",
      );
    });

    expect(wrapper.get("[data-mobile-cli-preflight]").text()).toContain(
      "缺少地址或密钥",
    );
    expect(settings.distributedEnabled).toBeUndefined();
    expect(
      invokeMock.mock.calls.some(([command]) =>
        [
          "update_enabled_tools",
          "update_settings",
          "start_distributed",
          "connect_distributed",
        ].includes(command),
      ),
    ).toBe(false);

    wrapper.unmount();
  });

  it("invalidates a pending preflight and releases its Distributed listener exactly once", async () => {
    const catalog = deferred<never[]>();
    const unlisten = vi.fn();
    listenMock.mockResolvedValueOnce(unlisten);
    mockInvoke("get_distributed_status", () => disconnectedStatus);
    mockInvoke("get_registered_tools_metadata", () => catalog.promise);

    const wrapper = mountWithPinia(AiLogicSettingsSection, {
      props: {
        settings: baseSettings({ mobileCliAgentRoute: "vcpPlugin" }),
      },
    });
    await vi.waitFor(() => {
      expect(
        invokeMock.mock.calls.some(
          ([command]) => command === "get_registered_tools_metadata",
        ),
      ).toBe(true);
    });

    wrapper.unmount();
    catalog.resolve([]);
    await flushPromises();

    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("patches only the route and preserves unrelated local edits across the backend snapshot", async () => {
    mockInvoke("read_settings", () => baseSettings());
    mockInvoke("get_settings_recovery_status", () => ({
      recoveredCorrupt: false,
    }));
    mockInvoke("get_distributed_status", () => disconnectedStatus);
    mockInvoke("get_registered_tools_metadata", () => []);
    mockInvoke("update_settings", (args) => {
      const updates = args?.updates as Partial<AppSettings>;
      if (updates.mobileCliAgentRoute) {
        return baseSettings({
          userName: "Server User",
          mobileCliAgentRoute: updates.mobileCliAgentRoute,
        });
      }
      return baseSettings({
        userName: updates.userName,
        mobileCliAgentRoute: "vcpPlugin",
      });
    });

    const wrapper = mountWithPinia(SettingsView, {
      props: { isOpen: true },
      global: { stubs: { transition: false } },
    });
    await vi.waitFor(() => {
      expect(wrapper.findAll(".settings-row").length).toBeGreaterThan(0);
    });
    const advancedRow = wrapper
      .findAll(".settings-row")
      .find((row) => row.text().includes("高级功能"));
    await advancedRow!.trigger("click");
    await vi.dynamicImportSettled();
    await flushPromises();

    const routeSection = wrapper.findComponent(AiLogicSettingsSection);
    expect(routeSection.exists()).toBe(true);
    const localSettings = routeSection.props("settings") as AppSettings;
    localSettings.userName = "Draft User";

    await wrapper
      .get('button[data-mobile-cli-route="vcpPlugin"]')
      .trigger("click");
    await vi.waitFor(() => {
      expect(invokeMock.mock.calls.filter(([command]) => command === "update_settings")).toHaveLength(1);
    });

    const routeSave = invokeMock.mock.calls.find(
      ([command]) => command === "update_settings",
    );
    expect(routeSave?.[1]).toEqual({
      updates: { mobileCliAgentRoute: "vcpPlugin" },
    });
    expect(
      (routeSection.props("settings") as AppSettings).userName,
    ).toBe("Draft User");
    expect(useSettingsStore().settings?.userName).toBe("Server User");

    await wrapper.get("header button").trigger("click");
    await vi.waitFor(() => {
      expect(invokeMock.mock.calls.filter(([command]) => command === "update_settings")).toHaveLength(2);
    });
    const laterSave = invokeMock.mock.calls.filter(
      ([command]) => command === "update_settings",
    )[1];
    expect(laterSave?.[1]).toEqual({ updates: { userName: "Draft User" } });

    wrapper.unmount();
  });

  it("rolls a failed route save back without toggling adjacent capabilities", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    mockInvoke("read_settings", () => baseSettings());
    mockInvoke("get_settings_recovery_status", () => ({
      recoveredCorrupt: false,
    }));
    mockInvoke("get_distributed_status", () => disconnectedStatus);
    mockInvoke("get_registered_tools_metadata", () => []);
    mockInvoke("update_settings", () =>
      Promise.reject(new Error("settings write failed")),
    );

    setActivePinia(createPinia());
    const wrapper = mountWithPinia(SettingsView, {
      props: { isOpen: true },
      global: { stubs: { transition: false } },
    });
    await vi.waitFor(() => {
      expect(wrapper.findAll(".settings-row").length).toBeGreaterThan(0);
    });

    const advancedRow = wrapper
      .findAll(".settings-row")
      .find((row) => row.text().includes("高级功能"));
    expect(advancedRow).toBeDefined();
    await advancedRow!.trigger("click");
    await nextTick();
    await vi.dynamicImportSettled();
    await flushPromises();

    const remoteRoute = wrapper.get(
      'button[data-mobile-cli-route="vcpPlugin"]',
    );
    await remoteRoute.trigger("click");
    await vi.waitFor(() => {
      expect(
        wrapper
          .get('button[data-mobile-cli-route="localLoopback"]')
          .attributes("aria-checked"),
      ).toBe("true");
    });

    const updateCall = invokeMock.mock.calls.find(
      ([command]) => command === "update_settings",
    );
    expect(updateCall?.[1]).toEqual({
      updates: { mobileCliAgentRoute: "vcpPlugin" },
    });
    expect(wrapper.get('[role="alert"]').text()).toContain("已恢复此前选择");

    const forbiddenCommands = new Set([
      "update_enabled_tools",
      "start_distributed",
      "connect_distributed",
      "enable_vcp_tool_injection",
    ]);
    expect(
      invokeMock.mock.calls.filter(([command]) =>
        forbiddenCommands.has(command),
      ),
    ).toEqual([]);
    expect(useSettingsStore().settings?.mobileCliAgentRoute).toBe(
      "localLoopback",
    );

    wrapper.unmount();
  });
});
