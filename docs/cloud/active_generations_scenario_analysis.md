# VCPMobile 活跃生成注册表方案 — 全场景矩阵分析报告

> 本报告从【连接保持】、【温回归重连】、【冷启动】三种运行时场景出发，交叉分析流式/非流式、正常结束/用户中断/上游中断/App 被杀、单 Agent/多 Agent 群组并发等所有维度，验证 `active_generations` 注册表方案的完备性，并标记已发现的遗漏与修复建议。

---

## 0. 消息生命周期代码路径全景图

所有对话消息最终归结为以下三条调用链路：

| 入口 | 路径 | 骨架写入 | 终结器 | 注册表涉及 |
|------|------|----------|--------|------------|
| **Agent 单聊** | `chatHistoryStore.ts` → `handle_agent_chat_message` → `internal_process_agent_chat_message` → `perform_vcp_request` → `finalize_stream_message` | 前端 `chatStreamStore` 调用 `invoke("append_single_message")` | `finalize_stream_message` | ✅ |
| **群组接力** | `chatHistoryStore.ts` → `handle_group_chat_message` → `internal_process_group_chat_message` → 串行 for-loop → `perform_vcp_request` → `finalize_stream_message` | 前端 `chatStreamStore` 调用 `invoke("append_single_message")` | `finalize_stream_message` | ✅ |
| **重新生成** | `regenerateResponse` → `invoke("regenerate_topic_response")` → `internal_process_agent/group_chat_message(append_user_msg=false)` | 同上 | 同上 | ✅ |

> **关键发现**：骨架消息的持久化调用发生在**前端** `chatStreamStore.ts:L331`，通过 `invoke("append_single_message")` 异步调用 Rust 后端的 Tauri Command。  
> 这意味着 `active_generations` 的 `INSERT` 发生在前端 `chatStreamStore` 收到 `thinking` 事件后的异步 Tauri invoke 中，与后端 `perform_vcp_request` 的生命周期是**解耦**的。

---

## 1. 场景矩阵分析

### 1.1 连接保持（Connection Active — App 在前台，SSE 连接正常）

#### ✅ 1.1.1 流式 + 正常结束

```
时序：
  前端 invoke("handle_agent_chat_message")
    → Rust: append_single_message(user msg, role="user") → 不触发注册（user 角色）
    → Rust: perform_vcp_request(stream=true)
    → 前端收到 thinking 事件 → invoke("append_single_message") 写入骨架
      → 骨架 role="assistant", finish_reason=None → ✅ INSERT INTO active_generations
    → SSE Token 流正常接收
    → 收到 [DONE]
    → Rust: finalize_stream_message()
      → 更新 messages (content + finish_reason="completed")
      → ✅ DELETE FROM active_generations
    → 前端收到 end 事件 → UI 收尾
```

**结果**：✅ 注册表完美闭环。INSERT 和 DELETE 成对执行。

#### ✅ 1.1.2 流式 + 用户手动中止 (Abort)

```
时序：
  用户点击"中止" → 前端 invoke("interruptRequest")
    → Rust: oneshot::send(()) 触发 abort_rx
    → handle_streaming_request 的 tokio::select! 匹配到 abort_rx
    → is_aborted = true
    → perform_vcp_request 返回 Ok((res, true))
    → agent_chat_application_service:
        finish_reason = "cancelled_by_user"
        调用 finalize_stream_message()
          → 更新 messages (content + 中止后缀 + finish_reason="cancelled_by_user")
          → ✅ DELETE FROM active_generations
```

**结果**：✅ 正常。中止后 `finalize_stream_message` 被调用，注册表正常清理。

#### ✅ 1.1.3 非流式 + 正常结束

```
时序：
  Rust: perform_vcp_request(stream=false) → handle_non_streaming_request
    → 等待完整 HTTP 响应
    → 返回 Ok((res, false))
  agent_chat_application_service:
    full_content 存在
    → 调用 finalize_stream_message()
      → ✅ DELETE FROM active_generations
```

但这里有一个细节需要确认：**非流式请求是否会触发骨架写入？**

前端的 `chatStreamStore.processStreamEvent` 在收到 `thinking` 事件时才创建骨架。而 `handle_non_streaming_request` 不发送 `thinking` 事件（它直接在最后发送一个 `aurora` 事件携带完整内容）。

**BUT** — 看 `agent_chat_application_service.rs:L142`：
```rust
// 在发起 VCP 请求前，向前端发射 thinking 事件以初始化气泡
let _ = stream_channel.send(StreamEvent::thinking(thinking_id.clone(), context));
```

