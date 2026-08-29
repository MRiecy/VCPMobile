import { beforeEach, describe, expect, it } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useChatHistoryStore } from "@/core/stores/chatHistoryStore";
import { useChatSessionStore } from "@/core/stores/chatSessionStore";
import { useSettingsStore } from "@/core/stores/settings";
import { useAssistantStore } from "@/core/stores/assistant";
import { useNotificationStore } from "@/core/stores/notification";
import { clearInvokeMocks, mockInvoke } from "@/tests/mocks/tauri";

/**
 * invite_only 群组「提及即邀约」链路的 store 级回归测试：
 * 发送消息 → handle_group_chat_message 返回 no_ai_response/invite_only
 * → 解析 @提及 → 串行调用 invite_group_member_to_speak。
 *
 * 血训：中文输入法全角 ＠ 曾让解析恒为空，用户只看到引导 toast。
 */

const setupInviteOnlyGroup = async () => {
  const assistant = useAssistantStore();
  assistant.agents = [
    { id: "a1", name: "Nova", model: "m" },
    { id: "a2", name: "Luna", model: "m" },
  ] as any;
  assistant.groups = [
    { id: "g1", name: "邀约群", members: ["a1", "a2"], mode: "invite_only" },
  ] as any;

  const settings = useSettingsStore();
  settings.settings = {
    vcpServerUrl: "http://vcp.test",
    vcpApiKey: "key",
    userName: "User",
  } as any;

  const session = useChatSessionStore();
  session.setConversation({ id: "g1", type: "group" }, "t1");

  const history = useChatHistoryStore();
  await history.loadHistoryPaginated("g1", "group", "t1");
  return history;
};

describe("invite_only 提及即邀约", () => {
  const invitedAgentIds: string[] = [];

  beforeEach(() => {
    setActivePinia(createPinia());
    clearInvokeMocks();
    invitedAgentIds.length = 0;

    mockInvoke("get_active_generations", () => []);
    mockInvoke("load_chat_history", () => []);
    mockInvoke("append_single_message", () => ({ blocks: [], topicUpdatedAt: 1 }));
    mockInvoke("handle_group_chat_message", () => ({
      status: "no_ai_response",
      reason: "invite_only",
    }));
    mockInvoke("invite_group_member_to_speak", (args) => {
      invitedAgentIds.push((args?.payload as { agentId: string }).agentId);
      return { status: "ok" };
    });
  });

  it("auto-invites mentioned members in order of first appearance", async () => {
    const history = await setupInviteOnlyGroup();
    await history.sendMessage("@Luna 和 @Nova 你们好");
    expect(invitedAgentIds).toEqual(["a2", "a1"]);
  });

  it("auto-invites on the full-width ＠ from Chinese IMEs", async () => {
    const history = await setupInviteOnlyGroup();
    await history.sendMessage("＠Nova 在吗");
    expect(invitedAgentIds).toEqual(["a1"]);
  });

  it("shows guidance toast instead of inviting when nobody is mentioned", async () => {
    const history = await setupInviteOnlyGroup();
    await history.sendMessage("大家好");
    expect(invitedAgentIds).toEqual([]);

    const notifications = useNotificationStore();
    expect(
      notifications.activeToasts.some((n) => n.title === "群组未产生回复"),
    ).toBe(true);
  });
});
