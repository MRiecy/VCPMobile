# 02 - V2 完整配置 Schema 与类型契约

## 1. 结构设计准则

1. **信封标准（The Universal Envelope）**：继续沿用已在 Android / iOS 原生选择器与 Rust 校验中建立的标准顶层元数据（`app: "vcp-mobile"`，`kind: "vcp-mobile-settings-backup"`，`version: 2`）。
2. **所有权清晰分层（Ownership Co-location）**：
   - 数据库实体归 Rust，字段强校验与原子落盘由 Rust 裁决；
   - 客户端偏好归 Vue 各 Store，格式归一化与回退默认值由前端 owner 裁决。
3. **稀疏补丁与存在性投影（Presence & Patch Semantics）**：
   - 导出只序列化明确定义的配置项，不序列化空/无意义对象；
   - 导入时缺项（absent）保留当前系统已有值，**绝不强行用系统默认值覆盖已有自定义配置**；
   - 显式非法值（如数值溢出、枚举越界）安全回退至安全默认值，保留同级合法兄弟字段。

---

## 2. 完整 JSON 结构范例

```json
{
  "app": "vcp-mobile",
  "kind": "vcp-mobile-settings-backup",
  "version": 2,
  "exportedAt": "2026-10-31T08:30:00.000Z",

  "domains": {
    "identity": {
      "userName": "指挥官"
    },
    "server": {
      "url": "https://chat.vcp.internal:8443",
      "endpointMode": "standard",
      "apiKey": "sk-vcp-prod-secret-key"
    },
    "log": {
      "url": "https://log.vcp.internal:9000",
      "level": "INFO"
    },
    "sync": {
      "token": "sync-token-abc123xyz"
    },
    "voice": {
      "asr": {
        "stream": true,
        "inputEncoding": "mp3_64",
        "language": "zh"
      },
      "tts": {
        "mode": "builtin",
        "voice": "Mia",
        "stream": true
      },
      "vad": {
        "endSilenceMs": 600
      },
      "guidance": {
        "rules": "语音交互时尽量简短回复，避免输出冗长 Markdown 格式。"
      }
    },
    "jev": {
      "timeoutMs": 15000,
      "maxRetries": 2
    },
    "navigation": {
      "agentOrder": ["agent-uuid-001", "agent-uuid-002"],
      "groupOrder": ["____group_dev", "____group_life"]
    }
  },

  "preferences": {
    "theme": {
      "themeKey": "themes-star-abyss.ts",
      "mode": "dark"
    },
    "chat": {
      "presentationMode": "bubble",
      "smoothStreamingEnabled": true,
      "messageHapticsEnabled": true,
      "collapseToolsWhileStreaming": false
    },
    "topic": {
      "sortMode": "updated",
      "pinnedTopicKeys": [
        "[\"agent\",\"agent-uuid-001\",\"topic-important-01\"]",
        "[\"group\",\"____group_dev\",\"topic-standup-daily\"]"
      ]
    },
    "diary": {
      "hiddenFolders": ["cluster_trash"],
      "folderOrder": ["work", "private", "ideas"],
      "collapsedCategories": ["cluster"]
    },
    "client": {
      "autoCheckUpdate": true
    }
  },

  "agentPreferences": {
    "agent-uuid-001": {
      "mobileSystemPrompt": "你现在处于移动端会话，回复字数严格控制在 150 字以内，使用精简标点。",
      "useTemperature": false,
      "voice": {
        "enabled": true,
        "mode": "builtin",
        "voice": "苏打",
        "stream": true,
        "cloneSampleId": ""
      }
    },
    "agent-uuid-002": {
      "mobileSystemPrompt": "代码审查助手，在手机上展示 Diff 时折叠辅助注释。",
      "useTemperature": true,
      "voice": {
        "enabled": false,
        "mode": "builtin",
        "voice": "Mia",
        "stream": false,
        "cloneSampleId": ""
      }
    }
  },

  "groupPreferences": {
    "____group_dev": {
      "chatMode": "jev",
      "modeSettings": {
        "sequential": {
          "speakerOrder": ["agent-uuid-001", "agent-uuid-002"]
        },
        "jev": {
          "navigatorPrompt": "请根据任务分配选择最合适的工程师回答。",
          "speakerThreshold": 0.65,
          "continueThreshold": 0.75,
          "stopThreshold": 0.3,
          "maxSpeakersPerRound": 3,
          "maxAutonomousRounds": 5,
          "historyWindow": 30,
          "continueDebounceMs": 800,
          "fallbackPolicy": "top_speaker",
          "memberStyles": {
            "agent-uuid-001": "严肃客观的架构师风格",
            "agent-uuid-002": "专注测试边界的质检员风格"
          }
        }
      },
      "enableContextMessageWindow": true,
      "contextMessageWindowSize": 50
    }
  },

  "standaloneData": {
    "modelFavorites": [
      "gemini-2.5-flash",
      "claude-3-7-sonnet",
      "deepseek-reasoner"
    ],
    "tarvenRules": [
      {
        "id": "rule-clean-speech-01",
        "name": "移动端无噪音过滤",
        "ruleType": "system_suffix",
        "isEnabled": true,
        "content": "请不要在输出中重复系统元数据，直接给出最终结果。",
        "scope": "global",
        "wrap": true,
        "role": "user",
        "depth": 0,
        "position": "append",
        "sortOrder": 10
      }
    ]
  }
}
```