这行代码在 `perform_vcp_request` **之前**执行，且不区分流式/非流式。所以前端**始终**会收到 `thinking` 事件，始终会创建骨架并触发 `append_single_message`。

**结果**：✅ 注册和注销均正常。非流式也会经历完整的 INSERT → DELETE 生命周期。

#### ✅ 1.1.4 流式/非流式 + 上游网络错误（HTTP 级失败）

```
时序：
  前端收到 thinking 事件 → invoke("append_single_message")
    → ✅ INSERT INTO active_generations
  Rust: perform_vcp_request → handle_streaming_request
    → client.post() 发送失败 (DNS/TCP/TLS)
    → response_res 匹配 Err(e)
    → return Err(e.to_string())
  perform_vcp_request 返回 Err(...)
  agent_chat_application_service:
    match result {
        Err(e) => {
            log::error!(...);
            // ✅ v1.1.3 修复：DELETE FROM active_generations WHERE msg_id = ?
        }
    }
```

**结果**：✅ **已修复。** 当前 `agent_chat_application_service.rs:L189-194`、`group_chat_application_service.rs:L313-322` 以及 `vcp_client.rs:sendToVCP` 的 `Err` 分支（`vcp_client.rs:191-199`，仅流式模式）均会执行 `DELETE FROM active_generations WHERE msg_id = ?`，网络错误路径不再泄漏注册表记录。

#### ✅ 1.1.5 流式 + SSE 流中途读取错误

```
时序：
  SSE 流建立成功，开始接收 Token
  → 某次 lines.next() 返回 Some(Err(e))
  → handle_streaming_request 发送 error 事件并 break
  → 函数正常返回 Ok((json!({...}), is_aborted))
  → agent_chat_application_service 调用 finalize_stream_message()
    → ✅ DELETE FROM active_generations
```

**结果**：✅ 因为 `handle_streaming_request` 即使遇到流读取错误也是通过 `break` 跳出循环后**正常返回 `Ok`**，而不是 `return Err`。所以 `finalize_stream_message` 会被调用。

#### ✅ 1.1.6 流式 + SSE 流意外断开 (None — 连接被服务端关闭)

```
时序：
  lines.next() 返回 None（TCP 连接关闭）
  → handle_streaming_request:
    if !full_content.is_empty() {
      // 有内容，视为正常结束
    } else {
      // 无内容，发送 error 事件
    }
    → break → 正常返回 Ok(...)
  → finalize_stream_message() 被调用
    → ✅ DELETE FROM active_generations
```

**结果**：✅ 正常。

---

### 1.2 温回归重连（Warm Resume — App 切后台后切回，进程未被杀）

#### ✅ 1.2.1 App 切后台 → SSE 流仍在 → 切回前台

由于 Tauri 进程仍然活着，`tokio` 运行时持续运行，`reqwest` 的 HTTP 连接由 TCP keepalive 维持。在 Android 上：

- 如果后台时间较短（< 几分钟），SSE 连接大概率存活。流式生成会在后台正常完成，`finalize_stream_message` 会被调用。
- 用户切回前台时，前端的 `activeStreamMessages` 如果仍在内存中，UI 会直接展示最终结果。

**结果**：✅ 注册表正常闭环。

#### 🔶 1.2.2 App 切后台 → SSE 连接断开 → 切回前台

如果 Android 系统在后台回收了网络资源（WifiLock 释放、省电策略等），TCP 连接断开：

- `handle_streaming_request` 中 `lines.next()` 会返回 `Some(Err)` 或 `None`
- 函数正常返回 `Ok(...)`
- `finalize_stream_message` 被调用
  - ✅ `DELETE FROM active_generations` 执行成功
- 前端收到 `error` 或 `end` 事件
- 内存中的 `activeStreamMessages` 保留了已接收的部分内容

**结果**：✅ 注册表正常闭环。但用户会看到不完整的消息，且当前没有自动重连机制（这属于断点续传的未来工作）。

---

### 1.3 冷启动（Cold Start — App 被系统杀死后重新打开）

#### 🔶 1.3.1 流式生成中 App 被强杀

```
状态快照（杀死瞬间）：
  - messages 表: 骨架记录存在，content="" , finish_reason=NULL
  - active_generations 表: msg_id 记录存在（INSERT 已执行）
  - 内存: 全部丢失

冷启动后：
  - SELECT * FROM active_generations → 精确命中被中断的 msg_id
  - 向 VCPToolBox 查询状态 → 拉取/续接/标记失败
  - 清理 active_generations 记录
```

