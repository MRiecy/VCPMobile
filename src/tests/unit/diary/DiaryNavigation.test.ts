import { beforeEach, describe, expect, it } from "vitest";
import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import DiaryCenterView from "@/features/diary/DiaryCenterView.vue";
import { useDiaryStore } from "@/features/diary/diaryStore";
import { useOverlayStore } from "@/core/stores/overlay";
import { mockInvoke } from "@/tests/mocks/tauri";

describe("DiaryCenter internal navigation gate", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mockInvoke("diary_list_folders", () => ({ folders: [] }));
  });

  function mountCenter() {
    return mount(DiaryCenterView, {
      props: { isOpen: true, zIndex: 40, openTarget: null },
    });
  }

  function seedEditor(dirty: boolean) {
    const store = useDiaryStore();
    store.document = {
      key: { folder: "F", file: "a.txt" },
      content: "baseline",
      contentHash: "hash",
    };
    store.startEditing();
    if (dirty) store.setDraft("changed");
    return store;
  }

  it("queues a discard confirmation for a dirty editor", async () => {
    const store = seedEditor(true);
    const overlay = useOverlayStore();
    const wrapper = mountCenter();

    await wrapper.get('button[aria-label="退出编辑"]').trigger("click");
    await Promise.resolve();

    expect(store.screen).toBe("editor");
    expect(store.draft).toBe("changed");
    expect(overlay.confirmConfig?.title).toBe("放弃未保存修改？");
    overlay.confirmConfig?.onCancel();
    wrapper.unmount();
  });

  it("refuses back while a save is being verified", async () => {
    const store = seedEditor(true);
    store.saveState = "saving";
    const overlay = useOverlayStore();
    const wrapper = mountCenter();

    await wrapper.get('button[aria-label="退出编辑"]').trigger("click");
    await Promise.resolve();

    expect(store.screen).toBe("editor");
    expect(overlay.confirmConfig).toBeNull();
    wrapper.unmount();
  });

  it("reduces a clean editor to its reader without closing the global page", async () => {
    const store = seedEditor(false);
    const wrapper = mountCenter();

    await wrapper.get('button[aria-label="退出编辑"]').trigger("click");

    expect(store.screen).toBe("reader");
    expect(wrapper.emitted("close")).toBeUndefined();
    wrapper.unmount();
  });

  it("keeps the memo list visible until an expanded search has a query", async () => {
    const store = useDiaryStore();
    const wrapper = mountCenter();

    expect(wrapper.find('[data-diary-role="search-panel"]').exists()).toBe(false);
    await wrapper.get('button[aria-label="搜索日记"]').trigger("click");

    expect(wrapper.find('[data-diary-role="search-panel"]').exists()).toBe(true);
    expect(store.searchMode).toBe("none");

    await wrapper.get('button[aria-label="关闭搜索"]').trigger("click");
    expect(wrapper.find('button[aria-label="搜索日记"]').exists()).toBe(true);
    expect(store.searchMode).toBe("none");
    wrapper.unmount();
  });
});
