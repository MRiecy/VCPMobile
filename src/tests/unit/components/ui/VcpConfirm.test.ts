// @vitest-environment happy-dom

import { afterEach, describe, expect, it } from "vitest";
import { mount, type VueWrapper } from "@vue/test-utils";
import { nextTick } from "vue";
import VcpConfirm from "../../../../components/ui/VcpConfirm.vue";

const wrappers: VueWrapper[] = [];

const mountConfirm = (props: InstanceType<typeof VcpConfirm>["$props"]) => {
  const wrapper = mount(VcpConfirm, {
    props,
    attachTo: document.body,
  });
  wrappers.push(wrapper);
  return wrapper;
};

afterEach(() => {
  wrappers.splice(0).forEach((wrapper) => wrapper.unmount());
  document.body.innerHTML = "";
});

describe("VcpConfirm", () => {
  it("渲染可访问的对话框并默认聚焦取消按钮", async () => {
    mountConfirm({
      isOpen: true,
      title: "删除助手",
      message: "此操作不可撤销。",
      isDanger: true,
    });

    await nextTick();
    const dialog = document.querySelector<HTMLElement>('[role="alertdialog"]');
    const buttons = document.querySelectorAll<HTMLButtonElement>("button");

    expect(dialog).not.toBeNull();
    expect(dialog?.getAttribute("aria-modal")).toBe("true");
    expect(
      document.getElementById(dialog?.getAttribute("aria-labelledby") || "")
        ?.textContent,
    ).toContain("删除助手");
    expect(
      document.getElementById(dialog?.getAttribute("aria-describedby") || "")
        ?.textContent,
    ).toContain("此操作不可撤销");
    expect(buttons).toHaveLength(2);
    expect(document.activeElement).toBe(buttons[0]);
  });

  it("同一次打开只发出一次确认结果", async () => {
    const wrapper = mountConfirm({
      isOpen: true,
      title: "确认操作",
      message: "继续吗？",
    });
    await nextTick();

    const confirmButton = Array.from(document.querySelectorAll("button")).find(
      (button) => button.textContent?.trim() === "确认",
    );
    confirmButton?.click();
    confirmButton?.click();

    expect(wrapper.emitted("confirm")).toHaveLength(1);
    expect(wrapper.emitted("cancel")).toBeUndefined();
  });

  it("支持 Escape、遮罩取消和焦点循环", async () => {
    const escapeWrapper = mountConfirm({
      isOpen: true,
      title: "确认操作",
      message: "继续吗？",
    });
    await nextTick();

    const dialog = document.querySelector<HTMLElement>('[role="dialog"]');
    const buttons = document.querySelectorAll<HTMLButtonElement>("button");
    buttons[0].focus();
    dialog?.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Tab",
        shiftKey: true,
        bubbles: true,
      }),
    );
    expect(document.activeElement).toBe(buttons[1]);

    dialog?.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Tab",
        bubbles: true,
      }),
    );
    expect(document.activeElement).toBe(buttons[0]);

    dialog?.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Escape",
        bubbles: true,
      }),
    );
    expect(escapeWrapper.emitted("cancel")).toHaveLength(1);

    escapeWrapper.unmount();
    wrappers.splice(wrappers.indexOf(escapeWrapper), 1);
    document.body.innerHTML = "";

    const backdropWrapper = mountConfirm({
      isOpen: true,
      title: "确认操作",
      message: "继续吗？",
    });
    await nextTick();
    document
      .querySelector<HTMLElement>(".vcp-confirm-modal")
      ?.parentElement?.click();
    expect(backdropWrapper.emitted("cancel")).toHaveLength(1);
  });

  it("onlyConfirm 模式隐藏取消按钮，并将所有关闭方式视为确认", async () => {
    const wrapper = mountConfirm({
      isOpen: true,
      title: "同步已完成",
      message: "需要立即刷新。",
      confirmText: "立即刷新",
      onlyConfirm: true,
    });
    await nextTick();

    expect(document.querySelectorAll("button")).toHaveLength(1);
    expect(document.activeElement).toBe(document.querySelector("button"));

    document
      .querySelector<HTMLElement>(".vcp-confirm-modal")
      ?.parentElement?.click();
    expect(wrapper.emitted("confirm")).toHaveLength(1);
    expect(wrapper.emitted("cancel")).toBeUndefined();
  });

  it("关闭后恢复触发元素焦点", async () => {
    const opener = document.createElement("button");
    document.body.appendChild(opener);
    opener.focus();

    const wrapper = mountConfirm({
      isOpen: true,
      title: "确认操作",
      message: "继续吗？",
    });
    await nextTick();
    expect(document.activeElement).not.toBe(opener);

    await wrapper.setProps({ isOpen: false });
    await nextTick();
    expect(document.activeElement).toBe(opener);
  });
});
