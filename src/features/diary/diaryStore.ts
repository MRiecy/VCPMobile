import { invoke } from "@tauri-apps/api/core";
import { defineStore } from "pinia";
import { computed, ref } from "vue";
import type {
  DiaryBatchOutcome,
  DiaryComposerDraft,
  DiaryCreateOutcome,
  DiaryDocument,
  DiaryFolderCategory,
  DiaryFolderList,
  DiaryNoteKey,
  DiaryNoteSummary,
  DiaryRenameOutcome,
  DiarySaveOutcome,
  DiarySaveState,
  DiarySearchMode,
  DiarySearchResponse,
  DiarySearchScope,
  DiarySemanticResponse,
  DiaryUiError,
} from "./types";
import {
  noteKeyId,
  parseDiaryError,
  sameNoteKey,
  semanticHitToSummary,
} from "./types";

const SEARCH_RESULT_LIMIT = 200;
const SEMANTIC_RESULT_LIMIT = 50;
const TOMBSTONE_RETENTION_MS = 5 * 60 * 1000;

function localDateValue(date = new Date()): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function newRequestId(kind: "text" | "semantic" | "mutation"): string {
  const suffix = globalThis.crypto?.randomUUID?.()
    ?? `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  return `diary-${kind}-${suffix}`;
}

function cloneKey(key: DiaryNoteKey): DiaryNoteKey {
  return { folder: key.folder, file: key.file };
}

function cloneComposer(value: DiaryComposerDraft): DiaryComposerDraft {
  return { ...value };
}

function makeComposer(folder = ""): DiaryComposerDraft {
  return {
    maid: "",
    date: localDateValue(),
    folder,
    fileNameSuffix: "",
    tag: "",
    content: "",
  };
}

function replaceSummaryKey(
  items: DiaryNoteSummary[],
  source: DiaryNoteKey,
  target: DiaryNoteKey,
  keepSource: boolean,
): DiaryNoteSummary[] {
  const sourceIndex = items.findIndex((item) => sameNoteKey(item, source));
  if (sourceIndex < 0) return items;

  const next = [...items];
  const targetSummary: DiaryNoteSummary = {
    ...next[sourceIndex],
    ...target,
  };
  if (keepSource) next.splice(sourceIndex, 0, targetSummary);
  else next.splice(sourceIndex, 1, targetSummary);
  return next;
}

export const useDiaryStore = defineStore("diary", () => {
  // Remote resources. Only local display preferences at the bottom are persisted.
  const initialized = ref(false);
  const folders = ref<string[]>([]);
  const foldersLoading = ref(false);
  const foldersRefreshing = ref(false);
  const foldersError = ref<DiaryUiError | null>(null);

  const selectedFolder = ref("");
  const notesFolder = ref("");
  const notes = ref<DiaryNoteSummary[]>([]);
  const notesLoading = ref(false);
  const notesRefreshing = ref(false);
  const notesError = ref<DiaryUiError | null>(null);

  const screen = ref<"list" | "reader" | "editor" | "preview" | "composer">("list");
  const document = ref<DiaryDocument | null>(null);
  const documentTarget = ref<DiaryNoteKey | null>(null);
  const documentLoading = ref(false);
  const documentRefreshing = ref(false);
  const documentError = ref<DiaryUiError | null>(null);

  // Text and semantic search share user input, but keep independent request owners.
  const searchMode = ref<DiarySearchMode>("none");
  const searchScope = ref<DiarySearchScope>("folder");
  const searchQuery = ref("");
  const textSearchResults = ref<DiaryNoteSummary[]>([]);
  const semanticSearchResults = ref<DiaryNoteSummary[]>([]);
  const searchLoading = ref(false);
  const searchError = ref<DiaryUiError | null>(null);
  const searchLimited = ref(false);
  const indexMayBeCatchingUp = ref(false);
  const textRequestId = ref<string | null>(null);
  const semanticRequestId = ref<string | null>(null);

  // Guarded editor state.
  const draft = ref("");
  const baselineContent = ref("");
  const baselineHash = ref("");
  const saveState = ref<DiarySaveState>("idle");
  const saveError = ref<DiaryUiError | null>(null);

  // Create and management state.
  const composerDraft = ref<DiaryComposerDraft>(makeComposer());
  const composerBaseline = ref<DiaryComposerDraft>(makeComposer());
  const composerSubmitting = ref(false);
  const composerError = ref<DiaryUiError | null>(null);
  const selectionMode = ref(false);
  const selectedKeyIds = ref<string[]>([]);
  const tombstones = ref<string[]>([]);
  const activeMutation = ref<string | null>(null);
  const mutationError = ref<DiaryUiError | null>(null);
  const lastBatchOutcome = ref<DiaryBatchOutcome | null>(null);

  // Local discovery preferences only. They never alter server authorization or
  // LightMemo's authoritative exclusion policy.
  const hiddenFolders = ref<string[]>([]);
  const collapsedCategories = ref<DiaryFolderCategory[]>([]);
  const folderOrder = ref<string[]>([]);

  let foldersGeneration = 0;
  let notesGeneration = 0;
  let documentGeneration = 0;
  let textSearchGeneration = 0;
  let semanticSearchGeneration = 0;
  let initializationPromise: Promise<void> | null = null;
  const tombstoneDeadlines = new Map<string, number>();

  const orderedFolders = computed(() => {
    const rank = new Map(folderOrder.value.map((folder, index) => [folder, index]));
    return [...folders.value].sort((left, right) => {
      const leftRank = rank.get(left);
      const rightRank = rank.get(right);
      if (leftRank !== undefined || rightRank !== undefined) {
        return (leftRank ?? Number.MAX_SAFE_INTEGER) - (rightRank ?? Number.MAX_SAFE_INTEGER);
      }
      return left.localeCompare(right, "zh-CN");
    });
  });

  const visibleFolders = computed(() => {
    const hidden = new Set(hiddenFolders.value);
    return orderedFolders.value.filter((folder) => !hidden.has(folder));
  });

  const draftDirty = computed(() => draft.value !== baselineContent.value);
  const composerDirty = computed(
    () => JSON.stringify(composerDraft.value) !== JSON.stringify(composerBaseline.value),
  );
  const isSaving = computed(() => saveState.value === "saving");

  function isTombstoned(key: DiaryNoteKey): boolean {
    const id = noteKeyId(key);
    const deadline = tombstoneDeadlines.get(id);
    return tombstones.value.includes(id)
      && (deadline === undefined || deadline > Date.now());
  }

  function isDiscoverable(key: DiaryNoteKey): boolean {
    return !hiddenFolders.value.includes(key.folder) && !isTombstoned(key);
  }

  function filterDiscoverable(items: DiaryNoteSummary[]): DiaryNoteSummary[] {
    return items.filter(isDiscoverable);
  }

  const displayedNotes = computed(() => {
    if (searchMode.value === "text") return filterDiscoverable(textSearchResults.value);
    if (searchMode.value === "semantic") return filterDiscoverable(semanticSearchResults.value);
    return filterDiscoverable(notes.value);
  });

  const selectedKeys = computed<DiaryNoteKey[]>(() => {
    const selected = new Set(selectedKeyIds.value);
    return displayedNotes.value
      .filter((note) => selected.has(noteKeyId(note)))
      .map(cloneKey);
  });

  const hasInternalState = computed(() =>
    screen.value !== "list"
      || selectionMode.value
      || searchMode.value !== "none"
      || Boolean(activeMutation.value),
  );

  function pruneExpiredTombstones(): void {
    const now = Date.now();
    tombstones.value = tombstones.value.filter((id) => {
      const deadline = tombstoneDeadlines.get(id);
      if (deadline === undefined || deadline > now) return true;
      tombstoneDeadlines.delete(id);
      return false;
    });
  }

  function clearTombstone(key: DiaryNoteKey): void {
    const id = noteKeyId(key);
    tombstoneDeadlines.delete(id);
    tombstones.value = tombstones.value.filter((item) => item !== id);
  }

  function rememberFolderOrder(): void {
    const known = new Set(folders.value);
    const retained = folderOrder.value.filter((folder) => known.has(folder));
    const seen = new Set(retained);
    for (const folder of folders.value) {
      if (!seen.has(folder)) retained.push(folder);
    }
    folderOrder.value = retained;
  }

  async function loadFolders(refresh = initialized.value): Promise<void> {
    const generation = ++foldersGeneration;
    const hadData = initialized.value;
    foldersError.value = null;
    if (refresh && hadData) foldersRefreshing.value = true;
    else foldersLoading.value = true;

    let folderToLoad = "";
    try {
      const response = await invoke<DiaryFolderList>("diary_list_folders");
      if (generation !== foldersGeneration) return;

      folders.value = [...new Set(response.folders)];
      initialized.value = true;
      rememberFolderOrder();

      if (!selectedFolder.value || !visibleFolders.value.includes(selectedFolder.value)) {
        selectedFolder.value = visibleFolders.value[0] ?? "";
      }
      if (selectedFolder.value && notesFolder.value !== selectedFolder.value) {
        folderToLoad = selectedFolder.value;
      } else if (!selectedFolder.value) {
        notes.value = [];
        notesFolder.value = "";
      }
    } catch (error) {
      if (generation === foldersGeneration) foldersError.value = parseDiaryError(error);
    } finally {
      if (generation === foldersGeneration) {
        foldersLoading.value = false;
        foldersRefreshing.value = false;
      }
    }

    if (folderToLoad) await loadNotes(folderToLoad, false);
  }

  async function loadNotes(folder = selectedFolder.value, refresh = notesFolder.value === folder): Promise<void> {
    const targetFolder = folder;
    if (!targetFolder.trim()) {
      notes.value = [];
      notesFolder.value = "";
      return;
    }

    const generation = ++notesGeneration;
    notesError.value = null;
    if (refresh && notes.value.length > 0) notesRefreshing.value = true;
    else notesLoading.value = true;

    try {
      const response = await invoke<DiaryNoteSummary[]>("diary_list_notes", { folder: targetFolder });
      if (generation !== notesGeneration || selectedFolder.value !== targetFolder) return;

      notes.value = response;
      notesFolder.value = targetFolder;
      // File mutations commit before the semantic index catches up. Retain
      // source tombstones for a bounded grace period even after the ordinary
      // list stops reporting the source, so stale LightMemo hits cannot revive it.
      pruneExpiredTombstones();
    } catch (error) {
      if (generation === notesGeneration && selectedFolder.value === targetFolder) {
        notesError.value = parseDiaryError(error);
      }
    } finally {
      if (generation === notesGeneration && selectedFolder.value === targetFolder) {
        notesLoading.value = false;
        notesRefreshing.value = false;
      }
    }
  }

  async function initialize(): Promise<void> {
    if (initialized.value) return;
    if (!initializationPromise) {
      initializationPromise = loadFolders(false).finally(() => {
        initializationPromise = null;
      });
    }
    await initializationPromise;
  }

  async function selectFolder(folder: string): Promise<void> {
    if (!folder) return;
    if (folder === selectedFolder.value && notesFolder.value === folder) return;
    cancelSearch();
    selectionMode.value = false;
    selectedKeyIds.value = [];
    screen.value = "list";
    selectedFolder.value = folder;
    await loadNotes(folder, false);
  }

  function cancelTextSearch(): void {
    const requestId = textRequestId.value;
    ++textSearchGeneration;
    textRequestId.value = null;
    if (searchMode.value === "text") searchLoading.value = false;
    if (requestId) {
      void invoke<void>("diary_cancel_search", { request: { requestId } }).catch(() => undefined);
    }
  }

  function cancelSemanticSearch(): void {
    const requestId = semanticRequestId.value;
    ++semanticSearchGeneration;
    semanticRequestId.value = null;
    if (searchMode.value === "semantic") searchLoading.value = false;
    if (requestId) {
      void invoke<void>("diary_cancel_semantic_search", { request: { requestId } }).catch(() => undefined);
    }
  }

  function cancelSearch(): void {
    cancelTextSearch();
    cancelSemanticSearch();
    searchMode.value = "none";
    searchLoading.value = false;
    searchError.value = null;
    searchLimited.value = false;
    indexMayBeCatchingUp.value = false;
    selectionMode.value = false;
    selectedKeyIds.value = [];
  }

  function setSearchMode(mode: DiarySearchMode): void {
    if (mode === searchMode.value) return;
    if (searchMode.value === "text") cancelTextSearch();
    if (searchMode.value === "semantic") cancelSemanticSearch();
    searchMode.value = mode;
    if (mode === "text") textSearchResults.value = [];
    if (mode === "semantic") {
      semanticSearchResults.value = [];
      indexMayBeCatchingUp.value = false;
    }
    searchLoading.value = false;
    searchError.value = null;
    searchLimited.value = false;
  }

  function setSearchScope(scope: DiarySearchScope): void {
    searchScope.value = scope;
  }

  function invalidateSearchInput(): void {
    searchError.value = null;
    searchLimited.value = false;
    if (searchMode.value === "text") {
      cancelTextSearch();
      textSearchResults.value = [];
    } else if (searchMode.value === "semantic") {
      cancelSemanticSearch();
      semanticSearchResults.value = [];
      indexMayBeCatchingUp.value = false;
    }
  }

  async function runTextSearch(query = searchQuery.value): Promise<void> {
    searchQuery.value = query;
    const term = query.trim();
    if (!term) {
      cancelSearch();
      return;
    }

    if (searchMode.value === "semantic") cancelSemanticSearch();
    searchMode.value = "text";
    const generation = ++textSearchGeneration;
    const requestId = newRequestId("text");
    textRequestId.value = requestId;
    searchLoading.value = true;
    searchError.value = null;
    searchLimited.value = false;

    try {
      const response = await invoke<DiarySearchResponse>("diary_search", {
        request: {
          requestId,
          term,
          folder: searchScope.value === "folder" ? selectedFolder.value : null,
        },
      });
      if (
        generation !== textSearchGeneration
        || textRequestId.value !== requestId
        || searchMode.value !== "text"
      ) return;

      textSearchResults.value = response.notes.slice(0, SEARCH_RESULT_LIMIT);
      searchLimited.value = response.limited
        || response.notes.length > SEARCH_RESULT_LIMIT
        || response.total > textSearchResults.value.length;
    } catch (error) {
      if (
        generation === textSearchGeneration
        && textRequestId.value === requestId
        && searchMode.value === "text"
      ) {
        const parsed = parseDiaryError(error);
        if (parsed.code !== "DIARY_CANCELLED") searchError.value = parsed;
      }
    } finally {
      if (
        generation === textSearchGeneration
        && textRequestId.value === requestId
        && searchMode.value === "text"
      ) {
        textRequestId.value = null;
        searchLoading.value = false;
      }
    }
  }

  async function runSemanticSearch(query = searchQuery.value, k = 5): Promise<void> {
    searchQuery.value = query;
    const term = query.trim();
    if (!term) {
      cancelSearch();
      return;
    }

    if (searchMode.value === "text") cancelTextSearch();
    searchMode.value = "semantic";
    const generation = ++semanticSearchGeneration;
    const requestId = newRequestId("semantic");
    semanticRequestId.value = requestId;
    searchLoading.value = true;
    searchError.value = null;
    searchLimited.value = false;

    try {
      const response = await invoke<DiarySemanticResponse>("diary_semantic_search", {
        request: {
          requestId,
          query: term,
          folder: searchScope.value === "folder" ? selectedFolder.value : null,
          searchAll: searchScope.value === "all",
          k: Math.min(Math.max(Math.trunc(k), 1), SEMANTIC_RESULT_LIMIT),
        },
      });
      if (
        generation !== semanticSearchGeneration
        || semanticRequestId.value !== requestId
        || searchMode.value !== "semantic"
      ) return;

      semanticSearchResults.value = response.hits.map(semanticHitToSummary);
      indexMayBeCatchingUp.value = response.indexMayBeCatchingUp;
    } catch (error) {
      if (
        generation === semanticSearchGeneration
        && semanticRequestId.value === requestId
        && searchMode.value === "semantic"
      ) {
        const parsed = parseDiaryError(error);
        if (parsed.code !== "DIARY_CANCELLED") searchError.value = parsed;
      }
    } finally {
      if (
        generation === semanticSearchGeneration
        && semanticRequestId.value === requestId
        && searchMode.value === "semantic"
      ) {
        semanticRequestId.value = null;
        searchLoading.value = false;
      }
    }
  }

  async function openNote(key: DiaryNoteKey, refresh = false): Promise<boolean> {
    const target = cloneKey(key);
    const generation = ++documentGeneration;
    const sameTarget = sameNoteKey(document.value?.key ?? null, target);
    documentTarget.value = target;
    documentError.value = null;
    screen.value = "reader";
    if (refresh && sameTarget && document.value) documentRefreshing.value = true;
    else {
      documentLoading.value = true;
      if (!sameTarget) document.value = null;
    }

    try {
      const response = await invoke<DiaryDocument>("diary_get_note", { key: target });
      if (generation !== documentGeneration || !sameNoteKey(response.key, target)) return false;
      if (isTombstoned(target)) return false;

      document.value = response;
      baselineContent.value = response.content;
      baselineHash.value = response.contentHash;
      draft.value = response.content;
      saveState.value = "idle";
      saveError.value = null;
      return true;
    } catch (error) {
      if (generation === documentGeneration) {
        documentError.value = parseDiaryError(error);
      }
      return false;
    } finally {
      if (generation === documentGeneration) {
        documentLoading.value = false;
        documentRefreshing.value = false;
      }
    }
  }

  function leaveReader(): void {
    ++documentGeneration;
    documentLoading.value = false;
    documentRefreshing.value = false;
    documentTarget.value = null;
    screen.value = "list";
  }

  function startEditing(): void {
    if (!document.value) return;
    draft.value = document.value.content;
    baselineContent.value = document.value.content;
    baselineHash.value = document.value.contentHash;
    saveState.value = "idle";
    saveError.value = null;
    screen.value = "editor";
  }

  function setDraft(value: string): void {
    draft.value = value;
    if (saveState.value !== "saving") {
      saveState.value = value === baselineContent.value ? "idle" : "dirty";
      saveError.value = null;
    }
  }

  function showPreview(): void {
    if (screen.value === "editor") screen.value = "preview";
  }

  function returnToEditor(): void {
    if (screen.value === "preview") screen.value = "editor";
  }

  function discardDraft(): void {
    draft.value = baselineContent.value;
    saveState.value = "idle";
    saveError.value = null;
    screen.value = "reader";
  }

  async function saveDraft(force = false): Promise<DiarySaveOutcome | null> {
    if (!document.value || isSaving.value) return null;
    const operationId = newRequestId("mutation");
    const key = cloneKey(document.value.key);
    const content = draft.value;
    const hash = baselineHash.value;
    activeMutation.value = operationId;
    saveState.value = "saving";
    saveError.value = null;

    try {
      const outcome = await invoke<DiarySaveOutcome>("diary_save_note", {
        request: { key, content, baselineHash: hash, force },
      });
      if (!sameNoteKey(document.value?.key ?? null, key)) return outcome;

      document.value = { key, content, contentHash: outcome.contentHash };
      baselineContent.value = content;
      baselineHash.value = outcome.contentHash;
      saveState.value = draft.value === content ? "saved" : "dirty";
      indexMayBeCatchingUp.value = true;
      if (selectedFolder.value === key.folder) void loadNotes(key.folder, true);
      return outcome;
    } catch (error) {
      const parsed = parseDiaryError(error);
      saveError.value = parsed;
      saveState.value = parsed.code === "DIARY_CONFLICT"
        ? "conflict"
        : parsed.code === "DIARY_SAVE_UNCERTAIN"
          ? "uncertain"
          : "error";
      return null;
    } finally {
      if (activeMutation.value === operationId) activeMutation.value = null;
    }
  }

  async function loadRemoteDraft(): Promise<boolean> {
    if (!document.value || isSaving.value) return false;
    const key = cloneKey(document.value.key);
    const opened = await openNote(key, true);
    if (screen.value !== "reader" || !sameNoteKey(documentTarget.value, key)) return false;
    if (opened) {
      screen.value = "editor";
      saveState.value = "idle";
      saveError.value = null;
    } else {
      screen.value = "editor";
      saveState.value = "error";
      saveError.value = documentError.value ?? {
        code: "DIARY_UNKNOWN",
        message: "远端内容读取失败，当前草稿仍保留",
      };
    }
    return opened;
  }

  function markTombstone(key: DiaryNoteKey): void {
    const id = noteKeyId(key);
    if (!tombstones.value.includes(id)) tombstones.value.push(id);
    tombstoneDeadlines.set(id, Date.now() + TOMBSTONE_RETENTION_MS);
  }

  function removeKeysEverywhere(keys: DiaryNoteKey[]): void {
    const ids = new Set(keys.map(noteKeyId));
    notes.value = notes.value.filter((item) => !ids.has(noteKeyId(item)));
    textSearchResults.value = textSearchResults.value.filter((item) => !ids.has(noteKeyId(item)));
    semanticSearchResults.value = semanticSearchResults.value.filter((item) => !ids.has(noteKeyId(item)));
  }

  async function renameNote(targetFile: string): Promise<DiaryRenameOutcome | null> {
    if (!document.value || activeMutation.value) return null;
    const operationId = newRequestId("mutation");
    const source = cloneKey(document.value.key);
    const operationHash = baselineHash.value;
    activeMutation.value = operationId;
    mutationError.value = null;

    try {
      const outcome = await invoke<DiaryRenameOutcome>("diary_rename_note", {
        request: { source, targetFile: targetFile.trim(), baselineHash: operationHash },
      });
      const keepSource = outcome.status === "copied_source_retained";
      if (!keepSource) markTombstone(source);
      clearTombstone(outcome.key);

      notes.value = replaceSummaryKey(notes.value, source, outcome.key, keepSource);
      textSearchResults.value = replaceSummaryKey(textSearchResults.value, source, outcome.key, keepSource);
      semanticSearchResults.value = replaceSummaryKey(semanticSearchResults.value, source, outcome.key, keepSource);

      if (sameNoteKey(document.value?.key ?? null, source) && document.value) {
        document.value = {
          ...document.value,
          key: cloneKey(outcome.key),
          contentHash: outcome.contentHash,
        };
        documentTarget.value = cloneKey(outcome.key);
        baselineHash.value = outcome.contentHash;
      }
      indexMayBeCatchingUp.value = true;
      return outcome;
    } catch (error) {
      mutationError.value = parseDiaryError(error);
      return null;
    } finally {
      if (activeMutation.value === operationId) activeMutation.value = null;
    }
  }

  function startComposer(): void {
    const initial = makeComposer(selectedFolder.value);
    composerDraft.value = initial;
    composerBaseline.value = cloneComposer(initial);
    composerError.value = null;
    screen.value = "composer";
  }

  function discardComposer(): void {
    composerDraft.value = cloneComposer(composerBaseline.value);
    composerError.value = null;
    screen.value = "list";
  }

  async function createNote(): Promise<DiaryCreateOutcome | null> {
    if (
      composerSubmitting.value
      || activeMutation.value
      || composerError.value?.code === "DIARY_CREATE_UNCERTAIN"
    ) return null;
    const operationId = newRequestId("mutation");
    const snapshot = cloneComposer(composerDraft.value);
    composerSubmitting.value = true;
    activeMutation.value = operationId;
    composerError.value = null;

    try {
      const outcome = await invoke<DiaryCreateOutcome>("diary_create_note", {
        request: {
          maid: snapshot.maid,
          date: snapshot.date,
          folder: snapshot.folder.trim() || null,
          fileNameSuffix: snapshot.fileNameSuffix.trim() || null,
          tag: snapshot.tag.trim() || null,
          content: snapshot.content,
        },
      });

      if (!folders.value.includes(outcome.key.folder)) {
        folders.value.push(outcome.key.folder);
        rememberFolderOrder();
      }
      clearTombstone(outcome.key);
      selectedFolder.value = outcome.key.folder;
      notesFolder.value = "";
      composerBaseline.value = cloneComposer(snapshot);
      cancelSearch();
      indexMayBeCatchingUp.value = true;
      await openNote(outcome.key, false);
      void loadNotes(outcome.key.folder, false);
      return outcome;
    } catch (error) {
      composerError.value = parseDiaryError(error);
      return null;
    } finally {
      composerSubmitting.value = false;
      if (activeMutation.value === operationId) activeMutation.value = null;
    }
  }

  function enterSelection(key?: DiaryNoteKey): void {
    selectionMode.value = true;
    if (key) {
      const id = noteKeyId(key);
      if (!selectedKeyIds.value.includes(id)) selectedKeyIds.value.push(id);
    }
  }

  function toggleSelection(key: DiaryNoteKey): void {
    const id = noteKeyId(key);
    if (selectedKeyIds.value.includes(id)) {
      selectedKeyIds.value = selectedKeyIds.value.filter((item) => item !== id);
    } else {
      selectedKeyIds.value.push(id);
    }
  }

  function clearSelection(): void {
    selectionMode.value = false;
    selectedKeyIds.value = [];
  }

  async function moveNotes(keys: DiaryNoteKey[], targetFolder: string): Promise<DiaryBatchOutcome | null> {
    if (keys.length === 0 || activeMutation.value) return null;
    const operationId = newRequestId("mutation");
    const sources = keys.map(cloneKey);
    activeMutation.value = operationId;
    mutationError.value = null;
    lastBatchOutcome.value = null;

    try {
      const outcome = await invoke<DiaryBatchOutcome>("diary_move_notes", {
        request: { sources, targetFolder },
      });
      outcome.succeeded.forEach(markTombstone);
      outcome.succeeded.forEach((source) => {
        clearTombstone({ folder: targetFolder, file: source.file });
      });
      removeKeysEverywhere(outcome.succeeded);
      selectedKeyIds.value = outcome.errors.map((item) => noteKeyId(item.key));
      selectionMode.value = outcome.errors.length > 0;
      lastBatchOutcome.value = outcome;

      const current = document.value?.key;
      if (current && outcome.succeeded.some((key) => sameNoteKey(key, current)) && document.value) {
        document.value = {
          ...document.value,
          key: { folder: targetFolder, file: current.file },
        };
        documentTarget.value = cloneKey(document.value.key);
      }
      indexMayBeCatchingUp.value = true;
      return outcome;
    } catch (error) {
      mutationError.value = parseDiaryError(error);
      return null;
    } finally {
      if (activeMutation.value === operationId) activeMutation.value = null;
    }
  }

  async function deleteNotes(keys: DiaryNoteKey[]): Promise<DiaryBatchOutcome | null> {
    if (keys.length === 0 || activeMutation.value) return null;
    const operationId = newRequestId("mutation");
    const sources = keys.map(cloneKey);
    activeMutation.value = operationId;
    mutationError.value = null;
    lastBatchOutcome.value = null;

    try {
      const outcome = await invoke<DiaryBatchOutcome>("diary_delete_notes", {
        request: { sources },
      });
      outcome.succeeded.forEach(markTombstone);
      removeKeysEverywhere(outcome.succeeded);
      selectedKeyIds.value = outcome.errors.map((item) => noteKeyId(item.key));
      selectionMode.value = outcome.errors.length > 0;
      lastBatchOutcome.value = outcome;

      if (
        document.value
        && outcome.succeeded.some((key) => sameNoteKey(key, document.value?.key ?? null))
      ) {
        ++documentGeneration;
        document.value = null;
        documentTarget.value = null;
        screen.value = "list";
      }
      indexMayBeCatchingUp.value = true;
      return outcome;
    } catch (error) {
      mutationError.value = parseDiaryError(error);
      return null;
    } finally {
      if (activeMutation.value === operationId) activeMutation.value = null;
    }
  }

  async function deleteEmptyFolder(folder: string): Promise<boolean> {
    if (!folder || activeMutation.value) return false;
    const operationId = newRequestId("mutation");
    activeMutation.value = operationId;
    mutationError.value = null;
    try {
      await invoke<void>("diary_delete_empty_folder", { request: { folder } });
      folders.value = folders.value.filter((item) => item !== folder);
      hiddenFolders.value = hiddenFolders.value.filter((item) => item !== folder);
      folderOrder.value = folderOrder.value.filter((item) => item !== folder);
      if (selectedFolder.value === folder) {
        selectedFolder.value = visibleFolders.value[0] ?? "";
        notes.value = [];
        notesFolder.value = "";
        if (selectedFolder.value) await loadNotes(selectedFolder.value, false);
      }
      return true;
    } catch (error) {
      mutationError.value = parseDiaryError(error);
      return false;
    } finally {
      if (activeMutation.value === operationId) activeMutation.value = null;
    }
  }

  function hideFolder(folder: string): void {
    if (!hiddenFolders.value.includes(folder)) hiddenFolders.value.push(folder);
    if (selectedFolder.value === folder) {
      const next = visibleFolders.value[0] ?? "";
      selectedFolder.value = next;
      notes.value = [];
      notesFolder.value = "";
      if (next) void loadNotes(next, false);
    }
  }

  function restoreFolder(folder: string): void {
    hiddenFolders.value = hiddenFolders.value.filter((item) => item !== folder);
  }

  function toggleCategoryCollapsed(category: DiaryFolderCategory): void {
    if (collapsedCategories.value.includes(category)) {
      collapsedCategories.value = collapsedCategories.value.filter((item) => item !== category);
    } else {
      collapsedCategories.value.push(category);
    }
  }

  function setFolderOrder(order: string[]): void {
    const known = new Set(folders.value);
    const unique = [...new Set(order)].filter((folder) => known.has(folder));
    for (const folder of folders.value) {
      if (!unique.includes(folder)) unique.push(folder);
    }
    folderOrder.value = unique;
  }

  function clearOperationMessages(): void {
    mutationError.value = null;
    lastBatchOutcome.value = null;
  }

  return {
    initialized,
    folders,
    foldersLoading,
    foldersRefreshing,
    foldersError,
    selectedFolder,
    notesFolder,
    notes,
    notesLoading,
    notesRefreshing,
    notesError,
    screen,
    document,
    documentTarget,
    documentLoading,
    documentRefreshing,
    documentError,
    searchMode,
    searchScope,
    searchQuery,
    textSearchResults,
    semanticSearchResults,
    searchLoading,
    searchError,
    searchLimited,
    indexMayBeCatchingUp,
    draft,
    baselineContent,
    baselineHash,
    saveState,
    saveError,
    composerDraft,
    composerSubmitting,
    composerError,
    selectionMode,
    selectedKeyIds,
    tombstones,
    activeMutation,
    mutationError,
    lastBatchOutcome,
    hiddenFolders,
    collapsedCategories,
    folderOrder,
    orderedFolders,
    visibleFolders,
    displayedNotes,
    selectedKeys,
    draftDirty,
    composerDirty,
    isSaving,
    hasInternalState,
    initialize,
    loadFolders,
    loadNotes,
    selectFolder,
    setSearchMode,
    setSearchScope,
    invalidateSearchInput,
    runTextSearch,
    runSemanticSearch,
    cancelTextSearch,
    cancelSemanticSearch,
    cancelSearch,
    openNote,
    leaveReader,
    startEditing,
    setDraft,
    showPreview,
    returnToEditor,
    discardDraft,
    saveDraft,
    loadRemoteDraft,
    renameNote,
    startComposer,
    discardComposer,
    createNote,
    enterSelection,
    toggleSelection,
    clearSelection,
    moveNotes,
    deleteNotes,
    deleteEmptyFolder,
    hideFolder,
    restoreFolder,
    toggleCategoryCollapsed,
    setFolderOrder,
    clearOperationMessages,
  };
}, {
  persist: {
    pick: ["hiddenFolders", "collapsedCategories", "folderOrder"],
  },
});
