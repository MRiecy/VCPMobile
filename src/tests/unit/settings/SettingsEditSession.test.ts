import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { isReadonly } from "vue";
import { useSettingsStore, type AppSettings } from "@/core/stores/settings";
import {
  CONNECTION_FIELDS,
  IDENTITY_FIELDS,
  SUMMARY_FIELDS,
  type SettingsEditableKey,
} from "@/features/settings/editing/fieldSets";
import { useSettingsEditSession } from "@/features/settings/editing/useSettingsEditSession";
import { mockInvoke } from "../../mocks/tauri";

const { notify } = vi.hoisted(() => ({ notify: vi.fn() }));
vi.mock("@/core/stores/notification", () => ({
  useNotificationStore: () => ({ addNotification: notify }),
}));

const initialSettings = (): AppSettings => ({
  userName: "original",
  vcpServerUrl: "https://vcp.example",
  chatEndpointMode: "standard",
  vcpApiKey: "key",
  vcpLogUrl: "",
  vcpLogKey: "",
  syncServerUrl: "",
  syncHttpUrl: "",
  syncToken: "",
  adminUsername: "admin",
  adminPassword: "password",
  fileKey: "",
  topicSummaryModel: "model-a",
  syncLogLevel: "INFO",
  agentOrder: ["agent-a"],
  groupOrder: [],
  currentThemeMode: null,
  syncPrerenderEnabled: false,
  enableAssistant: false,
  assistantAgentId: "",
  distributedEnabled: false,
  distributedWsUrl: "",
  distributedVcpKey: "",
  distributedDeviceName: "device",
});

const clone = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

interface PendingWrite {
  patch: Partial<AppSettings>;
  commit(): void;
  reject(error: Error): void;
}

function backend() {
  const database = { settings: initialSettings() };
  const writes: PendingWrite[] = [];
  mockInvoke("read_settings", () => clone(database.settings));
  mockInvoke("get_settings_recovery_status", () => ({ recoveredCorrupt: false }));
  mockInvoke("update_settings", (args) => new Promise<AppSettings>((resolve, reject) => {
    const patch = clone(args?.updates as Partial<AppSettings>);
    writes.push({
      patch,
      commit: () => {
        database.settings = { ...database.settings, ...patch };
        resolve(clone(database.settings));
      },
      reject,
    });
  }));
  const write = async (index: number): Promise<PendingWrite> => {
    await vi.waitFor(() => expect(writes.length).toBeGreaterThan(index));
    return writes[index];
  };
  return { database, writes, write };
}

const disposers: Array<() => void> = [];
function session<const Keys extends readonly SettingsEditableKey[]>(keys: Keys) {
  const result = useSettingsEditSession(keys);
  disposers.push(result.dispose);
  return result;
}

beforeEach(() => {
  setActivePinia(createPinia());
  notify.mockClear();
});

afterEach(() => {
  for (const dispose of disposers.splice(0)) dispose();
});

