import { defineStore } from "pinia";
import { ref } from "vue";
import type { ChatMessage } from "../types/chat";

interface Toast {
  id: string;
  title: string;
  message: string;
  type: "info" | "success" | "warning" | "error";
  timestamp: number;
}

interface AppSettings {
  userName?: string;
  vcpServerUrl?: string;
  vcpApiKey?: string;
  assistantAgentId?: string;
  [key: string]: any;
}

export const useFloatingAssistantStore = defineStore("floatingAssistant", () => {
  const messages = ref<ChatMessage[]>([]);
  const inputText = ref("");
  const isGenerating = ref(false);
  const currentStreamingMessageId = ref<string | null>(null);

  // Internal state replaces external stores
  const internalSettings = ref<AppSettings | null>(null);
  const toasts = ref<Toast[]>([]);

  // --- WebSocket IPC Logic ---
  const isFloatingMode = ref(
    window.location.pathname.includes("floating") ||
    window.location.search.includes("mode=floating"),
  );
  const ws = ref<WebSocket | null>(null);
  const wsReady = ref(false);
  const wsConfigured = ref(false); // true when initial_config received and settings loaded

  const addToast = (
    type: Toast["type"],
    title: string,
    message: string,
  ) => {
    const toast: Toast = {
      id: Math.random().toString(36).substring(2, 9),
      type,
      title,
      message,
      timestamp: Date.now(),
    };
    toasts.value.push(toast);
    if (toasts.value.length > 5) toasts.value.shift();
    setTimeout(() => {
      toasts.value = toasts.value.filter((t) => t.id !== toast.id);
    }, 3000);
  };

  const initWebSocket = () => {
    if (!isFloatingMode.value || ws.value) return;

    console.log("[FloatingAssistantStore] Initializing WebSocket IPC...");
    const socket = new WebSocket(`ws://127.0.0.1:14202/ws`);

    socket.onopen = () => {
      console.log("[FloatingAssistantStore] WebSocket connected.");
      wsReady.value = true;
      socket.send(JSON.stringify({ action: "get_initial_config" }));
    };

    socket.onmessage = (event) => {
      const data = JSON.parse(event.data);
      handleWsMessage(data);
    };

    socket.onerror = (err) => {
      console.error("[FloatingAssistantStore] WebSocket error:", err);
    };

    socket.onclose = () => {
      console.warn("[FloatingAssistantStore] WebSocket closed.");
      wsReady.value = false;
      ws.value = null;
    };

    ws.value = socket;
  };

  const handleWsMessage = (data: any) => {
    if (data.type === "initial_config") {
      if (data.settings && typeof data.settings === "object") {
        internalSettings.value = data.settings;
        wsConfigured.value = true;
        console.log(
          "[FloatingAssistantStore] Config loaded:",
          { agentId: data.settings.assistantAgentId, vcpUrl: data.settings.vcpServerUrl },
        );
      } else {
        console.warn(
          "[FloatingAssistantStore] Invalid settings in initial_config:",
          data.settings,
        );
      }
      return;
    }

    if (data.type === "archive_success") {
      addToast("success", "归档成功", "本次会话已存入主话题列表");
      clearSession();
      return;
    }

    const messageId = data.messageId || data.message_id;

    if (data.type === "thinking") {
      // 仅匹配 AI 消息占位 (以 _assistant_temp 结尾)，避免误匹配 user 消息
      const target = messages.value.find((m) => m.id.endsWith("_assistant_temp"));
      if (target && messageId) {
        target.id = messageId;
        currentStreamingMessageId.value = messageId;
      }
    } else if (data.type === "data") {
      let target = messages.value.find(
        (m) => m.id === currentStreamingMessageId.value,
      );
      // Fallback: 通过 isThinking 标记找到正在流式生成的 AI 消息
      if (!target) {
        target = messages.value.find((m) => m.isThinking);
      }
      if (target) {
        target.isThinking = false;
        let textChunk = "";
        if (typeof data.chunk === "string") {
          textChunk = data.chunk;
        } else if (
          data.chunk?.choices &&
          data.chunk.choices.length > 0
        ) {
          const delta = data.chunk.choices[0].delta;
          if (delta?.content) textChunk = delta.content;
        }
        if (textChunk) {
          target.content = (target.content || "") + textChunk;
        }
      }
    } else if (data.type === "end") {
      isGenerating.value = false;
      let target = messages.value.find(
        (m) => m.id === currentStreamingMessageId.value,
      );
      if (!target) {
        target = messages.value.find((m) => m.isThinking);
      }
      if (target) target.isThinking = false;
      currentStreamingMessageId.value = null;
    } else if (data.type === "error") {
      isGenerating.value = false;
      let target = messages.value.find(
        (m) => m.id === currentStreamingMessageId.value,
      );
      if (!target) {
        target = messages.value.find((m) => m.isThinking);
      }
      if (target) {
        target.isThinking = false;
        target.content =
          (target.content || "") +
          `\n\n[错误]: ${data.error || "请求异常"}`;
      }
      currentStreamingMessageId.value = null;
    }
  };

  const clearSession = () => {
    messages.value = [];
    inputText.value = "";
    isGenerating.value = false;
    currentStreamingMessageId.value = null;
  };

  /** Resolve settings: floating mode uses internal state, main app fetches via Tauri */
  const resolveSettings = async (): Promise<AppSettings | null> => {
    if (isFloatingMode.value) return internalSettings.value;

    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const s = await invoke<AppSettings>("read_settings");
      internalSettings.value = s;
      return s;
    } catch {
      return internalSettings.value;
    }
  };

  const resolveAgentId = async (): Promise<string | null> => {
    const settings = await resolveSettings();
    return settings?.assistantAgentId || null;
  };

  const archiveSession = async (): Promise<string | null> => {
    const validMessages = messages.value
      .filter(
        (m) =>
          m.content && (m.role === "user" || m.role === "assistant"),
      )
      .map((m) => ({
        role: m.role,
        name: m.name || null,
        content: m.content || "",
        timestamp: m.timestamp,
      }));

    if (validMessages.length === 0) return null;

    const agentId = await resolveAgentId();
    if (!agentId) return null;

    if (isFloatingMode.value && ws.value) {
      ws.value.send(
        JSON.stringify({
          action: "archive_assistant_chat",
          payload: {
            ownerId: agentId,
            ownerType: "agent",
            tempMessages: validMessages,
          },
        }),
      );
      return "WS_PENDING";
    }

    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const newTopicId = await invoke<string>(
        "archive_assistant_chat",
        {
          ownerId: agentId,
          ownerType: "agent",
          tempMessages: validMessages,
        },
      );
      addToast("success", "归档成功", "本次划词会话已成功归档至主话题列表中");
      clearSession();
      return newTopicId;
    } catch (e: any) {
      console.error("[FloatingAssistantStore] Archive failed:", e);
      return null;
    }
  };

  const sendMessage = async (content: string) => {
    if (!content.trim() || isGenerating.value) return;

    const settings = await resolveSettings();
    console.log("[FloatingAssistantStore] sendMessage settings:", settings);

    if (!settings) {
      addToast("error", "配置未就绪", "请稍后重试，助手配置加载中...");
      return;
    }

    const agentId = settings.assistantAgentId || null;
    if (!agentId) {
      addToast("error", "未配置助手 Agent", "请在主应用设置中指定划词助手 Agent");
      return;
    }

    const vcpUrl = settings.vcpServerUrl || "";
    const vcpApiKey = settings.vcpApiKey || "";
    if (!vcpUrl || !vcpApiKey) {
      addToast("error", "VCP 连接未配置", "请在主应用设置中填写服务器地址和 API Key");
      return;
    }

    const now = Date.now();
    const userMsg: ChatMessage = {
      id: `msg_${now}_user_temp`,
      role: "user",
      name: settings?.userName || "User",
      content,
      timestamp: now,
    };
    messages.value.push(userMsg);

    const aiMsgId = `msg_${now + 1}_assistant_temp`;
    const aiMsg: ChatMessage = {
      id: aiMsgId,
      role: "assistant",
      content: "",
      timestamp: now + 1,
      isThinking: true,
    };
    messages.value.push(aiMsg);

    isGenerating.value = true;
    currentStreamingMessageId.value = aiMsgId;

    const tempMessages = messages.value
      .slice(0, -1)
      .filter((m) => m.content)
      .map((m) => ({
        role: m.role,
        name: m.name || null,
        content: m.content || "",
        timestamp: m.timestamp,
      }));

    const payload = { agentId, tempMessages, vcpUrl, vcpApiKey };

    if (isFloatingMode.value && ws.value) {
      console.log("[FloatingAssistantStore] Sending via WS:", { agentId, vcpUrl, msgCount: tempMessages.length });
      ws.value.send(
        JSON.stringify({
          action: "handle_assistant_chat_stream",
          payload,
        }),
      );
      return;
    }

    try {
      const { invoke, Channel } = await import("@tauri-apps/api/core");
      const channel = new Channel<any>();
      channel.onmessage = (event: any) => {
        handleWsMessage(event);
      };

      await invoke("handle_assistant_chat_stream", {
        payload,
        streamChannel: channel,
      });
    } catch (e: any) {
      isGenerating.value = false;
      currentStreamingMessageId.value = null;
    }
  };

  return {
    messages,
    inputText,
    isGenerating,
    isFloatingMode,
    wsReady,
    wsConfigured,
    toasts,
    initWebSocket,
    clearSession,
    archiveSession,
    sendMessage,
    resolveSettings,
  };
});
