// @vitest-environment happy-dom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { useModalHistory } from "../../core/composables/useModalHistory";

describe("useModalHistory modal replacement", () => {
  const historyApi = useModalHistory();

  beforeEach(() => {
    while (historyApi.closeTopModal()) {
      // 清空模块级弹窗栈，确保测试之间互不影响。
    }
    window.history.replaceState({ vcpRoot: true, vcpMain: true }, "");
  });

  it("将 ContextMenu 原子替换为 Confirm，不新增历史项或额外回退", () => {
    const pushState = vi.spyOn(window.history, "pushState");
    const replaceState = vi.spyOn(window.history, "replaceState");
    const back = vi
      .spyOn(window.history, "back")
      .mockImplementation(() => undefined);
    const closeConfirm = vi.fn();

    historyApi.registerModal("ContextMenu", vi.fn());
    expect(
      historyApi.replaceModal("ContextMenu", "Confirm", closeConfirm),
    ).toBe(true);

    expect(pushState).toHaveBeenCalledTimes(1);
    expect(replaceState).toHaveBeenCalledTimes(1);
    expect(window.history.state.vcpModalId).toBe("Confirm");
    expect(historyApi.modalStackLength()).toBe(1);

    historyApi.unregisterModal("ContextMenu");
    expect(back).not.toHaveBeenCalled();
    expect(historyApi.modalStackLength()).toBe(1);

    const historySynced = vi.fn();
    historyApi.unregisterModal("Confirm", historySynced);
    expect(back).toHaveBeenCalledTimes(1);
    expect(historyApi.modalStackLength()).toBe(0);
    expect(historySynced).not.toHaveBeenCalled();

    window.dispatchEvent(
      new PopStateEvent("popstate", {
        state: { vcpRoot: true, vcpMain: true },
      }),
    );
    expect(historySynced).toHaveBeenCalledTimes(1);

    pushState.mockRestore();
    replaceState.mockRestore();
    back.mockRestore();
  });

  it("普通 Prompt 仍保持 pushState 与单次 back 行为", () => {
    const pushState = vi.spyOn(window.history, "pushState");
    const back = vi
      .spyOn(window.history, "back")
      .mockImplementation(() => undefined);

    historyApi.registerModal("Prompt", vi.fn());
    expect(pushState).toHaveBeenCalledTimes(1);
    expect(historyApi.modalStackLength()).toBe(1);

    historyApi.unregisterModal("Prompt");
    expect(back).toHaveBeenCalledTimes(1);
    expect(historyApi.modalStackLength()).toBe(0);
    window.dispatchEvent(
      new PopStateEvent("popstate", {
        state: { vcpRoot: true, vcpMain: true },
      }),
    );

    pushState.mockRestore();
    back.mockRestore();
  });
});
