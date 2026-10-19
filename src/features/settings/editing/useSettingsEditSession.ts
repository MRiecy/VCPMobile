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
import { useOverlayStore } from "../../../core/stores/overlay";
import type { SettingsEditableKey } from "./fieldSets";
import type { ConfigDomain, FieldIssue } from "../../../core/config/generated/config";
import { buildSettingsUpdate, cloneConfig, readSetting, sameConfigValue, type SettingsFieldValues } from "../../../core/config/fields";

export type SettingsSaveResult =
  | {
      ok: true;
      changed: boolean;
      dirty: boolean;
      snapshot: Readonly<AppSettings>;
    }
  | { ok: false; error: string; issues?: FieldIssue[] };

/** One visit's draft. The settings Store remains the only committed snapshot. */
export function useSettingsEditSession<const Keys extends readonly SettingsEditableKey[]>(
  fieldKeys: Keys,
) {
  type Key = Keys[number];
  type Fields = Pick<SettingsFieldValues, Key>;

  const settingsStore = useSettingsStore();
  const overlay = useOverlayStore();
  const keys = [...fieldKeys];
  const keySet = new Set<Key>(keys);
  const base = shallowRef<Fields | null>(null);
  const draftState = shallowRef<Fields | null>(null);
  const savingState = ref(false);
  const leavingState = ref(false);
  const errorState = ref<string | null>(null);
  const disposed = ref(false);
  const editRevisions = new Map<Key, number>();
  const domainRevisions: Partial<Record<ConfigDomain, string>> = {};
  const fieldErrors = shallowRef<FieldIssue[]>([]);
  const domains = [...new Set(keys.map(key => key.split('.')[0] as ConfigDomain))];
  const domainIssues = computed(() => domains.flatMap(domain => {
    const status = settingsStore.snapshot?.domains[domain];
    return status ? [{ domain, ...status }] : [];
  }).filter(status => status.issues.length > 0));
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
    return keys.some((key) => !sameConfigValue(draftState.value![key], base.value![key]));
  });
  const saving = computed(() => !disposed.value && savingState.value);
  const leaving = computed(() => leavingState.value);
  const error = computed(() => errorState.value);

  const project = (snapshot: AppSettings): Fields => {
    const fields = {} as Fields;
    for (const key of keys) fields[key] = cloneConfig(readSetting(snapshot, key));
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
      draftState.value = cloneConfig(nextBase);
      for (const domain of domains) domainRevisions[domain] = settingsStore.snapshot!.domains[domain].revision;
      return;
    }

    const nextDraft = { ...previousDraft };
    const preservedBase = { ...previousBase };
    for (const key of keys) {
      // Dirty fields retain their actual edit baseline for server-side conflict checks.
      if (sameConfigValue(previousDraft[key], previousBase[key]) && !pending?.has(key)) {
        nextDraft[key] = nextBase[key];
        preservedBase[key] = nextBase[key];
      }
    }
    for (const domain of domains) {
      const hasLocalIntent = keys.some(key => key.startsWith(`${domain}.`)
        && (pending?.has(key) || !sameConfigValue(nextDraft[key], preservedBase[key])));
      if (!hasLocalIntent) domainRevisions[domain] = settingsStore.snapshot!.domains[domain].revision;
    }
    base.value = preservedBase;
    draftState.value = nextDraft;
  };

  const stopSnapshotWatch = watch(() => settingsStore.settings, rebase, {
    immediate: true,
    flush: "sync",
  });

  const failure = (message: string, issues: FieldIssue[] = []): SettingsSaveResult => {
    if (!disposed.value) { errorState.value = message; fieldErrors.value = issues; }
    return { ok: false, error: message, issues };
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
    const issue = fieldErrors.value.find(item => `${item.domain}.${item.field}` === key);
    if (issue && settingsStore.settings && base.value) {
      base.value = { ...base.value, [key]: cloneConfig(readSetting(settingsStore.settings, key)) };
    }
    fieldErrors.value = fieldErrors.value.filter(item => `${item.domain}.${item.field}` !== key);
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
          if (!sameConfigValue(draftState.value[key], base.value[key])) {
            patch[key] = draftState.value[key];
            batch.set(key, editRevisions.get(key) ?? 0);
          }
        }
        if (batch.size === 0) continue;

        pending = batch;
        const request = buildSettingsUpdate(settingsStore.snapshot!, patch, base.value, domainRevisions);
        const result = await settingsStore.updateSettings(request);
        if (disposed.value) return failure("设置页面已关闭");
        if (result.status !== 'saved') {
          saveRequested = false;
          const issues = result.status === 'rejected' ? result.issues : result.conflicts;
          return failure(issues.map(issue => issue.message).join('；') || '配置已变化，请检查后重试', issues);
        }

        changed = true;
        const latest = project(settingsStore.settings!);
        const nextBase = { ...base.value };
        const nextDraft = { ...draftState.value } as Fields;
        for (const [key, revision] of batch) {
          nextBase[key] = latest[key];
          if ((editRevisions.get(key) ?? 0) === revision) {
            nextDraft[key] = latest[key];
          }
        }
        base.value = nextBase;
        draftState.value = nextDraft;
        fieldErrors.value = [];
        rebase();
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
    activeLeave = save().then(async (result) => {
      if (disposed.value) return false;
      if (!result.ok && result.issues?.some(issue => issue.code === 'requiresReset')) {
        const accepted = await overlay.showConfirm({
          title: '放弃本页未保存修改并返回？',
          message: '部分配置需要在设置首页整组恢复默认值。本页未保存的修改将被放弃。',
        });
        if (disposed.value) return false;
        if (accepted) {
          dispose();
          // A rejected write can be the first evidence of damaged stored data.
          // Refresh the home notice; a read failure remains visible in the Store.
          try { await settingsStore.fetchSettings(); } catch { /* Return remains available. */ }
          return true;
        }
      }
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
      if (keySet.has(key)) {
        const value = cloneConfig(readSetting(settingsStore.settings!, key));
        base.value = { ...base.value, [key]: value };
        nextDraft[key] = value;
      }
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
    fieldErrors,
    domainIssues,
    edit,
    save,
    settlePending,
    prepareLeave,
    acceptImport,
    dispose,
  };
}
