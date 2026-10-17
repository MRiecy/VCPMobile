import {
  computed,
  getCurrentScope,
  onScopeDispose,
  ref,
  shallowReadonly,
  shallowRef,
  watch,
} from "vue";
import { useSettingsStore, type AppSettings } from "../../../core/stores/settings";
import type { SettingsEditableKey } from "./fieldSets";

export type SettingsSaveResult =
  | {
      ok: true;
      changed: boolean;
      dirty: boolean;
      snapshot: Readonly<AppSettings>;
    }
  | { ok: false; error: string };

/** One visit's draft. The settings Store remains the only committed snapshot. */
export function useSettingsEditSession<const Keys extends readonly SettingsEditableKey[]>(
  fieldKeys: Keys,
) {
  type Key = Keys[number];
  type Fields = Pick<AppSettings, Key>;

  const settingsStore = useSettingsStore();
  const keys = [...fieldKeys];
  const keySet = new Set<Key>(keys);
  const base = shallowRef<Fields | null>(null);
  const draftState = shallowRef<Fields | null>(null);
  const savingState = ref(false);
  const leavingState = ref(false);
  const errorState = ref<string | null>(null);
  const disposed = ref(false);
  const editRevisions = new Map<Key, number>();
  let pending: Map<Key, number> | null = null;
  let saveRequested = false;
  let activeSave: Promise<SettingsSaveResult> | null = null;
  let activeLeave: Promise<boolean> | null = null;

  const ready = computed(() => !disposed.value && base.value !== null && settingsStore.settings !== null);
  const draft = computed<Readonly<Fields> | null>(() =>
    draftState.value ? shallowReadonly(draftState.value) : null,
  );
  const dirty = computed(() => {
    if (disposed.value || !base.value || !draftState.value) return false;
    return keys.some((key) => draftState.value![key] !== base.value![key]);
  });
  const saving = computed(() => !disposed.value && savingState.value);
  const leaving = computed(() => leavingState.value);
  const error = computed(() => errorState.value);

  const project = (snapshot: AppSettings): Fields => {
    const fields = {} as Fields;
    for (const key of keys) fields[key] = snapshot[key];
    return fields;
  };

  const rebase = () => {
    const snapshot = settingsStore.settings;
    if (disposed.value || !snapshot) return;
    const nextBase = project(snapshot);
    const previousBase = base.value;
    const previousDraft = draftState.value;
    if (!previousBase || !previousDraft) {
      base.value = nextBase;
      draftState.value = { ...nextBase };
      return;
    }

    const nextDraft = { ...previousDraft };
    for (const key of keys) {
      // A user can edit back to the old baseline while its replacement is pending.
      // That temporarily clean field still owns a local intent until this batch settles.
      if (previousDraft[key] === previousBase[key] && !pending?.has(key)) {
        nextDraft[key] = nextBase[key];
      }
    }
    base.value = nextBase;
    draftState.value = nextDraft;
  };

  const stopSnapshotWatch = watch(() => settingsStore.settings, rebase, {
    immediate: true,
    flush: "sync",
  });

  const failure = (message: string): SettingsSaveResult => {
    if (!disposed.value) errorState.value = message;
    return { ok: false, error: message };
  };

  const currentResult = (changed = false): SettingsSaveResult => {
    if (disposed.value) return failure("设置页面已关闭");
    const snapshot = settingsStore.settings;
    if (!ready.value || !snapshot) return failure("设置尚未加载，请重试");
    // Export and diagnostics receive a fixed value, never a live Store object.
    return {
      ok: true,
      changed,
      dirty: dirty.value,
      snapshot: JSON.parse(JSON.stringify(snapshot)) as AppSettings,
    };
  };

  const edit = <K extends Key>(key: K, value: Fields[K]): void => {
    if (!ready.value || leavingState.value || !draftState.value || !keySet.has(key)) return;
    editRevisions.set(key, (editRevisions.get(key) ?? 0) + 1);
    draftState.value = { ...draftState.value, [key]: value };
  };

  const runSave = async (): Promise<SettingsSaveResult> => {
    let changed = false;
    try {
      while (saveRequested && !disposed.value) {
        saveRequested = false;
        rebase();
        if (!ready.value || !base.value || !draftState.value) {
          return failure("设置尚未加载，请重试");
        }

        const patch: Partial<Fields> = {};
        const batch = new Map<Key, number>();
        for (const key of keys) {
          if (draftState.value[key] !== base.value[key]) {
            patch[key] = draftState.value[key];
            batch.set(key, editRevisions.get(key) ?? 0);
          }
        }
        if (batch.size === 0) continue;

        pending = batch;
        await settingsStore.updateSettings(patch);
        if (disposed.value) return failure("设置页面已关闭");

        changed = true;
        rebase();
        const nextDraft = { ...draftState.value } as Fields;
        for (const [key, revision] of batch) {
          if ((editRevisions.get(key) ?? 0) === revision) {
            nextDraft[key] = base.value[key];
          }
        }
        draftState.value = nextDraft;
        pending = null;
      }
      return currentResult(changed);
    } catch (cause) {
      rebase();
      saveRequested = false;
      return failure(String(cause));
    } finally {
      pending = null;
      // Clear synchronously with the final batch. A later save must start a new
      // process, rather than attach to an already settled loop's finalizer.
      activeSave = null;
      if (!disposed.value) savingState.value = false;
    }
  };

  const save = (): Promise<SettingsSaveResult> => {
    if (!ready.value) return Promise.resolve(currentResult());
    saveRequested = true;
    if (activeSave) return activeSave;
    // Install activeSave before processing an empty batch or calling the Store.
    const operation = Promise.resolve().then(runSave);
    activeSave = operation;
    errorState.value = null;
    savingState.value = true;
    return operation;
  };

  const settlePending = (): Promise<SettingsSaveResult> => {
    if (activeSave) return activeSave;
    rebase();
    return Promise.resolve(currentResult());
  };

  const prepareLeave = (): Promise<boolean> => {
    if (disposed.value) return Promise.resolve(false);
    if (activeLeave) return activeLeave;
    leavingState.value = true;
    // A load failure must not trap a page that never had an editable draft.
    if (!draftState.value) return Promise.resolve(true);
    activeLeave = save().then((result) => {
      if (disposed.value) return false;
      const canLeave = result.ok && !dirty.value;
      if (!canLeave) leavingState.value = false;
      return canLeave;
    }).finally(() => {
      activeLeave = null;
    });
    return activeLeave;
  };

  const acceptImport = (replacedKeys: readonly Key[]): void => {
    if (disposed.value) return;
    if (activeSave || pending) throw new Error("请等待当前设置保存完成后应用导入");
    rebase();
    if (!ready.value || !base.value || !draftState.value) {
      throw new Error("设置尚未加载，无法应用导入");
    }
    const nextDraft = { ...draftState.value };
    for (const key of replacedKeys) {
      if (keySet.has(key)) nextDraft[key] = base.value[key];
    }
    draftState.value = nextDraft;
    errorState.value = null;
  };

  const dispose = (): void => {
    if (disposed.value) return;
    disposed.value = true;
    saveRequested = false;
    stopSnapshotWatch();
  };
  if (getCurrentScope()) onScopeDispose(dispose);

  return {
    draft,
    ready,
    dirty,
    saving,
    leaving,
    error,
    edit,
    save,
    settlePending,
    prepareLeave,
    acceptImport,
    dispose,
  };
}