**结果**：✅ 这是该方案设计的核心场景，完美运作。

#### ⚠️ 1.3.2 骨架写入前 App 被杀（极端时序）

```
时序：
  前端收到 thinking 事件
  → 调用 invoke("append_single_message") [异步，尚未完成]
  → App 被杀
```

此时 `append_single_message` 的 SQLite 事务可能未提交，意味着：
- `messages` 表中**没有**骨架记录
- `active_generations` 表中**也没有**记录

**结果**：✅ 安全。冷启动时 `SELECT * FROM active_generations` 返回空，系统认为没有未完成任务。对于用户而言，这次生成被视为"从未发生"，用户的 `user` 消息已经持久化（在 `agent_chat_application_service.rs:L76` 中先行写入），用户可以在 UI 中看到自己的消息但没有回复，手动重发即可。

这是可接受的边界行为 — 窗口极短（从 thinking 事件到 invoke 完成通常 < 50ms）。

#### ⚠️ 1.3.3 非流式生成中 App 被杀

非流式请求中，Rust 后端在等待完整的 HTTP 响应。如果此时 App 被杀：
- 骨架记录和 `active_generations` 已写入（因为 thinking 事件在请求前发射）
- 冷启动时会检测到该记录
- 向服务端查询 → 大概率服务端没有缓存非流式请求的结果

**结果**：🔶 注册表工作正常，但服务端对非流式请求的断点恢复可能不支持（非流式请求通常是"一次性"的，服务端不缓存结果）。建议在冷启动对齐时，如果服务端返回 `not_found`，将消息标记为 `error` 并清理注册表。

---

### 1.4 多 Agent 群组并发场景

#### ✅ 1.4.1 群组接力赛 — 正常完成

```
时序（3 个 Agent 串行接力）：
  Speaker A:
    → 前端收到 thinking → invoke("append_single_message") → INSERT active_generations (msg_A)
    → perform_vcp_request → finalize_stream_message → DELETE active_generations (msg_A)
  Speaker B:
    → 前端收到 thinking → invoke("append_single_message") → INSERT active_generations (msg_B)
    → perform_vcp_request → finalize_stream_message → DELETE active_generations (msg_B)
  Speaker C:
    → 前端收到 thinking → invoke("append_single_message") → INSERT active_generations (msg_C)
    → perform_vcp_request → finalize_stream_message → DELETE active_generations (msg_C)
```

**结果**：✅ 每个 Agent 的消息独立注册和注销，互不干扰。

#### ✅ 1.4.2 群组接力赛 — 用户中途中止

```
时序：
  Speaker A 正常完成 → msg_A 注册/注销完毕
  Speaker B 生成中，用户点击"中止群组"
    → interruptGroupTurn() 设置 cancelled_turns 令牌
    → 当前 perform_vcp_request 通过 interruptRequest 中止
    → finalize_stream_message 被调用 → DELETE active_generations (msg_B)
  Speaker C:
    → for 循环检查 cancelled_turns → break，不再执行
```

**结果**：✅ 已完成的 Agent 已清理，被中止的 Agent 通过 finalize 清理，未执行的 Agent 不会产生注册。

#### ✅ 1.4.3 群组接力赛 — Speaker B 网络错误

```
时序：
  Speaker A 正常完成 → msg_A 注册/注销完毕
  Speaker B:
    → 前端收到 thinking → INSERT active_generations (msg_B)
    → perform_vcp_request 返回 Err
    → group_chat_application_service: if let Err(e) = res_result { log::error!(...); DELETE active_generations (msg_B); }
    → ✅ active_generations (msg_B) 已被清理
  Speaker C: 继续执行（可能正常或异常）
```

**结果**：✅ **已修复。** `group_chat_application_service.rs` 的 `Err` 分支现已删除对应 `active_generations` 记录，不再泄漏。

---

### 1.5 浮动助手（Assistant Chat — `handle_assistant_chat_stream`）

浮动助手是一个特殊路径（`agent_chat_application_service.rs:L206`）：
- **不写入本地数据库**：不调用 `append_single_message` 或 `finalize_stream_message`
- 仅通过 `StreamEvent` 管道向前端推送内容，前端也不持久化
- 前端的 `chatStreamStore` 收到 `thinking` 事件后，调用 `invoke("append_single_message")` 时使用的 `topicId` 是 `"assistant_chat"`

