import { beforeEach, describe, expect, it, vi } from "vitest";
import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";

import AgentList from "@/features/agent/AgentList.vue";
import { useAssistantStore } from "@/core/stores/assistant";

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
});
