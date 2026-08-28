import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { nextTick } from "vue";
import { mount } from "@vue/test-utils";
import { useAssistantStore } from "@/core/stores/assistant";
import { useChatSessionStore } from "@/core/stores/chatSessionStore";
import { useTopicStore } from "@/core/stores/topicListManager";
import TopicList from "@/features/topic/TopicList.vue";
import {
  channelInstances,
  invokeMock,
  mockInvoke,
} from "@/tests/mocks/tauri";

vi.mock("vue-router", () => ({
  useRouter: () => ({
    currentRoute: { value: { path: "/chat" } },
    push: vi.fn(),
  }),
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

const topic = (id: string, name = id) => ({
  id,
  name,
  createdAt: 1,
  locked: true,
  unread: false,
  unreadCount: 0,
  msgCount: 0,
  ownerId: "agent-a",
  ownerType: "agent" as const,
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

  it("watches semantic owner identity instead of owner object identity", async () => {
    const requests = [deferred<void>(), deferred<void>()];
    let callIndex = 0;
    mockInvoke("get_topics_streamed", () => requests[callIndex++].promise);
    const assistantStore = useAssistantStore();
    assistantStore.agents = [
      {
        id: "agent-a",
        name: "A",
        model: "test",
        avatarCalculatedColor: null,
      },
      {
        id: "agent-b",
        name: "B",
        model: "test",
        avatarCalculatedColor: null,
      },
    ];
    const sessionStore = useChatSessionStore();
    sessionStore.setConversation(
      { id: "agent-a", name: "A", type: "agent" },
      "topic-a",
    );

    const wrapper = mount(TopicList, {
      global: { directives: { longpress: {} } },
    });
    await expect.poll(() => callIndex).toBe(1);

    sessionStore.currentSelectedItem = {
      id: "agent-a",
      name: "A refreshed",
      type: "agent",
    };
    await expect.poll(() => callIndex).toBe(1);

    sessionStore.currentSelectedItem = {
      id: "agent-b",
      name: "B",
      type: "agent",
    };
    await expect.poll(() => callIndex).toBe(2);

    wrapper.unmount();
    requests.forEach(({ resolve }) => resolve());
    await Promise.all(requests.map(({ promise }) => promise));
  });

  it("resets the virtual window when search narrows to an offscreen topic", async () => {
    const store = useTopicStore();
    store.topics = Array.from({ length: 60 }, (_, index) =>
      topic(
        `topic-${index}`,
        index === 55 ? "Needle Topic" : `Topic ${index}`,
      ),
    );

    const wrapper = mount(TopicList, {
      props: { searchQuery: "" },
      global: { directives: { longpress: {} } },
    });
    const scroller = wrapper.find<HTMLElement>(".vcp-scrollable");
    await nextTick();
    Object.defineProperty(scroller.element, "clientHeight", {
      configurable: true,
      value: 296,
    });
    scroller.element.scrollTop = 40 * 74;
    await scroller.trigger("scroll");
    expect(wrapper.text()).not.toContain("Needle Topic");

    await wrapper.setProps({ searchQuery: "needle" });

    await expect.poll(() => scroller.element.scrollTop).toBe(0);
    await expect.poll(() => wrapper.text()).toContain("Needle Topic");
    wrapper.unmount();
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