---

## 3. Rust 类型定义与契约

```rust
// src-tauri/src/vcp_modules/agent/agent_types.rs

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentLocalSettings {
    #[serde(default)]
    pub mobile_system_prompt: String,
    #[serde(default)]
    pub use_temperature: bool,
    #[serde(default)]
    pub voice: AgentVoiceConfig,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(transparent)]
pub struct AgentLocalSettingsPatch(pub serde_json::Map<String, serde_json::Value>);

impl AgentLocalSettingsPatch {
    pub fn parse(value: &serde_json::Value) -> Result<Self, String> {
        let mut patch = serde_json::Map::new();
        let Some(obj) = value.as_object() else {
            return Ok(Self(patch));
        };
        if let Some(prompt) = obj.get("mobileSystemPrompt").and_then(|v| v.as_str()) {
            patch.insert("mobileSystemPrompt".into(), serde_json::json!(prompt));
        }
        if let Some(use_temp) = obj.get("useTemperature").and_then(|v| v.as_bool()) {
            patch.insert("useTemperature".into(), serde_json::json!(use_temp));
        }
        if let Some(voice_val) = obj.get("voice") {
            let voice = AgentVoiceConfig::from_stored(voice_val);
            patch.insert("voice".into(), serde_json::to_value(voice).map_err(|e| e.to_string())?);
        }
        Ok(Self(patch))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
```

```rust
// src-tauri/src/vcp_modules/infra/settings_manager/backup.rs

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedConfigBackup {
    pub domains: Map<String, Value>,
    pub preferences: Map<String, Value>,
    pub agent_preferences: BTreeMap<String, AgentLocalSettingsPatch>,
    pub group_preferences: BTreeMap<String, GroupLocalSettingsPatch>,
    pub model_favorites: Vec<String>,
    pub tarven_rules: Vec<crate::vcp_modules::chat::context_injection::TarvenRule>,
    pub exported_at: String,
}
```

```rust
// src-tauri/src/vcp_modules/infra/settings_manager/types.rs
// 通过 ts_rs 导出契约，执行 pnpm config:generate 自动同步前端生成文件

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "config-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ConfigBackupImportResult {
    pub result: ConfigSaveResult,
    pub missing_groups: bool,
    pub missing_agents: bool,
}
```

---

## 4. TypeScript 前端契约接口

```typescript
// src/features/settings/useConfigBackup.ts

export const CONFIG_BACKUP_VERSION = 2;

export interface AgentLocalSettingsDto {
  mobileSystemPrompt?: string;
  useTemperature?: boolean;
  voice?: {
    enabled?: boolean;
    mode?: "builtin" | "clone";
    voice?: string;
    cloneSampleId?: string;
    stream?: boolean;
  };
}

export interface ClientPreferencesDto {
  theme?: ThemePreferences;
  chat?: ChatPreferences;
  topic?: {
    sortMode?: TopicSortMode;
    /** 元素为 JSON 序列化的三元组字符串：JSON.stringify([ownerType, ownerId, topicId])，详见 topicListManager.ts:39 */
    pinnedTopicKeys?: string[];
  };
  diary?: {
    hiddenFolders?: string[];
    folderOrder?: string[];
    collapsedCategories?: ("diary" | "cluster")[];
  };
  client?: {
    autoCheckUpdate?: boolean;
    // 注：devMode 为单机开发者诊断开关，严格遵循 developer/store.ts 契约，绝不进入备份
  };
}

export interface ConfigBackupFile {
  app: "vcp-mobile";
  kind: "vcp-mobile-settings-backup";
  version: 2;
  exportedAt: string;
  domains: BackupDomains;
  preferences: ClientPreferencesDto;
  agentPreferences?: Record<string, AgentLocalSettingsDto>;
  groupPreferences?: Record<string, GroupLocalSettings>;
  standaloneData?: {
    modelFavorites?: string[];
    tarvenRules?: TarvenRule[];
  };
}
```

---

## 5. 向后兼容性矩阵与迁移保障

| 来源版本 | 特征标识 | 解析与迁移行为 |
|---|---|---|
| **v1 遗留文件** | `version: 1`, `settings: { ... }` | 走 `parse_v1_backup()` 单向转换；只迁移提供的白名单连接与身份字段；无 `preferences`、无 `agentPreferences`、无 `groupPreferences`。 |
| **早期 v2-dev 文件** | `version: 2`, 仅包含 `domains`, `preferences: {theme, chat}`, `groupPreferences` | 完美解析。`agentPreferences`、`standaloneData` 自动解析为 `None` / 空集合，已有群组偏好与全局连接正常还原。 |
| **终态 v2 扩展文件** | `version: 2`, 包含全量扩展块 | 全量解析并按所有者分流应用。 |
| **未知高版本** | `version > 2` 或其他 `kind` | Rust 在信封预检时直接抛出明确错误：`"不支持的备份版本，请升级 VCP Mobile"`。 |
| **超大恶意文件** | 字节长度 > 1 MiB (`1024 * 1024`) | 在 Rust 文件流读取前直接拒载，防止 Android WebView 与移动端内存爆栈。 |
