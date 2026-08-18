import { describe, expect, it } from "vitest";
import {
  BACKUP_SETTINGS_KEYS,
  CONFIG_BACKUP_EXTENSION,
  CONFIG_BACKUP_KIND,
  CONFIG_BACKUP_VERSION,
  buildBackupFileName,
  buildBackupPayload,
  bytesToBase64,
  parseBackupFile,
  pickBackupSettings,
} from "../../../features/settings/useConfigBackup";
import type { AppSettings } from "../../../core/stores/settings";

function makeSettings(overrides: Partial<AppSettings> = {}): AppSettings {
  return {
    userName: "测试用户",
    vcpServerUrl: "https://vcp.example.com",
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
    ...overrides,
  };
}

describe("useConfigBackup 白名单提取", () => {
  it("白名单恰好覆盖身份+连接+分布式 15 个字段", () => {
    expect(BACKUP_SETTINGS_KEYS).toHaveLength(15);
    expect(BACKUP_SETTINGS_KEYS).toContain("userName");
    expect(BACKUP_SETTINGS_KEYS).toContain("fileKey");
    expect(BACKUP_SETTINGS_KEYS).toContain("distributedDeviceName");
  });

  it("只提取白名单字段，排除本地偏好与排序数据", () => {
    const picked = pickBackupSettings(makeSettings());
    expect(picked.userName).toBe("测试用户");
    expect(picked.vcpApiKey).toBe("api-key-1");
    expect(picked.distributedEnabled).toBe(true);
    expect("agentOrder" in picked).toBe(false);
    expect("groupOrder" in picked).toBe(false);
    expect("topicSummaryModel" in picked).toBe(false);
    expect("syncLogLevel" in picked).toBe(false);
    expect("currentThemeMode" in picked).toBe(false);
  });

  it("distributedEnabled 非布尔时归一为 false，缺失字符串字段归一为空串", () => {
    const picked = pickBackupSettings(
      makeSettings({ distributedEnabled: "yes" as any, adminPassword: undefined }),
    );
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

describe("useConfigBackup 导入解析", () => {
  it("导出→导入 roundtrip 保持字段一致", () => {
    const payload = buildBackupPayload(makeSettings(), new Date(2026, 7, 18));
    const parsed = parseBackupFile(JSON.stringify(payload));
    expect(parsed.settings).toEqual(payload.settings);
    expect(parsed.exportedAt).toBe(payload.exportedAt);
  });

  it("拒绝非 JSON / 错误种类 / 错误版本 / 缺失 settings", () => {
    expect(() => parseBackupFile("{broken")).toThrow("JSON");
    expect(() => parseBackupFile(JSON.stringify({ app: "other", kind: CONFIG_BACKUP_KIND }))).toThrow(
      "不是 VCP Mobile",
    );
    expect(() =>
      parseBackupFile(
        JSON.stringify({ app: "vcp-mobile", kind: CONFIG_BACKUP_KIND, version: 99, settings: {} }),
      ),
    ).toThrow("版本");
    expect(() =>
      parseBackupFile(
        JSON.stringify({ app: "vcp-mobile", kind: CONFIG_BACKUP_KIND, version: CONFIG_BACKUP_VERSION }),
      ),
    ).toThrow("缺少配置内容");
  });

  it("未知字段一律丢弃，不进入导入 patch", () => {
    const payload = buildBackupPayload(makeSettings());
    const hacked = {
      ...payload,
      settings: {
        ...payload.settings,
        agentOrder: ["evil"],
        enableVcpToolInjection: true,
        somethingNew: "x",
      },
    };
    const parsed = parseBackupFile(JSON.stringify(hacked));
    expect("agentOrder" in parsed.settings).toBe(false);
    expect("enableVcpToolInjection" in parsed.settings).toBe(false);
    expect("somethingNew" in parsed.settings).toBe(false);
    expect(parsed.settings.vcpApiKey).toBe("api-key-1");
  });

  it("白名单字段类型错误时拒绝导入", () => {
    const payload = buildBackupPayload(makeSettings());
    const badString = { ...payload, settings: { ...payload.settings, syncToken: 123 } };
    expect(() => parseBackupFile(JSON.stringify(badString))).toThrow("syncToken");

    const badBool = { ...payload, settings: { ...payload.settings, distributedEnabled: "true" } };
    expect(() => parseBackupFile(JSON.stringify(badBool))).toThrow("distributedEnabled");
  });

  it("旧版含头像的备份文件可导入，头像字段被忽略", () => {
    const payload = buildBackupPayload(makeSettings());
    const legacy = { ...payload, avatar: { mimeType: "image/png", dataBase64: "QUJD" } };
    const parsed = parseBackupFile(JSON.stringify(legacy));
    expect(parsed.settings.vcpApiKey).toBe("api-key-1");
  });

  it("超过大小上限的文本直接拒绝", () => {
    const huge = " ".repeat(1024 * 1024 + 1);
    expect(() => parseBackupFile(huge)).toThrow("过大");
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
