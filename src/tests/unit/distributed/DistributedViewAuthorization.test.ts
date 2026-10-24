import { flushPromises } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import DistributedView from "@/features/distributed/DistributedView.vue";
import { useNotificationStore } from "@/core/stores/notification";
import { useSettingsStore } from "@/core/stores/settings";
import { emitTauriEvent, invokeMock, mockInvoke } from "@/tests/mocks/tauri";
import { mountWithPinia } from "@/tests/utils/mount";
import type { ConfigSaveResult, ConfigUpdate } from "@/core/config/generated/config";
import { applyConfigRequest, configRequestChanges, makeConfigSnapshot, makeSettings } from "@/tests/helpers/config";

const testPlatform = vi.hoisted(() => ({ value: 'android' as 'android' | 'ios' }));
vi.mock("@/core/utils/platform", () => ({
  get nativePlatform() { return testPlatform.value; },
  get isAndroid() { return testPlatform.value === 'android'; },
  get isIOS() { return testPlatform.value === 'ios'; },
}));

interface ToolFixture {
  name: string;
  display_name?: string;
  description?: string;
  category?: "oneshot" | "interactive" | "streaming";
  enabled?: boolean;
  requiresRoot?: boolean;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const disconnectedStatus = {
  state: "disconnected",
  connected: false,
  server_id: null,
  client_id: null,
  registered_tools: 0,
  last_error: null,
  session_id: 0,
};

function installBaseMocks(tools: ToolFixture[]) {
  const snapshot = makeConfigSnapshot(makeSettings({
    "identity.userName": "tester",
    "server.url": "https://legacy.invalid",
    "server.apiKey": "legacy-api-key",
    "log.url": "wss://log.invalid",
    "log.key": "legacy-log-key",
    "conversation.topicSummaryModel": "",
    "distributed.wsUrl": "wss://log.invalid",
    "distributed.vcpKey": "legacy-log-key",
    "distributed.deviceName": "VCPMobile",
  }));
  mockInvoke("read_settings", () => snapshot);
  mockInvoke("get_distributed_status", () => disconnectedStatus);
  mockInvoke("get_registered_tools_metadata", () => tools);
  mockInvoke("get_distributed_tool_config_status", () => ({
    state: "ready",
    message: null,
  }));
  return snapshot;
}

async function mountOpenView() {
  const wrapper = mountWithPinia(DistributedView, {
    props: { isOpen: true },
  });
  await wrapper.get('[data-distributed-tab="plugins"]').trigger("click");
  return wrapper;
}

async function waitForCatalog() {
  await vi.waitFor(() => {
    expect(
      invokeMock.mock.calls.some(
        ([command]) => command === "get_registered_tools_metadata",
      ),
    ).toBe(true);
  });
  await flushPromises();
}

beforeEach(() => {
  testPlatform.value = 'android';
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("DistributedView explicit tool authorization", () => {
  it("explains iOS Root support without requesting native Root access", async () => {
    testPlatform.value = 'ios';
    installBaseMocks([]);

    const wrapper = mountWithPinia(DistributedView, { props: { isOpen: true } });
    try {
      await flushPromises();
      const rootButton = wrapper.findAll('button').find(button => button.text() === 'Root 说明');
      expect(rootButton).toBeDefined();
      await rootButton!.trigger('click');

      expect(invokeMock.mock.calls.some(([command]) => [
        'plugin:vcp-mobile|check_root_access',
        'plugin:vcp-mobile|launch_root_manager',
        'plugin:vcp-mobile|run_root_command',
      ].includes(command))).toBe(false);
      expect(useNotificationStore().activeToasts).toHaveLength(1);
      expect(useNotificationStore().activeToasts[0].message).toContain('iOS');
    } finally {
      wrapper.unmount();
    }
  });

  it("waits for loaded connection fields and enables the iOS node in one save", async () => {
    testPlatform.value = 'ios';
    installBaseMocks([]);
    const loaded = makeConfigSnapshot(makeSettings({
      'log.url': 'wss://configured.example/ws',
      'log.key': 'configured-key',
      'distributed.deviceName': 'VCPMobile',
      'distributed.wsUrl': 'wss://configured.example/ws',
      'distributed.vcpKey': 'configured-key',
    }));
    const read = deferred<typeof loaded>();
    const save = deferred<ConfigSaveResult>();
    mockInvoke('read_settings', () => read.promise);
    mockInvoke('update_settings', () => save.promise);
    const wrapper = mountWithPinia(DistributedView, { props: { isOpen: true } });
    try {
      await flushPromises();
      const button = wrapper.findAll('button').find(item => item.text() === '连接')!;
      expect(button.attributes()).toHaveProperty('disabled');
      await button.trigger('click');
      expect(invokeMock.mock.calls.some(([command]) => command === 'update_settings')).toBe(false);

      read.resolve(loaded);
      await flushPromises();
      expect(button.attributes()).not.toHaveProperty('disabled');
      await button.trigger('click');
      await flushPromises();
      expect(button.attributes()).toHaveProperty('disabled');
      await button.trigger('click');
      const writes = invokeMock.mock.calls.filter(([command]) => command === 'update_settings');
      expect(writes).toHaveLength(1);
      const request = writes[0][1]?.request as ConfigUpdate;
      expect(configRequestChanges(request)).toEqual({
        'distributed.deviceName': 'VCPMobile',
        'distributed.wsUrl': 'wss://configured.example/ws',
        'distributed.vcpKey': 'configured-key',
        'distributed.enabled': true,
      });
      expect(request.patches).toEqual([expect.objectContaining({
        domain: 'distributed', expectedRevision: '1',
        baseValues: { deviceName: 'VCPMobile', wsUrl: 'wss://configured.example/ws', vcpKey: 'configured-key', enabled: false },
      })]);

      save.resolve({ status: 'saved', snapshot: applyConfigRequest(loaded, request) });
      await flushPromises();
      expect(useNotificationStore().activeToasts.filter(notice => notice.type === 'success')).toHaveLength(1);
      expect(button.attributes()).not.toHaveProperty('disabled');
    } finally {
      wrapper.unmount();
    }
  });

  it("keeps failed connection loading unsaveable until an explicit reload succeeds", async () => {
    testPlatform.value = 'ios';
    installBaseMocks([]);
    vi.spyOn(console, 'error').mockImplementation(() => {});
    const fresh = makeConfigSnapshot(makeSettings({
      'log.url': 'wss://reloaded.example/ws',
      'log.key': 'reloaded-key',
      'distributed.deviceName': 'VCPMobile',
      'distributed.wsUrl': 'wss://reloaded.example/ws',
      'distributed.vcpKey': 'reloaded-key',
    }), '2');
    mockInvoke('update_settings', args => ({
      status: 'saved', snapshot: applyConfigRequest(fresh, args?.request as ConfigUpdate),
    }));
    const wrapper = mountWithPinia(DistributedView, { props: { isOpen: true } });
    try {
      await flushPromises();
      expect(useSettingsStore().settings?.distributed.deviceName).toBe('VCPMobile');
      await wrapper.setProps({ isOpen: false });
      mockInvoke('read_settings', async () => { throw new Error('read unavailable'); });
      await wrapper.setProps({ isOpen: true });
      await flushPromises();
      expect(wrapper.get('[role="alert"]').text()).toContain('加载失败');
      const saveButton = wrapper.findAll('button').find(item => item.text() === '连接')!;
      expect(saveButton.attributes()).toHaveProperty('disabled');
      await saveButton.trigger('click');
      expect(invokeMock.mock.calls.some(([command]) => command === 'update_settings')).toBe(false);

      mockInvoke('read_settings', () => fresh);
      await wrapper.findAll('button').find(item => item.text() === '重新加载')!.trigger('click');
      await flushPromises();
      expect(wrapper.find('[role="alert"]').exists()).toBe(false);
      expect(saveButton.attributes()).not.toHaveProperty('disabled');
      await saveButton.trigger('click');
      await flushPromises();
      expect(invokeMock).toHaveBeenCalledWith('update_settings', { request: { patches: [{
        domain: 'distributed', expectedRevision: '2',
        changes: { deviceName: 'VCPMobile', wsUrl: 'wss://reloaded.example/ws', vcpKey: 'reloaded-key', enabled: true },
        baseValues: { deviceName: 'VCPMobile', wsUrl: 'wss://reloaded.example/ws', vcpKey: 'reloaded-key', enabled: false },
      }] } });
    } finally {
      wrapper.unmount();
    }
  });

  it("retries a disconnected enabled node after an unchanged save completes", async () => {
    const snapshot = installBaseMocks([]);
    snapshot.data.distributed.enabled = true;
    const save = deferred<ConfigSaveResult>();
    mockInvoke('update_settings', () => save.promise);
    mockInvoke('reconcile_distributed_node_cmd', () => undefined);
    const wrapper = mountWithPinia(DistributedView, { props: { isOpen: true } });
    try {
      await flushPromises();
      const button = wrapper.findAll('button').find(item => item.text() === '连接')!;
      await button.trigger('click');
      await flushPromises();
      expect(invokeMock).toHaveBeenCalledWith('update_settings', { request: { patches: [{
        domain: 'distributed', expectedRevision: '1',
        changes: { wsUrl: 'wss://log.invalid', vcpKey: 'legacy-log-key', deviceName: 'VCPMobile', enabled: true },
        baseValues: { wsUrl: 'wss://log.invalid', vcpKey: 'legacy-log-key', deviceName: 'VCPMobile', enabled: true },
      }] } });
      expect(invokeMock).not.toHaveBeenCalledWith('reconcile_distributed_node_cmd');

      // An unchanged successful commit does not increment the domain revision.
      save.resolve({ status: 'saved', snapshot });
      await flushPromises();
      expect(useSettingsStore().snapshot?.domains.distributed.revision).toBe('1');
      expect(invokeMock.mock.calls.filter(([command]) => command === 'reconcile_distributed_node_cmd')).toHaveLength(1);
      expect(button.attributes()).not.toHaveProperty('disabled');
    } finally {
      wrapper.unmount();
    }
  });

  it("does not start a connection when its configuration is rejected", async () => {
    installBaseMocks([]);
    vi.spyOn(console, 'error').mockImplementation(() => {});
    mockInvoke('update_settings', () => ({ status: 'rejected', issues: [{
      domain: 'distributed', field: 'wsUrl', code: 'invalid_url', message: '连接地址格式无效',
    }] }));
    const wrapper = mountWithPinia(DistributedView, { props: { isOpen: true } });
    try {
      await flushPromises();
      const button = wrapper.findAll('button').find(item => item.text() === '连接')!;
      await button.trigger('click');
      await flushPromises();
      expect(invokeMock).not.toHaveBeenCalledWith('reconcile_distributed_node_cmd');
      expect(useSettingsStore().settings?.distributed.enabled).toBe(false);
      expect(useNotificationStore().activeToasts).toEqual([expect.objectContaining({
        type: 'error', title: '连接设置保存失败', message: expect.stringContaining('连接地址格式无效'),
      })]);
      expect(button.attributes()).not.toHaveProperty('disabled');
    } finally {
      wrapper.unmount();
    }
  });

  it("reports startup failure separately after the connection configuration was saved", async () => {
    const snapshot = installBaseMocks([]);
    vi.spyOn(console, 'error').mockImplementation(() => {});
    mockInvoke('update_settings', args => ({
      status: 'saved', snapshot: applyConfigRequest(snapshot, args?.request as ConfigUpdate),
    }));
    mockInvoke('reconcile_distributed_node_cmd', () => Promise.reject(new Error('runtime unavailable')));
    const wrapper = mountWithPinia(DistributedView, { props: { isOpen: true } });
    try {
      await flushPromises();
      const button = wrapper.findAll('button').find(item => item.text() === '连接')!;
      await button.trigger('click');
      await flushPromises();
      expect(useSettingsStore().settings?.distributed.enabled).toBe(true);
      expect(useSettingsStore().snapshot?.domains.distributed.revision).toBe('2');
      expect(useNotificationStore().activeToasts.filter(notice => notice.type === 'error')).toEqual([
        expect.objectContaining({ title: '设置已保存，连接未能启动' }),
      ]);
      expect(button.attributes()).not.toHaveProperty('disabled');
    } finally {
      wrapper.unmount();
    }
  });

  it("renders a stable fail-closed catalog independently of settings and config failures", async () => {
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});
    const configStatus = deferred<{ state: "ready"; message: null }>();
    installBaseMocks([]);
    mockInvoke("read_settings", () =>
      Promise.reject(new Error("settings offline")),
    );
    mockInvoke("get_registered_tools_metadata", () => [
      { name: "Zulu", enabled: true, category: "oneshot" },
      { name: "Alpha", category: "streaming" },
    ]);
    mockInvoke(
      "get_distributed_tool_config_status",
      () => configStatus.promise,
    );

    const wrapper = await mountOpenView();
    await waitForCatalog();

    expect(
      wrapper
        .findAll("[data-tool-row]")
        .map((row) => row.attributes("data-tool-row")),
    ).toEqual(["Alpha", "Zulu"]);
    expect(
      wrapper.get('[data-tool-switch="Alpha"]').attributes("aria-checked"),
    ).toBe("false");
    expect(
      wrapper.get('[data-tool-switch="Alpha"]').attributes(),
    ).toHaveProperty("disabled");
    expect(wrapper.get("[data-tool-config-status]").text()).toContain(
      "目录可独立浏览",
    );
    configStatus.reject(new Error("config unavailable"));
    await vi.waitFor(() => {
      expect(wrapper.get("[data-tool-config-status]").text()).toContain(
        "当前仅可浏览目录",
      );
    });
    expect(wrapper.get("[data-tool-config-status]").text()).toContain(
      "当前仅可浏览目录",
    );
    expect(wrapper.find("[data-tool-catalog-error]").exists()).toBe(false);
    expect(consoleError).toHaveBeenCalled();

    wrapper.unmount();
  });


  it("uses one mutation owner, does not optimistically flip, and rescans after success", async () => {
    const update = deferred<void>();
    let catalogReads = 0;
    installBaseMocks([]);
    mockInvoke("get_registered_tools_metadata", () => {
      catalogReads += 1;
      return catalogReads === 1
        ? [
            { name: "Alpha", category: "streaming", enabled: false },
            { name: "Beta", category: "streaming", enabled: true },
          ]
        : [
            { name: "Alpha", category: "streaming", enabled: true },
            { name: "Beta", category: "streaming", enabled: true },
          ];
    });
    mockInvoke("update_enabled_tools", () => update.promise);

    const wrapper = await mountOpenView();
    await waitForCatalog();
    const alpha = wrapper.get('[data-tool-switch="Alpha"]');
    await alpha.trigger("click");

    expect(alpha.attributes("aria-checked")).toBe("false");
    expect(
      wrapper
        .findAll('[role="switch"]')
        .every((item) => item.attributes().disabled !== undefined),
    ).toBe(true);
    expect(
      invokeMock.mock.calls.filter(
        ([command]) => command === "update_enabled_tools",
      ),
    ).toEqual([["update_enabled_tools", { enabledNames: ["Alpha", "Beta"] }]]);

    await wrapper.get('[data-tool-switch="Beta"]').trigger("click");
    expect(
      invokeMock.mock.calls.filter(
        ([command]) => command === "update_enabled_tools",
      ),
    ).toHaveLength(1);

    update.resolve();
    await vi.waitFor(() => {
      expect(
        wrapper.get('[data-tool-switch="Alpha"]').attributes("aria-checked"),
      ).toBe("true");
      expect(catalogReads).toBe(2);
    });
    expect(
      wrapper
        .findAll('[role="switch"]')
        .every((item) => item.attributes().disabled === undefined),
    ).toBe(true);

    wrapper.unmount();
  });

  it("keeps the original state and reports a failed authorization write", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    installBaseMocks([
      { name: "Alpha", category: "streaming", enabled: false },
    ]);
    mockInvoke("update_enabled_tools", () =>
      Promise.reject(new Error("disk full")),
    );

    const wrapper = await mountOpenView();
    await waitForCatalog();
    await wrapper.get('[data-tool-switch="Alpha"]').trigger("click");
    await flushPromises();

    const alpha = wrapper.get('[data-tool-switch="Alpha"]');
    expect(alpha.attributes("aria-checked")).toBe("false");
    expect(alpha.attributes()).not.toHaveProperty("disabled");
    const toasts = useNotificationStore().activeToasts;
    expect(toasts[toasts.length - 1]?.title).toBe("工具授权未保存");

    wrapper.unmount();
  });

  it("drops a late scan after the view is closed and reopened", async () => {
    const staleScan = deferred<ToolFixture[]>();
    let catalogReads = 0;
    installBaseMocks([]);
    mockInvoke("get_registered_tools_metadata", () => {
      catalogReads += 1;
      if (catalogReads === 1) {
        return [{ name: "Initial", category: "streaming", enabled: false }];
      }
      if (catalogReads === 2) return staleScan.promise;
      return [{ name: "Current", category: "streaming", enabled: false }];
    });

    const wrapper = await mountOpenView();
    await waitForCatalog();
    await wrapper.get("[data-tool-catalog-retry]").trigger("click");
    await vi.waitFor(() => expect(catalogReads).toBe(2));

    await wrapper.setProps({ isOpen: false });
    await wrapper.setProps({ isOpen: true });
    await wrapper.get('[data-distributed-tab="plugins"]').trigger("click");
    await vi.waitFor(() => {
      expect(wrapper.find('[data-tool-row="Current"]').exists()).toBe(true);
    });

    staleScan.resolve([
      { name: "Stale", category: "streaming", enabled: true },
    ]);
    await flushPromises();
    expect(wrapper.find('[data-tool-row="Current"]').exists()).toBe(true);
    expect(wrapper.find('[data-tool-row="Stale"]').exists()).toBe(false);

    wrapper.unmount();
  });

  it("drops an older config result after a newer scan in the same view", async () => {
    const staleStatus = deferred<{
      state: "recovered_disabled";
      message: string;
    }>();
    let statusReads = 0;
    installBaseMocks([
      { name: "Alpha", category: "streaming", enabled: false },
    ]);
    mockInvoke("get_distributed_tool_config_status", () => {
      statusReads += 1;
      if (statusReads === 1) return staleStatus.promise;
      return { state: "ready", message: null };
    });

    const wrapper = await mountOpenView();
    await waitForCatalog();
    expect(wrapper.get("[data-tool-config-status]").text()).toContain(
      "目录可独立浏览",
    );

    await wrapper.get("[data-tool-catalog-retry]").trigger("click");
    await vi.waitFor(() => {
      expect(statusReads).toBe(2);
      expect(wrapper.find("[data-tool-config-status]").exists()).toBe(false);
    });

    staleStatus.resolve({
      state: "recovered_disabled",
      message: "stale recovery",
    });
    await flushPromises();
    expect(wrapper.find("[data-tool-config-status]").exists()).toBe(false);
    expect(
      useNotificationStore().activeToasts.some(
        (toast) => toast.title === "分布式工具已安全关闭",
      ),
    ).toBe(false);

    wrapper.unmount();
  });

  it.each(['android', 'ios'] as const)("%s expands every tool without execution and reads only an enabled streaming snapshot", async (platform) => {
    testPlatform.value = platform;
    installBaseMocks([
      {
        name: "VCPMobileCLI",
        category: "oneshot",
        enabled: true,
        description: "Run a local CLI job",
      },
      { name: "DeniedStream", category: "streaming", enabled: false },
      { name: "LiveStream", category: "streaming", enabled: true },
    ]);
    mockInvoke("execute_distributed_tool", () => '{"value":42}');

    const wrapper = await mountOpenView();
    await waitForCatalog();

    await wrapper.get('[data-tool-details="VCPMobileCLI"]').trigger("click");
    expect(wrapper.text()).toContain("需由 VCP 请求明确调用");
    expect(wrapper.find('[data-tool-read="VCPMobileCLI"]').exists()).toBe(
      false,
    );
    expect(
      invokeMock.mock.calls.some(
        ([command]) => command === "execute_distributed_tool",
      ),
    ).toBe(false);

    await wrapper.get('[data-tool-details="DeniedStream"]').trigger("click");
    expect(
      wrapper.get('[data-tool-read="DeniedStream"]').attributes(),
    ).toHaveProperty("disabled");
    await wrapper.get('[data-tool-read="DeniedStream"]').trigger("click");
    expect(
      invokeMock.mock.calls.some(
        ([command]) => command === "execute_distributed_tool",
      ),
    ).toBe(false);

    await wrapper.get('[data-tool-details="LiveStream"]').trigger("click");
    await wrapper.get('[data-tool-read="LiveStream"]').trigger("click");
    await flushPromises();
    expect(
      invokeMock.mock.calls.filter(
        ([command]) => command === "execute_distributed_tool",
      ),
    ).toEqual([["execute_distributed_tool", { name: "LiveStream" }]]);
    expect(wrapper.text()).toContain('"value": 42');

    wrapper.unmount();
  });

  it("preserves the system permission check before granting sensitive tools", async () => {
    installBaseMocks([
      { name: "MobileLocation", category: "streaming", enabled: false },
    ]);
    let permissionChecks = 0;
    mockInvoke("plugin:vcp-mobile|check_all_permissions", () => {
      permissionChecks += 1;
      return { location: false, notification: false };
    });
    mockInvoke("plugin:vcp-mobile|request_android_permission", () => undefined);
    mockInvoke("update_enabled_tools", () => undefined);

    const wrapper = await mountOpenView();
    await waitForCatalog();
    await wrapper.get('[data-tool-switch="MobileLocation"]').trigger("click");
    await flushPromises();

    expect(permissionChecks).toBe(2);
    expect(invokeMock).toHaveBeenCalledWith(
      "plugin:vcp-mobile|request_android_permission",
      { pType: "location" },
    );
    expect(
      invokeMock.mock.calls.some(
        ([command]) => command === "update_enabled_tools",
      ),
    ).toBe(false);
    expect(
      wrapper
        .get('[data-tool-switch="MobileLocation"]')
        .attributes("aria-checked"),
    ).toBe("false");

    wrapper.unmount();
  });

  it.each([false, true])("uses the acknowledged disconnect as the next connection's base without rebasing other fields (concurrent edit: %s)", async (concurrentEdit) => {
    let snapshot = installBaseMocks([]);
    snapshot.data.distributed.enabled = true;
    mockInvoke('get_distributed_status', () => ({ ...disconnectedStatus, state: 'connected', connected: true }));
    mockInvoke('update_settings', args => {
      const request = args?.request as ConfigUpdate;
      if (concurrentEdit && configRequestChanges(request)['distributed.enabled'] === false) {
        snapshot = makeConfigSnapshot(makeSettings({
          'distributed.enabled': true,
          'distributed.deviceName': 'Changed elsewhere',
          'distributed.wsUrl': 'wss://distributed.example/ws',
          'distributed.vcpKey': 'distributed-key',
        }), '2');
      }
      snapshot = applyConfigRequest(snapshot, request);
      return { status: 'saved', snapshot };
    });
    const wrapper = mountWithPinia(DistributedView, { props: { isOpen: true } });
    try {
      await flushPromises();
      await wrapper.findAll('button').find(button => button.text() === '断开连接')!.trigger('click');
      await flushPromises();
      emitTauriEvent('vcp-distributed-status', disconnectedStatus);
      await flushPromises();

      const nameInput = wrapper.get('input[placeholder="固定为 VCPMobile"]');
      expect(nameInput.attributes()).toHaveProperty('disabled');

      await wrapper.findAll('button').find(button => button.text() === '连接')!.trigger('click');
      await flushPromises();

      const writes = invokeMock.mock.calls.filter(([command]) => command === 'update_settings');
      expect(writes).toHaveLength(2);
      const request = writes[1][1]?.request as ConfigUpdate;
      expect(request.patches).toEqual([expect.objectContaining({
        domain: 'distributed', expectedRevision: '1',
        changes: expect.objectContaining({ enabled: true, deviceName: 'VCPMobile' }),
        baseValues: {
          enabled: false, deviceName: 'VCPMobile',
          wsUrl: 'wss://log.invalid', vcpKey: 'legacy-log-key',
        },
      })]);
    } finally {
      wrapper.unmount();
    }
  });

  it("can stop an iOS node while the socket is still connecting", async () => {
    testPlatform.value = 'ios';
    const snapshot = installBaseMocks([]);
    snapshot.data.distributed.enabled = true;
    mockInvoke('get_distributed_status', () => ({ ...disconnectedStatus, state: 'connecting' }));
    mockInvoke('update_settings', args => ({
      status: 'saved', snapshot: applyConfigRequest(snapshot, args?.request as ConfigUpdate),
    }));
    const wrapper = mountWithPinia(DistributedView, { props: { isOpen: true } });
    try {
      await flushPromises();
      await wrapper.findAll('button').find(button => button.text() === '断开连接')!.trigger('click');
      await flushPromises();
      expect(invokeMock).toHaveBeenCalledWith('update_settings', { request: { patches: [{
        domain: 'distributed', expectedRevision: '1',
        changes: { enabled: false }, baseValues: { enabled: true },
      }] } });
    } finally {
      wrapper.unmount();
    }
  });

  it("does not persist an iOS notification tool after authorization is denied", async () => {
    testPlatform.value = 'ios';
    installBaseMocks([{ name: 'MobileNotification', category: 'oneshot', enabled: false }]);
    mockInvoke('plugin:vcp-mobile|get_notification_permission', () => ({ authorization: 'notDetermined' }));
    mockInvoke('plugin:vcp-mobile|request_notification_permission', () => ({ authorization: 'denied' }));
    mockInvoke('plugin:vcp-mobile|open_notification_settings', () => undefined);
    const wrapper = await mountOpenView();
    try {
      await waitForCatalog();
      await wrapper.get('[data-tool-switch="MobileNotification"]').trigger('click');
      await flushPromises();
      expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|request_notification_permission');
      expect(invokeMock.mock.calls.some(([command]) => command === 'update_enabled_tools')).toBe(false);
      expect(wrapper.get('[data-tool-switch="MobileNotification"]').attributes('aria-checked')).toBe('false');
    } finally {
      wrapper.unmount();
    }
  });

  it("drops a sample that finishes after the tool was disabled", async () => {
    testPlatform.value = 'ios';
    const tool = { name: 'MobileMotionSensor', category: 'streaming' as const, enabled: true };
    installBaseMocks([tool]);
    const sample = deferred<string>();
    mockInvoke('execute_distributed_tool', () => sample.promise);
    mockInvoke('update_enabled_tools', () => { tool.enabled = false; });
    const wrapper = await mountOpenView();
    try {
      await waitForCatalog();
      await wrapper.get('[data-tool-details="MobileMotionSensor"]').trigger('click');
      await wrapper.get('[data-tool-read="MobileMotionSensor"]').trigger('click');
      await wrapper.get('[data-tool-switch="MobileMotionSensor"]').trigger('click');
      await flushPromises();
      sample.resolve('late sensor success');
      await flushPromises();
      expect(wrapper.text()).not.toContain('late sensor success');
      expect(wrapper.text()).toContain('授权后可读取样本');
    } finally {
      wrapper.unmount();
    }
  });

  it("shows catalog errors and retries without coupling to connection settings", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    let reads = 0;
    installBaseMocks([]);
    mockInvoke("get_registered_tools_metadata", () => {
      reads += 1;
      if (reads === 1) return Promise.reject(new Error("scan failed"));
      return [{ name: "Recovered", category: "streaming", enabled: false }];
    });

    const wrapper = await mountOpenView();
    await waitForCatalog();
    expect(wrapper.get("[data-tool-catalog-error]").text()).toContain(
      "扫描失败",
    );

    await wrapper.get("[data-tool-catalog-retry]").trigger("click");
    await vi.waitFor(() => {
      expect(wrapper.find('[data-tool-row="Recovered"]').exists()).toBe(true);
    });
    expect(wrapper.find("[data-tool-catalog-error]").exists()).toBe(false);

    wrapper.unmount();
  });
});
