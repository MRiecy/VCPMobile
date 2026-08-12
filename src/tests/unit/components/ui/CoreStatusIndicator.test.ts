import { mount } from "@vue/test-utils";
import { createPinia, disposePinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import CoreStatusIndicator from "@/components/ui/CoreStatusIndicator.vue";
import { useNotificationStore } from "@/core/stores/notification";

let activePinia: ReturnType<typeof createPinia>;

beforeEach(() => {
  activePinia = createPinia();
  setActivePinia(activePinia);
});

afterEach(() => {
  disposePinia(activePinia);
});

describe("CoreStatusIndicator", () => {
  it("keeps the ready state static after the core becomes active", () => {
    const store = useNotificationStore();
    store.updateStatus({ status: "connected", message: "connected", source: "VCPLog" });
    store.updateCoreStatus({ status: "ready", message: "ready", source: "Core" });

    const wrapper = mount(CoreStatusIndicator, { global: { plugins: [activePinia] } });

    const dotClasses = wrapper.get('[data-testid="core-status-dot"]').classes();
    expect(wrapper.text()).toContain("Core Active");
    expect(dotClasses).toContain("bg-green-500");
    expect(dotClasses).not.toContain("animate-pulse");
    expect(dotClasses).not.toContain("animate-bounce");
    expect(dotClasses).not.toContain("vcp-core-pulse");
    wrapper.unmount();
  });

  it("keeps the connecting state distinguishable without forcing steady redraws", () => {
    const store = useNotificationStore();
    store.updateStatus({ status: "connected", message: "connected", source: "VCPLog" });
    store.updateCoreStatus({ status: "connecting", message: "connecting", source: "Core" });

    const wrapper = mount(CoreStatusIndicator, { global: { plugins: [activePinia] } });

    expect(wrapper.get('[data-testid="core-status-dot"]').classes()).not.toContain("animate-pulse");
    expect(wrapper.text()).toContain("Booting...");
    wrapper.unmount();
  });
});
