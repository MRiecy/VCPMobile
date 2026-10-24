import { beforeEach, describe, expect, it, vi } from "vitest";
import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { invoke } from "@tauri-apps/api/core";

import AgentList from "@/features/agent/AgentList.vue";
import { useAssistantStore } from "@/core/stores/assistant";
import { useChatStreamStore } from "@/core/stores/chatStreamStore";
import type { StreamEventDto } from "@/core/types/chat";

const streamEvent = (partial: Partial<StreamEventDto> & Pick<StreamEventDto, "type">): StreamEventDto => ({
  messageId: "",
  chunk: null,
  content: null,
  finishReason: null,
  error: null,
  aurora: null,
  blocks: null,
  timestamp: null,
  topicUpdatedAt: null,
  consumerRevision: 1,
  context: null,
  ...partial,
});

const sortableHarness = vi.hoisted(() => ({
  create: vi.fn(),
  instances: [] as Array<{
    destroy: ReturnType<typeof vi.fn>;
    option: ReturnType<typeof vi.fn>;
  }>,
}));

vi.mock("sortablejs", () => ({
  default: { create: sortableHarness.create },
}));

describe("AgentList drag lifecycle", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    sortableHarness.instances.length = 0;
    sortableHarness.create.mockReset();
    sortableHarness.create.mockImplementation(() => {
      const instance = {
        destroy: vi.fn(),
        option: vi.fn(),
      };
      sortableHarness.instances.push(instance);
      return instance;
    });
  });

  it("rebinds Sortable when a loading refresh replaces the list DOM", async () => {
    const assistantStore = useAssistantStore();
    assistantStore.agents = [{
      id: "agent-a",
      name: "Agent A",
      model: "test",
      avatarCalculatedColor: null,
    }];

    const wrapper = mount(AgentList, {
      props: { searchQuery: "" },
      global: {
        stubs: { VcpAvatar: true },
        directives: { guide: {} },
      },
    });

    await vi.waitFor(() => expect(sortableHarness.create).toHaveBeenCalledTimes(1));
    const firstInstance = sortableHarness.instances[0];
    expect(sortableHarness.create.mock.calls[0]?.[1]).toMatchObject({ disabled: false });

    assistantStore.loading = true;
    await vi.waitFor(() => expect(firstInstance.destroy).toHaveBeenCalledTimes(1));

    assistantStore.loading = false;
    await vi.waitFor(() => expect(sortableHarness.create).toHaveBeenCalledTimes(2));
    const reboundInstance = sortableHarness.instances[1];

    await wrapper.setProps({ searchQuery: "agent" });
    expect(reboundInstance.option).toHaveBeenLastCalledWith("disabled", true);
    await wrapper.setProps({ searchQuery: "" });
    expect(reboundInstance.option).toHaveBeenLastCalledWith("disabled", false);

    wrapper.unmount();
    expect(reboundInstance.destroy).toHaveBeenCalledTimes(1);
  });

  it("projects cross-topic generation and group turns onto sidebar avatar slots without false positives before inventory recovery", async () => {
    vi.mocked(invoke).mockImplementation(async (cmd) => {
      if (cmd === "get_unread_counts") return {};
      if (cmd === "increment_topic_unread_count") return { unreadCount: 1, updatedAt: 1 };
      return undefined;
    });
    const assistantStore = useAssistantStore();
    const streamStore = useChatStreamStore();
    assistantStore.agents = [
      { id: "agent-a", name: "Agent A", model: "m1", avatarCalculatedColor: null },
      { id: "agent-b", name: "Agent B", model: "m2", avatarCalculatedColor: null },
    ];
    assistantStore.groups = [
      { id: "group-1", name: "Group 1", members: ["agent-a", "agent-b"], mode: "sequential", avatarCalculatedColor: null },
    ];

    const wrapper = mount(AgentList, {
      props: { searchQuery: "" },
      global: {
        stubs: { VcpAvatar: true },
        directives: { guide: {} },
      },
    });

    const getSlots = () => wrapper.findAll(".vcp-sidebar-avatar-slot");
    // 初始未恢复完成 (activityInventoryKnown === false) 时，绝不误判为正在生成
    expect(streamStore.activityInventoryKnown).toBe(false);
    expect(getSlots().map((s) => s.attributes("data-generating"))).toEqual([
      "false", // group-1
      "false", // agent-a
      "false", // agent-b
    ]);

    // 1. 多话题汇总：agent-a 的 topic-1 与 topic-2 同时活动，一个结束不熄灭另一个
    const releaseOp = streamStore.beginConversationOperation("agent-a", "agent", "topic-1");
    await wrapper.vm.$nextTick();
    expect(getSlots().map((s) => s.attributes("data-generating"))).toEqual([
      "false",
      "true",
      "false",
    ]);

    await streamStore.processStreamEvent(streamEvent({
      type: "thinking",
      messageId: "msg-2",
      consumerRevision: 1,
      context: { ownerId: "agent-a", ownerType: "agent", topicId: "topic-2" },
    }));
    releaseOp();
    await wrapper.vm.$nextTick();
    expect(streamStore.isOwnerGenerating("agent-a", "agent")).toBe(true);

    // 可恢复中断 waiting_retry 时暂停生成态指示；重连恢复时重新点亮
    await streamStore.processStreamEvent(streamEvent({
      type: "error",
      messageId: "msg-2",
      consumerRevision: 2,
      error: "network blip",
      context: { ownerId: "agent-a", ownerType: "agent", topicId: "topic-2" },
    }));
    await wrapper.vm.$nextTick();
    expect(streamStore.isOwnerGenerating("agent-a", "agent")).toBe(false);

    await streamStore.processStreamEvent(streamEvent({
      type: "reconnecting",
      messageId: "msg-2",
      consumerRevision: 3,
      context: { ownerId: "agent-a", ownerType: "agent", topicId: "topic-2" },
    }));
    await wrapper.vm.$nextTick();
    expect(streamStore.isOwnerGenerating("agent-a", "agent")).toBe(true);

    await streamStore.processStreamEvent(streamEvent({
      type: "end",
      messageId: "msg-2",
      consumerRevision: 4,
      content: "done",
      blocks: [],
      finishReason: "completed",
      context: { ownerId: "agent-a", ownerType: "agent", topicId: "topic-2" },
    }));
    await wrapper.vm.$nextTick();
    expect(streamStore.isOwnerGenerating("agent-a", "agent")).toBe(false);

    // 2. 群聊轮次连续性：归属于 group-1，不污染成员 agent-a / agent-b 的私聊条目
    await streamStore.processStreamEvent(streamEvent({
      type: "group_turn",
      consumerRevision: 10,
      context: { ownerId: "group-1", ownerType: "group", topicId: "g-topic", turnId: "turn-1" },
      chunk: { active: true, phase: "arbitrating", revision: 1 } as any,
    }));
    await wrapper.vm.$nextTick();
    expect(getSlots().map((s) => s.attributes("data-generating"))).toEqual([
      "true",  // group-1
      "false", // agent-a
      "false", // agent-b
    ]);

    await streamStore.processStreamEvent(streamEvent({
      type: "group_turn",
      consumerRevision: 11,
      context: { ownerId: "group-1", ownerType: "group", topicId: "g-topic", turnId: "turn-1" },
      chunk: { active: false, revision: 2 } as any,
    }));
    await wrapper.vm.$nextTick();
    expect(getSlots().map((s) => s.attributes("data-generating"))).toEqual([
      "false",
      "false",
      "false",
    ]);

    wrapper.unmount();
  });
});