**但等等** — 前端 `chatStreamStore.ts:L331` 的骨架写入逻辑是通用的，它不区分是 Agent 聊天还是浮动助手。所以浮动助手的 `thinking` 事件也会触发 `invoke("append_single_message")`，进而触发 `INSERT INTO active_generations`。

然而，浮动助手的 `handle_assistant_chat_stream` **不调用** `finalize_stream_message`：

```rust
// 7. 处理请求结果并补发流终结事件
match result {
    Ok((res, is_aborted)) => {
        // 发送 end 事件让前端知道传输完毕
        let _ = stream_channel.send(StreamEvent::end(...));
        // ← 没有调用 finalize_stream_message!
    }
    Err(e) => {
        let _ = stream_channel.send(StreamEvent::error(...));
    }
}
```

**结果**：🔶 **待确认。** Rust 后端 `handle_assistant_chat_stream` 确实不调用 `finalize_stream_message`，但注册表是否真正泄漏取决于前端 `chatStreamStore.ts` 在 `topicId === "assistant_chat"` 时是否仍会调用 `invoke("append_single_message")`。若前端仍调用，则 `active_generations` 会被插入但无法被后端清理；若前端已过滤，则无泄漏。需人工核对前端 `chatStreamStore.ts` 后确认。

---

## 2. 已发现 BUG 汇总与修复建议

### ✅ BUG-1（已修复）：`perform_vcp_request` 返回 `Err` 时注册表泄漏

**影响范围（v1.1.3 已修复）**：
- `agent_chat_application_service.rs:L189-194` — Agent 单聊 Err 分支执行 `DELETE FROM active_generations WHERE msg_id = ?`
- `group_chat_application_service.rs:L313-322` — 群组接力 Err 分支执行同样的删除
- `vcp_client.rs:sendToVCP` 的流式请求 Err 分支（`vcp_client.rs:191-199`）也会清理注册表

**修复状态**：当前代码已在上述三个入口的错误路径中主动删除 `active_generations` 记录，`perform_vcp_request` 返回 `Err` 时不再泄漏。

### 🔶 BUG-2（待确认）：浮动助手路径是否泄漏

**影响范围**：`handle_assistant_chat_stream` 不调用 `finalize_stream_message`；若前端 `chatStreamStore.ts` 仍对 `assistant_chat` 调用 `append_single_message`，则会导致 `active_generations` 插入后无人清理。

**待确认项**：
- 前端 `chatStreamStore.ts` 当前是否已过滤 `topicId === "assistant_chat"` 的骨架写入。
- 若未过滤，推荐在**前端**过滤（浮动助手不持久化，不应写入注册表），或在 `handle_assistant_chat_stream` 的 `Ok/Err` 双分支末尾追加 `DELETE FROM active_generations WHERE msg_id = ?` 作为兜底。

---

## 3. 修复后的完整场景通过矩阵

| 场景 | 流式 | 非流式 | 用户中止 | 上游中断 | App被杀 | 多Agent | 浮动助手 |
|------|:----:|:------:|:--------:|:--------:|:-------:|:-------:|:--------:|
| 连接保持 | ✅ | ✅ | ✅ | ✅* | N/A | ✅* | 🔶 |
| 温回归 | ✅ | ✅ | ✅ | ✅* | N/A | ✅* | 🔶 |
| 冷启动 | ✅ | 🔶 | ✅ | ✅* | ✅ | ✅* | 🔶 |

> `✅*` = 修复 BUG-1（已完成）并确认/修复 BUG-2 后可达到  
> `🔶` = 非流式冷启动恢复依赖服务端是否缓存结果（属于后续服务端工作）

---

## 4. 结论

**活跃生成注册表方案在核心设计上是正确且高效的**，但当前的代码修改仅覆盖了"正常路径"（happy path），遗漏了以下两个错误路径：

1. **`perform_vcp_request` 返回 `Err` 时的注册表清理**（影响 Agent 单聊、群组接力、sendToVCP 三个入口）
2. **浮动助手路径的注册表隔离**（浮动助手不走 finalize_stream_message，但前端会触发骨架写入）

修复这两个问题后，该方案可以在所有场景下实现注册表的完美闭环，为后续的云端断点恢复提供可靠的本地事务日志。

---

*最后更新：2026-07-04 | VCP Mobile v1.1.3*
