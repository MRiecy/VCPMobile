import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useSettingsStore, type AppSettings } from "@/core/stores/settings";
import {
  IDENTITY_FIELDS,
  SUMMARY_FIELDS,
  type SettingsEditableKey,
} from "@/features/settings/editing/fieldSets";
import { useSettingsEditSession } from "@/features/settings/editing/useSettingsEditSession";
import { invokeMock, mockInvoke } from "../../mocks/tauri";
import { makeSettings, makeConfigSnapshot, configRequestChanges } from '../../helpers/config';
import { applySettingsFields, type SettingsFieldValues } from '@/core/config/fields';
import type { ConfigUpdate, ConfigSaveResult } from '@/core/config/generated/config';
import { emitTauriEvent } from '../../mocks/tauri';

const { notify } = vi.hoisted(() => ({ notify: vi.fn() }));
vi.mock("@/core/stores/notification", () => ({
  useNotificationStore: () => ({ addNotification: notify }),
}));

const initialSettings = (): AppSettings => makeSettings({
  'identity.userName': 'original', 'server.url': 'https://vcp.example', 'server.apiKey': 'key',
  'server.adminUsername': 'admin', 'server.adminPassword': 'password',
  'conversation.topicSummaryModel': 'model-a', 'navigation.agentOrder': ['agent-a'],
});

const clone = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

interface PendingWrite {
  patch: Partial<SettingsFieldValues>;
  request: ConfigUpdate;
  commit(): void;
  respond(result: ConfigSaveResult): void;
  reject(error: Error): void;
}

