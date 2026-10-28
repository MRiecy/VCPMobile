# 02-数据契约与IPC接口规范

> 本文档规范 SQLite 数据库 Schema 变更、Rust 核心领域模型、Vue 前端 DTO 契约及新增的 Tauri IPC 命令。

> **契约以实施稿为准**：迁移编号为 `0105`；voice 为必有结构，mode 包含默认 `none`；无 `refText`；保存携 baseline 并返回权威配置。完整接口见 [05-实施决策与验证记录](05-实施决策与验证记录.md)，以下代码为初稿示意。

---

## 1. SQLite 数据库变更 (Schema Evolution)

### 变更方案：在 `agents` 表中新增 `voice_config` 列
在 SQLite 迁移体系中，为 `agents` 表增加持久化列，采用紧凑且扩展性强的 JSON 序列化存储：

```sql
-- migrations/0101_add_agent_voice_config.sql
ALTER TABLE agents ADD COLUMN voice_config TEXT NOT NULL DEFAULT '{}';
```

---

## 2. Rust 与 Vue 共享数据契约

### 2.1 Agent 专属语音设置契约 (AgentVoiceConfig)

#### Rust 领域实体 (`src-tauri/src/vcp_modules/agent/agent_types.rs`):
```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentVoiceMode {
    Builtin,
    Clone,
}

impl Default for AgentVoiceMode {
    fn default() -> Self {
        Self::Builtin
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentVoiceConfig {
    /// 是否启用独立音色配置；false 时自动继承全局默认音色
    #[serde(default)]
    pub enabled: bool,

    /// 音色模式：预置音色 vs 音色复刻
    #[serde(default)]
    pub mode: AgentVoiceMode,

    /// 预置音色标识符（例如: "冰糖", "茉莉", "mimo_default" 等）
    #[serde(default = "default_preset_voice")]
    pub voice: String,

    /// 克隆声音模型资产 ID（绑定 registry.json 中的样本 ID）
    #[serde(default)]
    pub clone_sample_id: String,

    /// 流式朗读（收到第一分片即开始播放解码）
    #[serde(default = "default_true")]
    pub stream: bool,
}

fn default_preset_voice() -> String {
    "冰糖".to_string()
}
fn default_true() -> bool {
    true
}

impl Default for AgentVoiceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: AgentVoiceMode::Builtin,
            voice: default_preset_voice(),
            clone_sample_id: String::new(),
            stream: true,
        }
    }
}
```

并在 `AgentConfig` 结构体中挂载：
```rust
pub struct AgentConfig {
    pub id: String,
    pub name: String,
    pub system_prompt: String,
    pub mobile_system_prompt: String,
    pub model: String,
    pub temperature: f64,
    pub context_token_limit: i32,
    pub max_output_tokens: i32,
    pub stream_output: bool,
    pub use_temperature: bool,
    pub avatar_calculated_color: Option<String>,
    pub topics: Vec<Topic>,

    /// 🎙️ 新增：专属语音配置（为空或未启用时回退全局设置）
    #[serde(default)]
    pub voice: Option<AgentVoiceConfig>,
}
```

#### Vue 前端契约 (`src/core/types/assistant.ts`):
```typescript
export interface AgentVoiceConfigDto {
  enabled: boolean;
  mode: 'builtin' | 'clone';
  voice: string;
  cloneSampleId: string;
  stream: boolean;
}

export interface AgentConfigDto {
  id: string;
  name: string;
  systemPrompt: string;
  mobileSystemPrompt: string;
  model: string;
  temperature: number;
  contextTokenLimit: number;
  maxOutputTokens: number;
  streamOutput: boolean;
  useTemperature: boolean;
  avatarCalculatedColor: string | null;
  topics: TopicDto[];
  // 🎙️ 专属语音
  voice?: AgentVoiceConfigDto;
}
```

---

### 2.2 克隆声音模型资产契约 (VoiceCloneModelDto)

#### Rust 结构体 (`src-tauri/src/vcp_modules/chat/voice_service/samples.rs`):
```rust
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VoiceCloneModelDto {
    pub id: String,
    pub name: String,
    pub file_name: String,
    pub format: String,        // "mp3" | "wav"
    pub file_bytes: u64,
    pub duration_ms: u32,
    pub file_hash: String,
    pub ref_text: Option<String>,
    pub created_at: u64,
}
```

---

## 3. Tauri IPC 命令注册表规范

在 `src-tauri/src/commands.rs` 中更新并暴露以下命令：

| 命令标识符 | 参数 | 返回值 | 职责描述 |
| :--- | :--- | :--- | :--- |
| `list_voice_clone_samples` | 无 | `Result<Vec<VoiceCloneModelDto>, String>` | 遍历声音库注册表，返回全部可用克隆音色模型 |
| `import_voice_clone_sample` | `path: String, name: String, ref_text: Option<String>` | `Result<VoiceCloneModelDto, String>` | 校验并持久化用户导入的音频，登记入库 |
| `delete_voice_clone_sample` | `sample_id: String` | `Result<bool, String>` | 删除指定音色文件并从注册表中剔除 |
| `preview_tts` | `tts_id: String, text: String, settings: TtsPreviewSettings` | `Result<(), String>` | 按指定参数合成并播放试听音频（保留现有能力） |
| `stop_tts` | `tts_id: String` | `Result<(), String>` | 中断当前正在播放的合成试听 |
| `save_agent_config` | `agent: AgentConfig` | `Result<bool, String>` | 保存 Agent 配置，同步保存其中的 `voice` 对象至 SQLite |
