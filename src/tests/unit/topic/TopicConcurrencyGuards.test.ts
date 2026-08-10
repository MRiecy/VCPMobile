import { beforeEach, describe, expect, it } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useTopicStore } from "@/core/stores/topicListManager";
import {
  channelInstances,
  invokeMock,
  mockInvoke,
} from "@/tests/mocks/tauri";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

const topic = (id: string) => ({
  id,
  name: id,
  createdAt: 1,
});

describe("topic list concurrency guards", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("deduplicates concurrent loads for the same owner", async () => {
    const pending = deferred<void>();
    mockInvoke("get_topics_streamed", () => pending.promise);
    const store = useTopicStore();

    const first = store.loadTopicList("agent-a", "agent");
    const second = store.loadTopicList("agent-a", "agent");

    expect(
      invokeMock.mock.calls.filter(([command]) => command === "get_topics_streamed"),
    ).toHaveLength(1);
    pending.resolve();
    await Promise.all([first, second]);
  });

  it("rejects stale chunks and stale finally across A to B to A", async () => {
    const requests = [deferred<void>(), deferred<void>(), deferred<void>()];
    let callIndex = 0;
    mockInvoke("get_topics_streamed", () => requests[callIndex++].promise);
    const store = useTopicStore();

    const firstA = store.loadTopicList("agent-a", "agent");
    const loadB = store.loadTopicList("agent-b", "agent");
    const latestA = store.loadTopicList("agent-a", "agent");

    channelInstances[0].emit([topic("stale-a")]);
    channelInstances[1].emit([topic("stale-b")]);
    expect(store.topics).toEqual([]);

    channelInstances[2].emit([topic("latest-a")]);
    expect(store.topics.map((item) => item.id)).toEqual(["latest-a"]);

    requests[0].resolve();
    requests[1].resolve();
    await Promise.all([firstA, loadB]);
    expect(store.loading).toBe(true);

    requests[2].resolve();
    await latestA;
    expect(store.loading).toBe(false);
  });

  it("invalidates an in-flight channel and allows an explicit same-owner reload", async () => {
    const requests = [deferred<void>(), deferred<void>()];
    let callIndex = 0;
    mockInvoke("get_topics_streamed", () => requests[callIndex++].promise);
    const store = useTopicStore();

    const stale = store.loadTopicList("agent-a", "agent");
    store.invalidateAllTopicCaches();
    const fresh = store.loadTopicList("agent-a", "agent");

    channelInstances[0].emit([topic("stale")]);
    channelInstances[1].emit([topic("fresh")]);
    expect(store.topics.map((item) => item.id)).toEqual(["fresh"]);

    requests[0].resolve();
    requests[1].resolve();
    await Promise.all([stale, fresh]);
  });

  it("does not inject a late create result into a different owner's list", async () => {
    const loads = [deferred<void>(), deferred<void>()];
    const create = deferred<any>();
    let loadIndex = 0;
    mockInvoke("get_topics_streamed", () => loads[loadIndex++].promise);
    mockInvoke("create_topic", () => create.promise);
    const store = useTopicStore();

    const loadA = store.loadTopicList("agent-a", "agent");
    channelInstances[0].emit([topic("topic-a")]);
    loads[0].resolve();
    await loadA;

    const creatingA = store.createTopic("agent-a", "agent", "new-a");
    const loadB = store.loadTopicList("agent-b", "agent");
    channelInstances[1].emit([topic("topic-b")]);
    loads[1].resolve();
    await loadB;

    create.resolve(topic("created-a"));
    await creatingA;
    expect(store.topics.map((item) => item.id)).toEqual(["topic-b"]);
  });
});
