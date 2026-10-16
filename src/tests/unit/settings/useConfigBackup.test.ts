import { describe, expect, it } from "vitest";
import {
  BACKUP_SETTINGS_KEYS,
  CONFIG_BACKUP_EXTENSION,
  CONFIG_BACKUP_KIND,
  CONFIG_BACKUP_VERSION,
  buildBackupFileName,
  buildBackupPayload,
  bytesToBase64,
  pickBackupSettings,
} from "../../../features/settings/useConfigBackup";
import type { AppSettings } from "../../../core/stores/settings";

function makeSettings(): AppSettings {
  return {
    userName: "测试用户",
    vcpServerUrl: "https://vcp.example.com",
    chatEndpointMode: "vcpTools",
    vcpApiKey: "api-key-1",
    vcpLogUrl: "ws://192.168.1.10:6005",
    vcpLogKey: "log-key-1",
    syncServerUrl: "ws://192.168.1.10:5975",
    syncHttpUrl: "http://192.168.1.10:5974",
    syncToken: "sync-token-1",
    adminUsername: "admin",
    adminPassword: "secret",
    fileKey: "file-key-1",
    distributedEnabled: true,
    distributedWsUrl: "ws://192.168.1.10:6005",
    distributedVcpKey: "dist-key",
    distributedDeviceName: "Pixel",
    topicSummaryModel: "gemini-3.1-flash-lite",
    syncLogLevel: "INFO",
    agentOrder: ["agent-1"],
    groupOrder: [],
    currentThemeMode: null,
    syncPrerenderEnabled: false,
    enableAssistant: false,
    assistantAgentId: "",
  };
}

describe("useConfigBackup 白名单提取", () => {
  it("白名单恰好覆盖身份+连接+分布式 16 个字段", () => {
    expect(BACKUP_SETTINGS_KEYS).toHaveLength(16);
    expect(BACKUP_SETTINGS_KEYS).toContain("userName");
    expect(BACKUP_SETTINGS_KEYS).toContain("fileKey");
    expect(BACKUP_SETTINGS_KEYS).toContain("distributedDeviceName");
  });

  it("只提取白名单字段，排除本地偏好与排序数据", () => {
    const picked = pickBackupSettings(makeSettings());
    expect(picked.userName).toBe("测试用户");
    expect(picked.chatEndpointMode).toBe("vcpTools");
    expect(picked.vcpApiKey).toBe("api-key-1");
    expect(picked.distributedEnabled).toBe(true);
    expect("agentOrder" in picked).toBe(false);
    expect("groupOrder" in picked).toBe(false);
    expect("topicSummaryModel" in picked).toBe(false);
    expect("syncLogLevel" in picked).toBe(false);
    expect("currentThemeMode" in picked).toBe(false);
  });

  it("distributedEnabled 非布尔时归一为 false，缺失字符串字段归一为空串", () => {
    const malformed = {
      ...makeSettings(),
      distributedEnabled: "yes",
      adminPassword: undefined,
    } as unknown as AppSettings;
    const picked = pickBackupSettings(malformed);
    expect(picked.distributedEnabled).toBe(false);
    expect(picked.adminPassword).toBe("");
  });
});

describe("useConfigBackup 导出 payload", () => {
  it("文件名带时间戳与 .vcpcfg 专用后缀", () => {
    const name = buildBackupFileName(new Date(2026, 7, 18, 9, 5));
    expect(name).toBe(`vcp-mobile-config-20260818-0905${CONFIG_BACKUP_EXTENSION}`);
  });

  it("payload 结构包含种类/版本/时间戳/设置，不含头像", () => {
    const payload = buildBackupPayload(makeSettings(), new Date(2026, 7, 18));
    expect(payload.app).toBe("vcp-mobile");
    expect(payload.kind).toBe(CONFIG_BACKUP_KIND);
    expect(payload.version).toBe(CONFIG_BACKUP_VERSION);
    expect(payload.exportedAt).toBe(new Date(2026, 7, 18).toISOString());
    expect(payload.settings.syncToken).toBe("sync-token-1");
    expect("avatar" in payload).toBe(false);
  });
});


describe("useConfigBackup Base64 工具", () => {
  it("bytesToBase64 对 UTF-8 中文编码可逆", () => {
    const original = new TextEncoder().encode(JSON.stringify({ userName: "测试用户🔥" }));
    const binary = atob(bytesToBase64(original));
    const roundtripped = Uint8Array.from(binary, (c) => c.charCodeAt(0));
    expect(Array.from(roundtripped)).toEqual(Array.from(original));
  });
});
