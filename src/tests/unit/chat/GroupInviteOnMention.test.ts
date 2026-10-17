import { beforeEach, describe, expect, it } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useChatHistoryStore } from "@/core/stores/chatHistoryStore";
import { useChatSessionStore } from "@/core/stores/chatSessionStore";
import { useSettingsStore } from "@/core/stores/settings";
import { useAssistantStore } from "@/core/stores/assistant";
import { useNotificationStore } from "@/core/stores/notification";
import { clearInvokeMocks, invokeMock, mockInvoke } from "@/tests/mocks/tauri";
import { useChatStreamStore } from "@/core/stores/chatStreamStore";
import { flushPromises } from "@/tests/utils/flush";

/**
 * invite_only 群组「提及即邀约」链路的 store 级回归测试：
 * 发送消息 → handle_group_chat_message 返回 no_ai_response/invite_only
 * → 解析 @提及 → 一次 invite_group_member_to_speak 交给同一个 Rust 群轮。
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

    mockInvoke("reconnect_chat_streams", () => ({ generations: [], groupTurns: [], activeRequests: [] }));
    mockInvoke("load_chat_history", () => []);
    mockInvoke("append_single_message", () => ({ blocks: [], topicUpdatedAt: 1 }));
    mockInvoke("handle_group_chat_message", () => ({
      status: "no_ai_response",
      reason: "invite_only",
    }));
    mockInvoke("invite_group_member_to_speak", (args) => {
      invitedAgentIds.push(...(args?.payload as { agentIds: string[] }).agentIds);
      return { status: "ok" };
    });
  });

  it("auto-invites mentioned members in order of first appearance", async () => {
    const history = await setupInviteOnlyGroup();
    await history.sendMessage("@Luna 和 @Nova 你们好");
    expect(invitedAgentIds).toEqual(["a2", "a1"]);
    const invitations = invokeMock.mock.calls.filter(([command]) => command === "invite_group_member_to_speak");
    expect(invitations).toHaveLength(1);
    expect(invitations[0][1]?.payload).toEqual({ groupId: "g1", topicId: "t1", agentIds: ["a2", "a1"] });
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

  it.each(["cancelled", "stopping"])("does not start another invitation after the batch reports %s", async status => {
    let finish!: (value: { status: string }) => void;
    mockInvoke("invite_group_member_to_speak", () => new Promise(resolve => { finish = resolve; }));
    const history = await setupInviteOnlyGroup();
    const stream = useChatStreamStore();
    const sending = history.sendMessage("@Luna 和 @Nova 你们好");
    await flushPromises();
    expect(stream.isConversationPreparing("g1", "group", "t1")).toBe(true);
    await stream.stopGroupTurn("g1", "t1");
    finish({ status });
    await sending;
    expect(invokeMock.mock.calls.filter(([command]) => command === "invite_group_member_to_speak")).toHaveLength(1);
    expect(stream.isConversationPreparing("g1", "group", "t1")).toBe(false);
  });
});
