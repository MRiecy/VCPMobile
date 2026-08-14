export const VCP_CLI_MANIFEST_COMMAND = "get_vcp_mobile_cli_manifest";
export const VCP_CLI_TOOL_NAME = "VCPMobileCLI";

export interface VcpCliInvocationCommand {
  commandIdentifier: string;
  description: string;
  example: string;
}

export interface VcpCliManifest {
  manifestVersion: string;
  name: string;
  version: string;
  displayName: string;
  description: string;
  author: string;
  pluginType: string;
  entryPoint: {
    type: string;
    command: string;
  };
  communication: {
    protocol: string;
    timeout: number;
  };
  capabilities: {
    invocationCommands: VcpCliInvocationCommand[];
  };
}

export interface VcpCliManifestDocument {
  /** Exact bytes returned by the backend canonical serializer. */
  rawJson: string;
  byteLength: number;
  manifest: VcpCliManifest;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function requireNonEmptyString(
  source: Record<string, unknown>,
  key: string,
  scope: string,
): string {
  const value = source[key];
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`${scope}.${key} 不是有效字符串`);
  }
  return value;
}

/**
 * The command must return canonical JSON text, not a deserialized object.
 * Keeping the backend text intact is what makes copy/export byte-identical to
 * the manifest used by the Distributed adapter.
 */
export function parseCanonicalVcpCliManifest(
  payload: unknown,
): VcpCliManifestDocument {
  if (typeof payload !== "string") {
    throw new Error(
      "后端未返回规范 manifest 文本，无法保证复制与注册内容逐字一致",
    );
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(payload);
  } catch {
    throw new Error("规范 manifest 不是有效 JSON");
  }

  if (!isRecord(parsed)) {
    throw new Error("规范 manifest 顶层必须是 JSON 对象");
  }

  const manifestVersion = requireNonEmptyString(
    parsed,
    "manifestVersion",
    "manifest",
  );
  const name = requireNonEmptyString(parsed, "name", "manifest");
  const version = requireNonEmptyString(parsed, "version", "manifest");
  const displayName = requireNonEmptyString(parsed, "displayName", "manifest");
  const description = requireNonEmptyString(parsed, "description", "manifest");
  const author = requireNonEmptyString(parsed, "author", "manifest");
  const pluginType = requireNonEmptyString(parsed, "pluginType", "manifest");

  const entryPointValue = parsed.entryPoint;
  if (!isRecord(entryPointValue)) {
    throw new Error("manifest.entryPoint 不是有效对象");
  }
  const entryPoint = {
    type: requireNonEmptyString(entryPointValue, "type", "manifest.entryPoint"),
    command: requireNonEmptyString(
      entryPointValue,
      "command",
      "manifest.entryPoint",
    ),
  };

  const communicationValue = parsed.communication;
  if (!isRecord(communicationValue)) {
    throw new Error("manifest.communication 不是有效对象");
  }
  const communicationTimeout = communicationValue.timeout;
  if (
    typeof communicationTimeout !== "number" ||
    !Number.isFinite(communicationTimeout)
  ) {
    throw new Error("manifest.communication.timeout 不是有效数字");
  }
  const communication = {
    protocol: requireNonEmptyString(
      communicationValue,
      "protocol",
      "manifest.communication",
    ),
    timeout: communicationTimeout,
  };

  const capabilitiesValue = parsed.capabilities;
  if (!isRecord(capabilitiesValue)) {
    throw new Error("manifest.capabilities 不是有效对象");
  }
  const invocationCommandsValue = capabilitiesValue.invocationCommands;
  if (
    !Array.isArray(invocationCommandsValue) ||
    invocationCommandsValue.length !== 1
  ) {
    throw new Error("manifest 必须包含且仅包含一个 invocation command");
  }
  const invocationCommandValue = invocationCommandsValue[0];
  if (!isRecord(invocationCommandValue)) {
    throw new Error("manifest invocation command 不是有效对象");
  }
  const invocationCommand: VcpCliInvocationCommand = {
    commandIdentifier: requireNonEmptyString(
      invocationCommandValue,
      "commandIdentifier",
      "manifest.capabilities.invocationCommands[0]",
    ),
    description: requireNonEmptyString(
      invocationCommandValue,
      "description",
      "manifest.capabilities.invocationCommands[0]",
    ),
    example: requireNonEmptyString(
      invocationCommandValue,
      "example",
      "manifest.capabilities.invocationCommands[0]",
    ),
  };

  if (
    name !== VCP_CLI_TOOL_NAME ||
    invocationCommand.commandIdentifier !== name
  ) {
    throw new Error("manifest.name 与 commandIdentifier 必须同为 VCPMobileCLI");
  }

  return {
    rawJson: payload,
    byteLength: new TextEncoder().encode(payload).byteLength,
    manifest: {
      manifestVersion,
      name,
      version,
      displayName,
      description,
      author,
      pluginType,
      entryPoint,
      communication,
      capabilities: {
        invocationCommands: [invocationCommand],
      },
    },
  };
}

export function manifestExportFileName(version: string): string {
  const safeVersion = version.replace(/[^0-9A-Za-z._-]/g, "_");
  return `${VCP_CLI_TOOL_NAME}-${safeVersion}.manifest.json`;
}
