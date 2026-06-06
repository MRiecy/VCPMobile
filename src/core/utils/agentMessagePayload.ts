export interface AgentMessagePayload {
  type?: string;
  title?: string;
  message?: unknown;
  originalContent?: unknown;
  recipient?: string;
  androidNotification?: {
    delivered?: boolean;
    [key: string]: unknown;
  };
  [key: string]: unknown;
}

function asObject(value: unknown): Record<string, unknown> | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  return value as Record<string, unknown>;
}

export function findAgentMessagePayload(
  value: unknown,
  depth = 0,
): AgentMessagePayload | null {
  if (!value || depth > 6) return null;
  if (typeof value === "string") {
    try {
      const parsed = JSON.parse(value);
      return (
        findAgentMessagePayload(parsed, depth + 1) ||
        findAgentMessageToolPayload(parsed, depth + 1)
      );
    } catch {
      return null;
    }
  }

  const objectValue = asObject(value);
  if (!objectValue) return null;
  if (objectValue.type === "agent_message") {
    return objectValue as AgentMessagePayload;
  }

  if (isAgentMessageToolObject(objectValue)) {
    const built = buildAgentMessagePayloadFromToolObject(objectValue, depth);
    if (built) return built;
  }

  for (const key of [
    "data",
    "callbackData",
    "result",
    "payload",
    "message",
    "content",
    "original_plugin_output",
    "raw",
    "details",
  ]) {
    const found = findAgentMessagePayload(objectValue[key], depth + 1);
    if (found) return found;
  }

  return null;
}

function findAgentMessageToolPayload(
  value: unknown,
  depth = 0,
): AgentMessagePayload | null {
  if (!value || depth > 6) return null;
  if (typeof value === "string") {
    try {
      const parsed = JSON.parse(value);
      return (
        findAgentMessagePayload(parsed, depth + 1) ||
        findAgentMessageToolPayload(parsed, depth + 1)
      );
    } catch {
      return null;
    }
  }

  const objectValue = asObject(value);
  if (!objectValue) return null;

  if (isAgentMessageToolObject(objectValue)) {
    const built = buildAgentMessagePayloadFromToolObject(objectValue, depth);
    if (built) return built;
  }

  for (const key of [
    "data",
    "callbackData",
    "result",
    "payload",
    "message",
    "content",
    "original_plugin_output",
    "raw",
    "details",
  ]) {
    const found = findAgentMessageToolPayload(objectValue[key], depth + 1);
    if (found) return found;
  }

  return null;
}

function isAgentMessageToolObject(value: Record<string, unknown>): boolean {
  return ["tool_name", "toolName", "pluginName", "PLUGIN_NAME_FOR_CALLBACK", "name"]
    .some((key) => isAgentMessageToolName(value[key]));
}

function isAgentMessageToolName(value: unknown): boolean {
  if (typeof value !== "string") return false;
  const normalized = value.replace(/[^a-z0-9]/gi, "").toLowerCase();
  return (
    normalized === "agentmessage" ||
    normalized === "mobileagentmessage" ||
    normalized.startsWith("agentmessage")
  );
}

function buildAgentMessagePayloadFromToolObject(
  value: Record<string, unknown>,
  depth = 0,
): AgentMessagePayload | null {
  for (const key of [
    "result",
    "payload",
    "original_plugin_output",
    "content",
    "message",
    "body",
    "data",
  ]) {
    const found = findAgentMessagePayload(value[key], depth + 1);
    if (found) return found;
  }

  const body = firstToolBodyString(value, depth + 1);
  if (!body) return null;

  const recipient = firstString(value, [
    "recipient",
    "Maid",
    "maid",
    "MaidName",
    "sender_name",
  ]);

  return {
    type: "agent_message",
    title:
      firstString(value, ["title"]) ||
      (recipient ? `${recipient} 的消息` : "Agent 消息"),
    message: body,
    originalContent: body,
    recipient,
    androidNotification: asObject(value.androidNotification) as
      | AgentMessagePayload["androidNotification"]
      | undefined,
  };
}

function firstToolBodyString(value: unknown, depth = 0): string | null {
  if (!value || depth > 7) return null;

  if (typeof value === "string") {
    const trimmed = value.trim();
    if (!trimmed) return null;
    try {
      const parsed = JSON.parse(trimmed);
      const agentPayload = findAgentMessagePayload(parsed, depth + 1);
      if (agentPayload?.originalContent || agentPayload?.message) {
        return String(agentPayload.originalContent || agentPayload.message);
      }
      const nested = firstToolBodyString(parsed, depth + 1);
      if (nested) return nested;
    } catch {}
    return trimmed;
  }

  const objectValue = asObject(value);
  if (!objectValue) return null;

  for (const key of [
    "originalContent",
    "body",
    "message",
    "content",
    "result",
    "original_plugin_output",
  ]) {
    const text = firstToolBodyString(objectValue[key], depth + 1);
    if (text) return text;
  }

  return null;
}

function firstString(
  value: Record<string, unknown>,
  keys: string[],
): string | undefined {
  for (const key of keys) {
    const item = value[key];
    if (typeof item === "string" && item.trim()) return item.trim();
  }
  return undefined;
}
