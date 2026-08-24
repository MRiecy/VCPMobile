import { beforeEach, describe, expect, it } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useGlobalSearchStore } from "@/features/globalsearch/globalSearchStore";
import type { FtsSearchResultItem } from "@/features/globalsearch/types";
import { clearInvokeMocks, invokeMock, mockInvoke } from "@/tests/mocks/tauri";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function item(msgId: string, timestamp: number, topicId = "t1"): FtsSearchResultItem {
  return {
    msgId,
    topicId,
    role: "assistant",
    speakerName: "Agent",
    timestamp,
    topicTitle: "话题",
    ownerId: "a1",
    ownerType: "agent",
    snippet: `含<mark>关键词</mark>的摘要 ${msgId}`,
  };
}

function makePage(start: number, count: number): FtsSearchResultItem[] {
  return Array.from({ length: count }, (_, i) => item(`m${start + i}`, start + i));
}

describe("globalSearchStore", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    clearInvokeMocks();
  });

  it("drops stale search responses via generation guard", async () => {
    const first = deferred<FtsSearchResultItem[]>();
    const second = deferred<FtsSearchResultItem[]>();
    const requests = [first, second];
    let index = 0;
    mockInvoke("search_messages_fts", () => requests[index++].promise);

    const store = useGlobalSearchStore();
    store.query = "机器学习";
    const p1 = store.search();
    store.query = "部署方案";
    const p2 = store.search();

    // 后发起的先返回
    second.resolve([item("new", 200)]);
    await p2;
    expect(store.results.map((r) => r.msgId)).toEqual(["new"]);
    expect(store.searching).toBe(false);

    // 先发起的迟到响应必须被丢弃
    first.resolve([item("stale", 100)]);
    await p1;
    expect(store.results.map((r) => r.msgId)).toEqual(["new"]);
  });

  it("maps owner/message type/speaker/time filters to the backend contract", async () => {
    mockInvoke("search_messages_fts", () => []);
    const store = useGlobalSearchStore();
    store.query = "部署";
    store.scope = "owner";
    store.scopeOwnerId = "agent-42";
    store.scopeOwnerType = "agent";
    store.speakerAgentId = "speaker-7";
    store.role = "assistant";
    store.timeRange = "custom";
    store.customStartDate = "2026-08-01";
    store.customEndDate = "2026-08-24";
    store.sort = "rank";

    await store.search();

    const args = invokeMock.mock.calls[0][1] as { filter: Record<string, unknown> };
    expect(args.filter.ownerId).toBe("agent-42");
    expect(args.filter.ownerType).toBe("agent");
    expect(args.filter.agentId).toBe("speaker-7");
    expect(args.filter.role).toBe("assistant");
    expect(args.filter.sort).toBe("rank");
    expect(typeof args.filter.startTime).toBe("number");
    expect(typeof args.filter.endTime).toBe("number");
    expect(Number(args.filter.endTime)).toBeGreaterThan(Number(args.filter.startTime));
    expect(args.filter.topicId).toBeNull();
  });

  it("paginates with keyset cursor from the last result", async () => {
    const pages = [makePage(1000, 50), makePage(900, 20)];
    let index = 0;
    mockInvoke("search_messages_fts", () => pages[index++]);

    const store = useGlobalSearchStore();
    store.query = "关键词";
    await store.search();
    expect(store.results).toHaveLength(50);
    expect(store.limited).toBe(true);

    await store.loadMore();
    const secondCall = invokeMock.mock.calls[1][1] as { filter: Record<string, unknown> };
    expect(secondCall.filter.beforeTimestamp).toBe(1049);
    expect(secondCall.filter.beforeMessageId).toBe("m1049");
    expect(store.results).toHaveLength(70);
    expect(store.limited).toBe(false);
  });

  it("does not paginate in rank sort mode", async () => {
    mockInvoke("search_messages_fts", () => makePage(1000, 50));
    const store = useGlobalSearchStore();
    store.query = "关键词";
    store.sort = "rank";
    await store.search();
    expect(store.limited).toBe(true);

    await store.loadMore();
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });

  it("releases a stale load-more request when a new search starts", async () => {
    const stalePage = deferred<FtsSearchResultItem[]>();
    const freshSearch = deferred<FtsSearchResultItem[]>();
    const responses = [Promise.resolve(makePage(1000, 50)), stalePage.promise, freshSearch.promise];
    let index = 0;
    mockInvoke("search_messages_fts", () => responses[index++]);

    const store = useGlobalSearchStore();
    store.query = "旧关键词";
    await store.search();

    const staleRequest = store.loadMore();
    expect(store.loadingMore).toBe(true);

    store.query = "新关键词";
    const freshRequest = store.search();
    expect(store.loadingMore).toBe(false);

    freshSearch.resolve([item("fresh", 2000)]);
    await freshRequest;
    stalePage.resolve([]);
    await staleRequest;

    expect(store.results.map((result) => result.msgId)).toEqual(["fresh"]);
    expect(store.loadingMore).toBe(false);
  });

  it("ignores queries shorter than the minimum without invoking backend", async () => {
    mockInvoke("search_messages_fts", () => makePage(1, 10));
    const store = useGlobalSearchStore();
    store.query = "短";
    await store.search();
    expect(invokeMock).not.toHaveBeenCalled();
    expect(store.results).toEqual([]);
    expect(store.hasSearched).toBe(false);
  });

});
