// @vitest-environment happy-dom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";

const modalHistory = vi.hoisted(() => ({
  registerModal: vi.fn(),
  unregisterModal: vi.fn(),
  replaceModal: vi.fn(),
}));

vi.mock("../../core/composables/useModalHistory", () => ({
  useModalHistory: () => ({
    ...modalHistory,
    modalStackLength: () => 0,
    initRootHistory: vi.fn(),
    closeTopModal: vi.fn(),
  }),
}));

import { useOverlayStore } from "../../core/stores/overlay";

describe("overlay confirm store", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
    modalHistory.replaceModal.mockReturnValue(false);
    modalHistory.unregisterModal.mockImplementation((_id, onHistorySynced) => {
      onHistorySynced?.();
    });
  });

  it("确认 Promise 只结算一次，并只注销一次历史项", async () => {
    const store = useOverlayStore();
    const result = store.showConfirm({
      title: "删除消息",
      message: "确定删除吗？",
      isDanger: true,
    });

    expect(store.confirmConfig?.isDanger).toBe(true);
    expect(modalHistory.registerModal).toHaveBeenCalledWith(
      "Confirm",
      expect.any(Function),
    );

    store.resolveConfirm(true);
    store.resolveConfirm(false);

    await expect(result).resolves.toBe(true);
    expect(modalHistory.unregisterModal).toHaveBeenCalledTimes(1);
    expect(modalHistory.unregisterModal).toHaveBeenCalledWith(
      "Confirm",
      expect.any(Function),
    );
    expect(store.confirmConfig).toBeNull();
  });

  it("返回键按取消结算，onlyConfirm 则按确认结算", async () => {
    const store = useOverlayStore();
    let historyClose: (() => void) | undefined;
    modalHistory.registerModal.mockImplementation((_id, closeHandler) => {
      historyClose = closeHandler;
    });

    const cancellable = store.showConfirm({
      title: "普通确认",
      message: "继续吗？",
    });
    historyClose?.();
    historyClose?.();
    await expect(cancellable).resolves.toBe(false);
    expect(modalHistory.unregisterModal).not.toHaveBeenCalled();

    const acknowledgement = store.showConfirm({
      title: "操作完成",
      message: "请刷新。",
      onlyConfirm: true,
    });
    historyClose?.();
    await expect(acknowledgement).resolves.toBe(true);
  });

  it("等待浏览器历史同步后再结算并展示下一请求", async () => {
    const store = useOverlayStore();
    let finishHistorySync: (() => void) | undefined;
    modalHistory.unregisterModal.mockImplementation((_id, onHistorySynced) => {
      finishHistorySync = onHistorySynced;
    });

    const first = store.showConfirm({ title: "第一项", message: "第一项内容" });
    const second = store.showConfirm({
      title: "第二项",
      message: "第二项内容",
    });
    let firstResolved = false;
    void first.then(() => {
      firstResolved = true;
    });

    expect(store.confirmConfig?.title).toBe("第一项");
    store.resolveConfirm(true);
    await Promise.resolve();
    expect(firstResolved).toBe(false);
    expect(store.confirmConfig).toBeNull();

    finishHistorySync?.();
    await expect(first).resolves.toBe(true);
    await Promise.resolve();

    expect(store.confirmConfig?.title).toBe("第二项");
    store.resolveConfirm(false);
    finishHistorySync?.();
    await expect(second).resolves.toBe(false);
  });

  it("从 ContextMenu 打开确认框时原子替换历史项", async () => {
    const store = useOverlayStore();
    store.openContextMenu([], "操作");
    modalHistory.replaceModal.mockReturnValue(true);

    const result = store.showConfirm({
      title: "删除消息",
      message: "确定删除吗？",
    });

    expect(modalHistory.replaceModal).toHaveBeenCalledWith(
      "ContextMenu",
      "Confirm",
      expect.any(Function),
    );
    expect(store.contextMenuConfig).toBeNull();
    expect(modalHistory.registerModal).toHaveBeenCalledTimes(1);

    store.resolveConfirm(false);
    await expect(result).resolves.toBe(false);
  });

  it("普通 Prompt 和页面栈仍使用原有注册流程", () => {
    const store = useOverlayStore();
    store.openPrompt({
      title: "重命名",
      initialValue: "旧名称",
      placeholder: "新名称",
      onConfirm: vi.fn(),
    });
    store.pushPage("settings");

    expect(modalHistory.registerModal).toHaveBeenCalledWith(
      "Prompt",
      expect.any(Function),
    );
    expect(modalHistory.registerModal).toHaveBeenCalledWith(
      "Page:settings:",
      expect.any(Function),
    );
    expect(modalHistory.replaceModal).not.toHaveBeenCalled();
  });
});
