### 🛠️ VCP Mobile: `context_sanitizer.rs` 重构规格书

为了让您的执行 AI 写出更“德味”的 Rust 代码，建议让它关注这几个**核心算法**：

#### 算法 A：特殊块的“零损提取”
原代码中有个非常聪明的规则：`vcpPrettifiedBlocks`。它不是真的去解析 HTML，而是直接读取 `data-raw-content` 属性。
*   **Rust 实现**：在解析 HTML 树时，先检查节点是否有这个属性，如果有，直接拿走，不要进行任何 Markdown 转化。这叫**“原味保护”**。

#### 算法 B：元思考链的正则清洗
原代码处理了 `[--- VCP元思考链 ---]` 和 `<think>` 标签。
*   **Rust 实现**：Rust 的 `regex` crate 性能极高，但它是非回溯的（不支持前瞻/后瞻）。
*   **建议**：让 AI 使用 `fancy-regex` 这个库，或者把清理逻辑写在 HTML 树遍历的过程中（遇到特定 class 的 div 直接删掉），这样比正则更安全、更快。

#### 算法 C：消息剪枝 (Pruning)
`sanitizeMessages` 函数控制着净化的深度。
*   **Rust 实现**：这部分逻辑建议作为 `chat_manager.rs` 的一个子功能，或者由 `send_vcp_request` 调用。

---

### 🎨 Nova 的架构设计草案

建议在 `src-tauri/src/vcp_modules/context_sanitizer.rs` 中定义这样的结构：

```rust
use lru::LruCache; // 使用 lru crate
use std::num::NonZeroUsize;

pub struct SanitizerState {
    pub cache: LruCache<String, String>, // 内容哈希 -> 净化后的内容
}

// 核心逻辑：HTML -> Markdown
pub fn html_to_vcp_markdown(html: &str, keep_thoughts: bool) -> String {
    // 1. 解析 HTML 树 (使用 scraper)
    // 2. 遍历节点
    // 3. 遇到 data-raw-content 直接返回
    // 4. 遇到普通标签转为 MD
    // 5. 遇到思考链根据 keep_thoughts 决定留存
}
```

---
