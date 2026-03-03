### 📄 VCP Mobile: Rust 网络层重载规格书 (Internal Revision 1.0)

#### 1. 目标 (Goal)

模块化实现rust重写，完全替代原有的 `vcpClient.js` 逻辑。

#### 2. 技术栈建议 (Stack)

- **HTTP Client**: `reqwest` (支持异步和流)
- **Serialization**: `serde` / `serde_json`
- **Async Runtime**: `tokio`
- **Streaming**: `futures_util` (处理 `StreamExt`)

#### 3. 核心逻辑功能点 (Logic Points to Port)

1. **动态路由切换 (Endpoint Switching)**:
  - 逻辑：读取 `AppData/settings.json`。
  - 如果 `enableVcpToolInjection == true`，将 BaseURL 后缀改为 `/v1/chatvcp/completions`。
2. **上下文注入 (Context Injection)**:
  - **音乐信息**: 读取 `songlist.json`，并将当前播放信息和点歌台标识 `{{VCPMusicController}}` 动态插入到 `messages[0]` (System Message) 的开头或结尾。
  - **UI 规范**: 强制在 System Message 中注入 `输出规范要求：{{VarDivRender}}`。
3. **流式解析 (SSE Decoding)**:
  - 这是重中之重！Rust 需要建立长连接。
  - 按行读取响应体。识别 `data:`  前缀。
  - **特殊信号处理**: 遇到 `data: [DONE]` 时，立即通过 Tauri Event 广播“结束”信号。
  - **JSON 提取**: 解析 `choices[0].delta.content`，并通过 Tauri 的 `Window::emit` 实时推送到前端。
4. **请求中止 (Abort Mechanism)**:
  - 原生 JS 使用 `AbortController`。
  - Rust 端建议使用 `tokio::sync::oneshot` 或者维护一个 `Arc<DashMap<String, AbortHandle>>`，通过消息 ID 来中止对应的异步任务。

#### 4. Rust 结构体参考 (Data Structures)

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct VcpRequestPayload {
    pub vcp_url: String,
    pub vcp_api_key: String,
    pub messages: Vec<serde_json::Value>,
    pub model_config: serde_json::Value,
    pub message_id: String,
    pub context: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct StreamEvent {
    pub r#type: String, // "data", "end", "error"
    pub chunk: Option<serde_json::Value>,
    pub message_id: String,
    pub error: Option<String>,
}
```

---

### 🎨 Nova 的 PM 寄语（给执行 AI）：

“Hi 伙计！在重写时请注意：**移动端的网络环境非常脆弱**。原版 JS 里的 30 秒硬超时在 Rust 端建议改为可配置的。另外，请确保在 Rust 端处理好 UTF-8 字符的分段截断问题（Buffer 处理），不要让前端收到乱码。加油，看好你哦！”