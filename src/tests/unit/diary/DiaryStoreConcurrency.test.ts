import { beforeEach, describe, expect, it } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useDiaryStore } from "@/features/diary/diaryStore";
import type {
  DiaryBatchOutcome,
  DiaryDocument,
  DiaryNoteSummary,
  DiarySaveOutcome,
  DiarySearchResponse,
  DiarySemanticResponse,
} from "@/features/diary/types";
import { invokeMock, mockInvoke } from "@/tests/mocks/tauri";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function note(folder: string, file: string, preview = file): DiaryNoteSummary {
  return { folder, file, preview, lastModified: "2026-08-12T10:00:00.000Z" };
}

function document(folder: string, file: string, content: string, contentHash: string): DiaryDocument {
  return { key: { folder, file }, content, contentHash };
}

describe("diary store ownership and mutation guards", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("rejects stale success and stale finally across folder A to B to A", async () => {
    const requests = [
      deferred<DiaryNoteSummary[]>(),
      deferred<DiaryNoteSummary[]>(),
      deferred<DiaryNoteSummary[]>(),
    ];
    let index = 0;
    mockInvoke("diary_list_notes", () => requests[index++].promise);
    const store = useDiaryStore();

    store.selectedFolder = "A";
    const firstA = store.loadNotes("A", false);
    store.selectedFolder = "B";
    const loadB = store.loadNotes("B", false);
    store.selectedFolder = "A";
    const latestA = store.loadNotes("A", false);

    requests[0].resolve([note("A", "stale-a.txt")]);
    requests[1].resolve([note("B", "stale-b.txt")]);
    await Promise.all([firstA, loadB]);
    expect(store.notes).toEqual([]);
    expect(store.notesLoading).toBe(true);

    requests[2].resolve([note("A", "latest-a.txt")]);
    await latestA;
    expect(store.notes.map((item) => item.file)).toEqual(["latest-a.txt"]);
    expect(store.notesFolder).toBe("A");
    expect(store.notesLoading).toBe(false);
  });

  it("keeps the latest document when stale A and B requests finish later", async () => {
    const requests = [
      deferred<DiaryDocument>(),
      deferred<DiaryDocument>(),
      deferred<DiaryDocument>(),
    ];
    let index = 0;
    mockInvoke("diary_get_note", () => requests[index++].promise);
    const store = useDiaryStore();

    const firstA = store.openNote({ folder: "F", file: "a.txt" });
    const loadB = store.openNote({ folder: "F", file: "b.txt" });
    const latestA = store.openNote({ folder: "F", file: "a.txt" });

    requests[2].resolve(document("F", "a.txt", "latest", "hash-latest"));
    await latestA;
    requests[0].resolve(document("F", "a.txt", "stale-a", "hash-a"));
    requests[1].resolve(document("F", "b.txt", "stale-b", "hash-b"));
    await Promise.all([firstA, loadB]);

    expect(store.document?.content).toBe("latest");
    expect(store.document?.contentHash).toBe("hash-latest");
    expect(store.documentLoading).toBe(false);
  });

  it("gates text search results and cancellation by request id", async () => {
    const requests = [
      deferred<DiarySearchResponse>(),
      deferred<DiarySearchResponse>(),
      deferred<DiarySearchResponse>(),
      deferred<DiarySearchResponse>(),
    ];
    let index = 0;
    mockInvoke("diary_search", () => requests[index++].promise);
    mockInvoke("diary_cancel_search", () => undefined);
    const store = useDiaryStore();
    store.selectedFolder = "F";

    const first = store.runTextSearch("first");
    const firstRequest = invokeMock.mock.calls.find(([command]) => command === "diary_search")?.[1]
      ?.request as { requestId: string };
    const second = store.runTextSearch("second");

    requests[1].resolve({ notes: [note("F", "second.txt")], total: 1, limited: false });
    await second;
    requests[0].resolve({ notes: [note("F", "first.txt")], total: 1, limited: false });
    await first;
    expect(store.displayedNotes.map((item) => item.file)).toEqual(["second.txt"]);

    const oversized = store.runTextSearch("many");
    requests[2].resolve({
      notes: Array.from({ length: 201 }, (_, item) => note("F", `${item}.txt`)),
      total: 201,
      limited: false,
    });
    await oversized;
    expect(store.displayedNotes).toHaveLength(200);
    expect(store.searchLimited).toBe(true);

    const pending = store.runTextSearch("cancel me");
    const latestSearchCall = [...invokeMock.mock.calls]
      .reverse()
      .find(([command]) => command === "diary_search");
    const latestRequestId = (latestSearchCall?.[1]?.request as { requestId: string }).requestId;
    store.cancelTextSearch();
    const cancelCall = [...invokeMock.mock.calls]
      .reverse()
      .find(([command]) => command === "diary_cancel_search");
    expect((cancelCall?.[1]?.request as { requestId: string }).requestId).toBe(latestRequestId);
    expect(latestRequestId).not.toBe(firstRequest.requestId);
    expect(store.searchLoading).toBe(false);
    requests[3].resolve({ notes: [note("F", "cancelled.txt")], total: 1, limited: false });
    await pending;
    expect(store.displayedNotes).toHaveLength(200);
  });

  it("invalidates an in-flight text result as soon as the input changes", async () => {
    const search = deferred<DiarySearchResponse>();
    mockInvoke("diary_search", () => search.promise);
    mockInvoke("diary_cancel_search", () => undefined);
    const store = useDiaryStore();
    store.selectedFolder = "F";

    const pending = store.runTextSearch("old query");
    store.searchQuery = "new query";
    store.invalidateSearchInput();
    search.resolve({ notes: [note("F", "stale.txt")], total: 1, limited: false });
    await pending;

    expect(store.textSearchResults).toEqual([]);
    expect(store.searchLoading).toBe(false);
    expect(invokeMock.mock.calls.some(([command]) => command === "diary_cancel_search"))
      .toBe(true);
  });

  it("keeps semantic scope and cancellation owned by the current request", async () => {
    const search = deferred<DiarySemanticResponse>();
    mockInvoke("diary_semantic_search", () => search.promise);
    mockInvoke("diary_cancel_semantic_search", () => undefined);
    const store = useDiaryStore();
    store.selectedFolder = "F";
    store.setSearchScope("all");

    const pending = store.runSemanticSearch("memory", 99);
    const call = invokeMock.mock.calls.find(([command]) => command === "diary_semantic_search");
    expect(call?.[1]?.request).toMatchObject({
      query: "memory",
      folder: null,
      searchAll: true,
      k: 50,
    });

    store.searchQuery = "changed";
    store.invalidateSearchInput();
    search.resolve({
      hits: [{ key: { folder: "F", file: "stale.txt" }, preview: "stale", score: 0.9 }],
      indexMayBeCatchingUp: true,
    });
    await pending;

    expect(store.semanticSearchResults).toEqual([]);
    expect(store.indexMayBeCatchingUp).toBe(false);
    expect(invokeMock.mock.calls.some(([command]) => command === "diary_cancel_semantic_search"))
      .toBe(true);
  });

  it("does not display cached results from a previous search mode", () => {
    const store = useDiaryStore();
    store.textSearchResults = [note("F", "old-text.txt")];
    store.semanticSearchResults = [note("F", "old-semantic.txt")];

    store.setSearchMode("text");
    expect(store.displayedNotes).toEqual([]);
    store.textSearchResults = [note("F", "current-text.txt")];
    store.setSearchMode("semantic");
    expect(store.displayedNotes).toEqual([]);
  });

  it("commits only the immutable save snapshot when typing continues", async () => {
    const save = deferred<DiarySaveOutcome>();
    mockInvoke("diary_save_note", () => save.promise);
    const store = useDiaryStore();
    store.document = document("F", "a.txt", "remote", "baseline-hash");
    store.startEditing();
    store.setDraft("snapshot");

    const saving = store.saveDraft(false);
    store.setDraft("newer typing");
    save.resolve({ contentHash: "snapshot-hash", verified: true });
    await saving;

    expect(store.document?.content).toBe("snapshot");
    expect(store.baselineContent).toBe("snapshot");
    expect(store.draft).toBe("newer typing");
    expect(store.saveState).toBe("dirty");
  });

  it("keeps a conflict draft and exposes an explicit conflict state", async () => {
    mockInvoke("diary_save_note", () => {
      throw new Error("DIARY_CONFLICT: 远端内容已变化");
    });
    const store = useDiaryStore();
    store.document = document("F", "a.txt", "remote", "baseline-hash");
    store.startEditing();
    store.setDraft("local draft");

    await store.saveDraft(false);
    expect(store.draft).toBe("local draft");
    expect(store.saveState).toBe("conflict");
    expect(store.saveError?.code).toBe("DIARY_CONFLICT");
  });

  it("does not resurrect the editor after a late remote-draft reload", async () => {
    const reload = deferred<DiaryDocument>();
    mockInvoke("diary_get_note", () => reload.promise);
    const store = useDiaryStore();
    store.document = document("F", "a.txt", "baseline", "baseline-hash");
    store.startEditing();
    store.setDraft("local draft");

    const pending = store.loadRemoteDraft();
    store.leaveReader();
    reload.resolve(document("F", "a.txt", "remote", "remote-hash"));

    expect(await pending).toBe(false);
    expect(store.screen).toBe("list");
    expect(store.draft).toBe("local draft");
  });

  it("tombstones successful deletes while retaining failed selections", async () => {
    const outcome: DiaryBatchOutcome = {
      succeeded: [{ folder: "F", file: "ok.txt" }],
      errors: [{ key: { folder: "F", file: "failed.txt" }, message: "locked" }],
    };
    mockInvoke("diary_delete_notes", () => outcome);
    const store = useDiaryStore();
    store.selectedFolder = "F";
    store.notes = [note("F", "ok.txt"), note("F", "failed.txt")];
    store.enterSelection({ folder: "F", file: "ok.txt" });
    store.toggleSelection({ folder: "F", file: "failed.txt" });

    await store.deleteNotes([
      { folder: "F", file: "ok.txt" },
      { folder: "F", file: "failed.txt" },
    ]);

    expect(store.displayedNotes.map((item) => item.file)).toEqual(["failed.txt"]);
    expect(store.tombstones).toContain("F\u0000ok.txt");
    expect(store.selectedKeyIds).toEqual(["F\u0000failed.txt"]);
    expect(store.selectionMode).toBe(true);
  });

  it("retains a delete tombstone across list reconciliation to hide stale semantic hits", async () => {
    mockInvoke("diary_delete_notes", () => ({
      succeeded: [{ folder: "F", file: "gone.txt" }],
      errors: [],
    }));
    mockInvoke("diary_list_notes", () => [note("F", "live.txt")]);
    const store = useDiaryStore();
    store.selectedFolder = "F";
    store.notes = [note("F", "gone.txt")];

    await store.deleteNotes([{ folder: "F", file: "gone.txt" }]);
    await store.loadNotes("F", true);
    store.setSearchMode("semantic");
    store.semanticSearchResults = [note("F", "gone.txt"), note("F", "live.txt")];

    expect(store.displayedNotes.map((item) => item.file)).toEqual(["live.txt"]);
  });

  it("blocks direct retry after an uncertain create result", async () => {
    const create = deferred<never>();
    mockInvoke("diary_create_note", () => create.promise);
    const store = useDiaryStore();
    store.startComposer();
    store.composerDraft = {
      maid: "Nova",
      date: "2026-08-12",
      folder: "F",
      fileNameSuffix: "note",
      tag: "",
      content: "content",
    };

    const pending = store.createNote();
    expect(store.activeMutation).toBeTruthy();
    expect(store.hasInternalState).toBe(true);
    create.reject(new Error("DIARY_CREATE_UNCERTAIN: 请刷新核对"));
    await pending;
    expect(store.composerError?.code).toBe("DIARY_CREATE_UNCERTAIN");

    await store.createNote();
    expect(invokeMock.mock.calls.filter(([command]) => command === "diary_create_note"))
      .toHaveLength(1);
  });
});