function backend() {
  const database = { settings: initialSettings(), revision: 1 };
  const writes: PendingWrite[] = [];
  mockInvoke("read_settings", () => makeConfigSnapshot(database.settings, String(database.revision)));
  mockInvoke("update_settings", (args) => new Promise<ConfigSaveResult>((resolve, reject) => {
    const request = clone(args?.request as ConfigUpdate);
    const patch = configRequestChanges(request);
    writes.push({
      patch, request,
      respond: resolve,
      commit: () => {
        database.settings = applySettingsFields(database.settings, patch);
        database.revision += 1;
        resolve({status: "saved", snapshot: makeConfigSnapshot(database.settings, String(database.revision))});
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
    editor.edit("identity.userName", "unsaved");
    expect(editor.ready.value).toBe(false);
    expect(editor.draft.value).toBeNull();
    expect((await editor.save()).ok).toBe(false);
    expect((await editor.settlePending()).ok).toBe(false);
    expect(await editor.prepareLeave()).toBe(true);
    expect(server.writes).toHaveLength(0);
  });


  it("never writes an untouched stale field after another owner changed it", async () => {
    const server = backend();
    const store = useSettingsStore();
    await store.fetchSettings();
    const editor = session(IDENTITY_FIELDS);
    editor.edit("identity.userName", "local-name");
    const firstSave = editor.save();
    const firstWrite = await server.write(0);
    expect(firstWrite.patch).toEqual({ "identity.userName": "local-name" });

    // Rust merges against the current database, including independent changes.
    server.database.settings.server.adminUsername = "remote-admin";
    server.database.settings.navigation.agentOrder = ["agent-b", "agent-a"];
    firstWrite.commit();
    await firstSave;
    expect(editor.draft.value?.["server.adminUsername"]).toBe("remote-admin");

    editor.edit("server.adminPassword", "new-password");
    const secondSave = editor.save();
    const secondWrite = await server.write(1);
    expect(secondWrite.patch).toEqual({ "server.adminPassword": "new-password" });
    secondWrite.commit();
    await secondSave;
    expect(store.settings?.server.adminUsername).toBe("remote-admin");
    expect(store.settings?.navigation.agentOrder).toEqual(["agent-b", "agent-a"]);
  });

  it("keeps typing during a save without claiming those later edits were committed", async () => {
    const server = backend();
    await useSettingsStore().fetchSettings();
    const editor = session(IDENTITY_FIELDS);
    editor.edit("identity.userName", "submitted");
    const saving = editor.save();
    const write = await server.write(0);

    editor.edit("identity.userName", "still-typing");
    write.commit();
    const result = await saving;

    expect(result).toMatchObject({ ok: true, changed: true, dirty: true });
    if (result.ok) expect(result.snapshot.identity.userName).toBe("submitted");
    expect(editor.draft.value?.["identity.userName"]).toBe("still-typing");
    expect(server.writes).toHaveLength(1);
  });

  it("preserves an edit back to the old value and drains another explicit save", async () => {
    const server = backend();
    await useSettingsStore().fetchSettings();
    const editor = session(IDENTITY_FIELDS);
    editor.edit("identity.userName", "submitted");
    const firstSave = editor.save();
    const firstWrite = await server.write(0);

    editor.edit("identity.userName", "original");
    expect(editor.dirty.value).toBe(false);
    expect(editor.save()).toBe(firstSave);
    firstWrite.commit();
    const secondWrite = await server.write(1);
    expect(secondWrite.patch).toEqual({ "identity.userName": "original" });
    secondWrite.commit();

    expect(await firstSave).toMatchObject({ ok: true, changed: true, dirty: false });
    expect(editor.draft.value?.["identity.userName"]).toBe("original");
  });

  it("stops a failed save chain, preserves the latest draft, and retries explicitly", async () => {
    const server = backend();
    await useSettingsStore().fetchSettings();
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const editor = session(SUMMARY_FIELDS);
    editor.edit("conversation.topicSummaryModel", "model-b");
    const firstSave = editor.save();
    const firstWrite = await server.write(0);
    editor.edit("conversation.topicSummaryModel", "model-c");
    editor.save();
    firstWrite.reject(new Error("disk full"));

    expect(await firstSave).toMatchObject({ ok: false, error: "Error: 设置保存失败，请重试" });
    expect(server.writes).toHaveLength(1);
    expect(editor.error.value).toContain("设置保存失败");
    expect(editor.draft.value?.["conversation.topicSummaryModel"]).toBe("model-c");
    expect(editor.dirty.value).toBe(true);

    const retry = editor.save();
    const retryWrite = await server.write(1);
    expect(retryWrite.patch).toEqual({ "conversation.topicSummaryModel": "model-c" });
    retryWrite.commit();
    expect(await retry).toMatchObject({ ok: true, dirty: false });
    expect(editor.error.value).toBeNull();
  });

  it("settles only already requested work and leaves unrequested edits for the caller", async () => {
    const server = backend();
    await useSettingsStore().fetchSettings();
    const editor = session(IDENTITY_FIELDS);
    editor.edit("identity.userName", "submitted");
    const save = editor.save();
    const write = await server.write(0);
    editor.edit("server.adminPassword", "unrequested-password");
    expect(editor.settlePending()).toBe(save);
    write.commit();
    expect(await save).toMatchObject({ ok: true, dirty: true });

    expect(await editor.settlePending()).toMatchObject({ ok: true, changed: false, dirty: true });
    expect(server.writes).toHaveLength(1);
    expect(editor.draft.value?.["server.adminPassword"]).toBe("unrequested-password");
  });

  it("rejects importing over its pending save and accepts only replaced fields afterward", async () => {
    const server = backend();
    const store = useSettingsStore();
    await store.fetchSettings();
    const editor = session(IDENTITY_FIELDS);
    editor.edit("identity.userName", "saving-name");
    const save = editor.save();
    expect(() => editor.acceptImport(["identity.userName"])).toThrow("保存完成");
    (await server.write(0)).commit();
    await save;

    editor.edit("identity.userName", "unsubmitted-name");
    editor.edit("server.adminPassword", "keep-this-draft");
    const imported = store.updateFields({ "identity.userName": "imported-name" });
    (await server.write(1)).commit();
    await imported;
    expect(editor.draft.value?.["identity.userName"]).toBe("unsubmitted-name");

    // Another committed update before acceptance must not be replaced by an old response.
    const latest = store.updateFields({ "identity.userName": "latest-name" });
    (await server.write(2)).commit();
    await latest;
    editor.acceptImport(["identity.userName"]);
    expect(editor.draft.value).toEqual({
      "identity.userName": "latest-name",
      "server.adminUsername": "admin",
      "server.adminPassword": "keep-this-draft",
    });
    expect(editor.dirty.value).toBe(true);
  });

  it("freezes edits on leave, waits the current write, then saves the newer draft", async () => {
    const server = backend();
    await useSettingsStore().fetchSettings();
    const editor = session(IDENTITY_FIELDS);
    editor.edit("identity.userName", "first-name");
    const save = editor.save();
    const firstWrite = await server.write(0);
    editor.edit("identity.userName", "last-name");

    const leaving = editor.prepareLeave();
    expect(editor.prepareLeave()).toBe(leaving);
    expect(editor.leaving.value).toBe(true);
    editor.edit("identity.userName", "blocked-edit");
    firstWrite.commit();
    const finalWrite = await server.write(1);
    expect(finalWrite.patch).toEqual({ "identity.userName": "last-name" });
    finalWrite.commit();

    expect(await leaving).toBe(true);
    expect(await save).toMatchObject({ ok: true, dirty: false });
    expect(editor.leaving.value).toBe(true);
    expect(editor.draft.value?.["identity.userName"]).toBe("last-name");
  });

  it("keeps the page editable when leaving fails", async () => {
    const server = backend();
    await useSettingsStore().fetchSettings();
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const editor = session(IDENTITY_FIELDS);
    editor.edit("identity.userName", "new-name");
    const leaving = editor.prepareLeave();
    (await server.write(0)).reject(new Error("write denied"));

    expect(await leaving).toBe(false);
    expect(editor.leaving.value).toBe(false);
    expect(editor.dirty.value).toBe(true);
    editor.edit("identity.userName", "corrected-name");
    expect(editor.draft.value?.["identity.userName"]).toBe("corrected-name");
  });

  it('keeps validation failures editable and identifies the affected field', async () => {
    const server = backend();
    await useSettingsStore().fetchSettings();
    const editor = session(IDENTITY_FIELDS);
    editor.edit('identity.userName', 'invalid-name');
    const leaving = editor.prepareLeave();
    (await server.write(0)).respond({ status: 'rejected', issues: [
      { domain: 'identity', field: 'userName', code: 'tooLong', message: '用户名过长' },
    ] });
    expect(await leaving).toBe(false);
    expect(invokeMock.mock.calls.filter(([command]) => command === 'read_settings')).toHaveLength(1);
    expect(editor.ready.value).toBe(true);
    expect(editor.dirty.value).toBe(true);
    expect(editor.error.value).toBe('用户身份 · 用户名：用户名过长');
  });

  it("ends an old visit without cancelling its submitted write or changing a new draft", async () => {
    const server = backend();
    const store = useSettingsStore();
    await store.fetchSettings();
    const oldEditor = session(IDENTITY_FIELDS);
    oldEditor.edit("identity.userName", "old-submitted");
    const save = oldEditor.save();
    const oldWrite = await server.write(0);
    oldEditor.edit("identity.userName", "never-submit");
    oldEditor.save();
    oldEditor.dispose();

    const newEditor = session(IDENTITY_FIELDS);
    newEditor.edit("identity.userName", "new-visit-draft");
    oldWrite.commit();
    expect((await save).ok).toBe(false);
    expect(store.settings?.identity.userName).toBe("old-submitted");
    expect(newEditor.draft.value?.["identity.userName"]).toBe("new-visit-draft");
    expect(oldEditor.draft.value?.["identity.userName"]).toBe("never-submit");
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
    editor.edit("conversation.topicSummaryModel", "model-b");
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

    store.settings!.navigation.agentOrder.push("later-agent");
    expect(result.snapshot.navigation.agentOrder).toEqual(["agent-a"]);
  });
});

describe("settings Store queue", () => {
  it('keeps an older edit baseline through an external update and exposes a real field conflict', async () => {
    const server = backend();
    const store = useSettingsStore();
    await store.fetchSettings();
    const editor = session(IDENTITY_FIELDS);
    editor.edit('identity.userName', 'local');
    const remote = makeConfigSnapshot(makeSettings({ 'identity.userName': 'remote' }), '2');
    emitTauriEvent('vcp-config-changed', remote);
    expect(editor.draft.value?.['identity.userName']).toBe('local');
    const saving = editor.save();
    const write = await server.write(0);
    expect(write.request.patches[0]).toMatchObject({ expectedRevision: '1', baseValues: { userName: 'original' } });
    write.respond({ status: 'conflict', snapshot: remote, conflicts: [{ domain: 'identity', field: 'userName', code: 'conflict', message: '用户名已被其他操作修改' }] });
    expect(await saving).toMatchObject({ ok: false });
    expect(editor.error.value).toBe('用户身份 · 用户名：用户名已被其他操作修改');
    expect(editor.draft.value?.['identity.userName']).toBe('local');
    editor.edit('identity.userName', 'explicit-new-intent');
    const retry = editor.save();
    const next = await server.write(1);
    expect(next.request.patches[0].baseValues).toEqual({ userName: 'remote' });
    next.respond({ status: 'saved', snapshot: makeConfigSnapshot(makeSettings({ 'identity.userName': 'explicit-new-intent' }), '3') });
    expect(await retry).toMatchObject({ ok: true, dirty: false });
  });

  it('does not replace a newer per-domain event with a late read or save result', async () => {
    const server = backend();
    const store = useSettingsStore();
    await store.fetchSettings();
    const remote = makeConfigSnapshot(makeSettings({ 'identity.userName': 'remote' }), '3');
    emitTauriEvent('vcp-config-changed', remote);
    await store.fetchSettings();
    expect(store.settings?.identity.userName).toBe('remote');
    const save = store.updateFields({ 'server.apiKey': 'new-key' });
    const write = await server.write(0);
    const ack = makeConfigSnapshot(makeSettings({ 'server.apiKey': 'new-key' }), '1');
    ack.domains.server.revision = '4';
    write.respond({ status: 'saved', snapshot: ack });
    await save;
    expect(store.settings?.identity.userName).toBe('remote');
    expect(store.settings?.server.apiKey).toBe('new-key');
  });

  it("captures patch arrays at enqueue time and describes only configuration persistence", async () => {
    const server = backend();
    const store = useSettingsStore();
    await store.fetchSettings();
    const first = store.updateFields({ "identity.userName": "hold-the-queue" });
    const firstWrite = await server.write(0);
    const order = ["agent-b", "agent-a"];
    const patch = { "navigation.agentOrder": order };
    const queued = store.updateFields(patch);
    order.push("later-mutation");
    patch["navigation.agentOrder"] = ["replacement"];

    firstWrite.commit();
    await first;
    const queuedWrite = await server.write(1);
    expect(queuedWrite.patch).toEqual({ "navigation.agentOrder": ["agent-b", "agent-a"] });
    queuedWrite.commit();
    await queued;
    expect(notify).toHaveBeenLastCalledWith(expect.objectContaining({ title: "设置已保存" }));
  });
});