describe("settings edit session", () => {
  it("does not invent a draft after load failure and still allows leaving", async () => {
    const server = backend();
    mockInvoke("read_settings", () => { throw new Error("offline"); });
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const editor = session(IDENTITY_FIELDS);

    await expect(useSettingsStore().fetchSettings()).rejects.toThrow("offline");
    editor.edit("userName", "unsaved");
    expect(editor.ready.value).toBe(false);
    expect(editor.draft.value).toBeNull();
    expect((await editor.save()).ok).toBe(false);
    expect((await editor.settlePending()).ok).toBe(false);
    expect(await editor.prepareLeave()).toBe(true);
    expect(server.writes).toHaveLength(0);
  });

  it("initializes a readonly field projection when the first real snapshot arrives", async () => {
    backend();
    const editor = session(CONNECTION_FIELDS);
    expect(editor.draft.value).toBeNull();

    await useSettingsStore().fetchSettings();

    expect(editor.ready.value).toBe(true);
    expect(isReadonly(editor.draft.value)).toBe(true);
    expect(Object.keys(editor.draft.value!)).toEqual([...CONNECTION_FIELDS]);
    expect(editor.draft.value?.vcpServerUrl).toBe("https://vcp.example");
    expect(editor.dirty.value).toBe(false);
  });

  it("never writes an untouched stale field after another owner changed it", async () => {
    const server = backend();
    const store = useSettingsStore();
    await store.fetchSettings();
    const editor = session(IDENTITY_FIELDS);
    editor.edit("userName", "local-name");
    const firstSave = editor.save();
    const firstWrite = await server.write(0);
    expect(firstWrite.patch).toEqual({ userName: "local-name" });

    // Rust merges against the current database, including independent changes.
    server.database.settings.adminUsername = "remote-admin";
    server.database.settings.agentOrder = ["agent-b", "agent-a"];
    firstWrite.commit();
    await firstSave;
    expect(editor.draft.value?.adminUsername).toBe("remote-admin");

    editor.edit("adminPassword", "new-password");
    const secondSave = editor.save();
    const secondWrite = await server.write(1);
    expect(secondWrite.patch).toEqual({ adminPassword: "new-password" });
    secondWrite.commit();
    await secondSave;
    expect(store.settings?.adminUsername).toBe("remote-admin");
    expect(store.settings?.agentOrder).toEqual(["agent-b", "agent-a"]);
  });

  it("keeps typing during a save without claiming those later edits were committed", async () => {
    const server = backend();
    await useSettingsStore().fetchSettings();
    const editor = session(IDENTITY_FIELDS);
    editor.edit("userName", "submitted");
    const saving = editor.save();
    const write = await server.write(0);

    editor.edit("userName", "still-typing");
    write.commit();
    const result = await saving;

    expect(result).toMatchObject({ ok: true, changed: true, dirty: true });
    if (result.ok) expect(result.snapshot.userName).toBe("submitted");
    expect(editor.draft.value?.userName).toBe("still-typing");
    expect(server.writes).toHaveLength(1);
  });

  it("preserves an edit back to the old value and drains another explicit save", async () => {
    const server = backend();
    await useSettingsStore().fetchSettings();
    const editor = session(IDENTITY_FIELDS);
    editor.edit("userName", "submitted");
    const firstSave = editor.save();
    const firstWrite = await server.write(0);

    editor.edit("userName", "original");
    expect(editor.dirty.value).toBe(false);
    expect(editor.save()).toBe(firstSave);
    firstWrite.commit();
    const secondWrite = await server.write(1);
    expect(secondWrite.patch).toEqual({ userName: "original" });
    secondWrite.commit();

    expect(await firstSave).toMatchObject({ ok: true, changed: true, dirty: false });
    expect(editor.draft.value?.userName).toBe("original");
  });

  it("stops a failed save chain, preserves the latest draft, and retries explicitly", async () => {
    const server = backend();
    await useSettingsStore().fetchSettings();
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const editor = session(SUMMARY_FIELDS);
    editor.edit("topicSummaryModel", "model-b");
    const firstSave = editor.save();
    const firstWrite = await server.write(0);
    editor.edit("topicSummaryModel", "model-c");
    editor.save();
    firstWrite.reject(new Error("disk full"));

    expect(await firstSave).toEqual({ ok: false, error: "Error: disk full" });
    expect(server.writes).toHaveLength(1);
    expect(editor.error.value).toContain("disk full");
    expect(editor.draft.value?.topicSummaryModel).toBe("model-c");
    expect(editor.dirty.value).toBe(true);

    const retry = editor.save();
    const retryWrite = await server.write(1);
    expect(retryWrite.patch).toEqual({ topicSummaryModel: "model-c" });
    retryWrite.commit();
    expect(await retry).toMatchObject({ ok: true, dirty: false });
    expect(editor.error.value).toBeNull();
  });

  it("settles only already requested work and leaves unrequested edits for the caller", async () => {
    const server = backend();
    await useSettingsStore().fetchSettings();
    const editor = session(IDENTITY_FIELDS);
    editor.edit("userName", "submitted");
    const save = editor.save();
    const write = await server.write(0);
    editor.edit("adminPassword", "unrequested-password");
    expect(editor.settlePending()).toBe(save);
    write.commit();
    expect(await save).toMatchObject({ ok: true, dirty: true });

    expect(await editor.settlePending()).toMatchObject({ ok: true, changed: false, dirty: true });
    expect(server.writes).toHaveLength(1);
    expect(editor.draft.value?.adminPassword).toBe("unrequested-password");
  });

  it("rejects importing over its pending save and accepts only replaced fields afterward", async () => {
    const server = backend();
    const store = useSettingsStore();
    await store.fetchSettings();
    const editor = session(IDENTITY_FIELDS);
    editor.edit("userName", "saving-name");
    const save = editor.save();
    expect(() => editor.acceptImport(["userName"])).toThrow("保存完成");
    (await server.write(0)).commit();
    await save;

    editor.edit("userName", "unsubmitted-name");
    editor.edit("adminPassword", "keep-this-draft");
    const imported = store.updateSettings({ userName: "imported-name" });
    (await server.write(1)).commit();
    await imported;
    expect(editor.draft.value?.userName).toBe("unsubmitted-name");

    // Another committed update before acceptance must not be replaced by an old response.
    const latest = store.updateSettings({ userName: "latest-name" });
    (await server.write(2)).commit();
    await latest;
    editor.acceptImport(["userName"]);
    expect(editor.draft.value).toEqual({
      userName: "latest-name",
      adminUsername: "admin",
      adminPassword: "keep-this-draft",
    });
    expect(editor.dirty.value).toBe(true);
  });

  it("freezes edits on leave, waits the current write, then saves the newer draft", async () => {
    const server = backend();
    await useSettingsStore().fetchSettings();
    const editor = session(IDENTITY_FIELDS);
    editor.edit("userName", "first-name");
    const save = editor.save();
    const firstWrite = await server.write(0);
    editor.edit("userName", "last-name");

    const leaving = editor.prepareLeave();
    expect(editor.prepareLeave()).toBe(leaving);
    expect(editor.leaving.value).toBe(true);
    editor.edit("userName", "blocked-edit");
    firstWrite.commit();
    const finalWrite = await server.write(1);
    expect(finalWrite.patch).toEqual({ userName: "last-name" });
    finalWrite.commit();

    expect(await leaving).toBe(true);
    expect(await save).toMatchObject({ ok: true, dirty: false });
    expect(editor.leaving.value).toBe(true);
    expect(editor.draft.value?.userName).toBe("last-name");
  });

  it("keeps the page editable when leaving fails", async () => {
    const server = backend();
    await useSettingsStore().fetchSettings();
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const editor = session(IDENTITY_FIELDS);
    editor.edit("userName", "new-name");
    const leaving = editor.prepareLeave();
    (await server.write(0)).reject(new Error("write denied"));

    expect(await leaving).toBe(false);
    expect(editor.leaving.value).toBe(false);
    expect(editor.dirty.value).toBe(true);
    editor.edit("userName", "corrected-name");
    expect(editor.draft.value?.userName).toBe("corrected-name");
  });

  it("ends an old visit without cancelling its submitted write or changing a new draft", async () => {
    const server = backend();
    const store = useSettingsStore();
    await store.fetchSettings();
    const oldEditor = session(IDENTITY_FIELDS);
    oldEditor.edit("userName", "old-submitted");
    const save = oldEditor.save();
    const oldWrite = await server.write(0);
    oldEditor.edit("userName", "never-submit");
    oldEditor.save();
    oldEditor.dispose();

    const newEditor = session(IDENTITY_FIELDS);
    newEditor.edit("userName", "new-visit-draft");
    oldWrite.commit();
    expect((await save).ok).toBe(false);
    expect(store.settings?.userName).toBe("old-submitted");
    expect(newEditor.draft.value?.userName).toBe("new-visit-draft");
    expect(oldEditor.draft.value?.userName).toBe("never-submit");
    expect(oldEditor.ready.value).toBe(false);
    expect(oldEditor.error.value).toBeNull();
    expect(server.writes).toHaveLength(1);
  });

  it("does not lose a save requested after a no-op batch has settled", async () => {
    const server = backend();
    await useSettingsStore().fetchSettings();
    const editor = session(SUMMARY_FIELDS);
    const noOp = editor.save();
    // The first microtask processes the empty batch; its public Promise may still settle.
    await Promise.resolve();
    editor.edit("topicSummaryModel", "model-b");
    const changed = editor.save();
    (await server.write(0)).commit();

    expect(await noOp).toMatchObject({ ok: true, changed: false });
    expect(await changed).toMatchObject({ ok: true, changed: true, dirty: false });
  });

  it("returns a fixed snapshot for actions and avoids no-op writes", async () => {
    const server = backend();
    const store = useSettingsStore();
    await store.fetchSettings();
    const editor = session(IDENTITY_FIELDS);
    const result = await editor.save();
    expect(result).toMatchObject({ ok: true, changed: false, dirty: false });
    expect(server.writes).toHaveLength(0);
    if (!result.ok) throw new Error(result.error);

    store.settings!.agentOrder.push("later-agent");
    expect(result.snapshot.agentOrder).toEqual(["agent-a"]);
  });
});

describe("settings Store queue", () => {
  it("captures patch arrays at enqueue time and describes only configuration persistence", async () => {
    const server = backend();
    const store = useSettingsStore();
    await store.fetchSettings();
    const first = store.updateSettings({ userName: "hold-the-queue" });
    const firstWrite = await server.write(0);
    const order = ["agent-b", "agent-a"];
    const patch = { agentOrder: order };
    const queued = store.updateSettings(patch);
    order.push("later-mutation");
    patch.agentOrder = ["replacement"];

    firstWrite.commit();
    await first;
    const queuedWrite = await server.write(1);
    expect(queuedWrite.patch).toEqual({ agentOrder: ["agent-b", "agent-a"] });
    queuedWrite.commit();
    await queued;
    expect(notify).toHaveBeenLastCalledWith(expect.objectContaining({ title: "设置已保存" }));
  });
});
