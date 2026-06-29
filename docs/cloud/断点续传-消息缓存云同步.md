# VCP 生态系统断点续传（Resumable Stream）双端接口规范

为了实现流式生成在移动端网络闪断、切后台、进程被杀等场景下的无缝恢复，本文档定义了 `VCPMobile`（客户端）与 `VCPToolBox`（服务端）之间的断点续传协议与接口规范。

---

## 1. 核心设计原则

1. **客户端无状态**：客户端本地无需高频写入流式中间文本，仅需通过 `active_generations` 注册表记录当前活跃的 `msg_id`。
2. **服务端权威缓存**：服务端（VCPToolBox）必须对正在生成的流式文本进行内存或 Redis/SQLite 级的高频追加缓存（`message_cache`）。
3. **字符位移对齐（Character-based Offset）**：
   * 接口中的偏移量（Offset）统一使用 **JS 字符串字符长度（UTF-16 码元数量）**。
   * 服务端使用 `substring(offset)` 进行切片，客户端使用 `content.length`（JS）或 `chars().count()`（Rust）进行计数，彻底规避中文字符在 UTF-8（3 字节）与 UTF-16 之间的多字节错位问题。

---

## 2. 接口定义

### 2.1 接口一：查询消息状态与缓存内容（用于冷启动对齐）
当客户端冷启动发现 `active_generations` 表中有残留记录时，首先调用此接口向服务端对齐进度。

* **请求方法**：`GET`
* **请求路径**：`/api/chat/messages/{msg_id}`
* **请求头**：
  ```http
  Authorization: Bearer <API_KEY>
  ```
* **成功响应 (200 OK)**：
  ```json
  {
    "msgId": "msg_group_1782630718904_user_pwgvx04_agent_001_1719586800000",
    "status": "streaming", // "streaming" (生成中) | "completed" (已完成) | "failed" (已失败/不存在)
    "content": "这是服务端目前已经生成出来的所有文本...", 
    "characterLength": 21, // 对应 content.length (UTF-16 字符数)
    "finishReason": null // null | "completed" | "error" | "cancelled_by_user"
  }
  ```
* **异常处理**：
  若任务在服务端不存在或完全失效，返回 `404 Not Found` 或 `status: "failed"`，客户端将本地消息标记为 `error` 并注销活跃记录。

---

### 2.2 接口二：断点流式接续（用于温回归/网络重连）
当客户端持有部分文本，需要续接后续的流式输出时调用。

* **请求方法**：`GET`（或带有查询参数的请求）
* **请求路径**：`/api/chat/stream?msg_id={msg_id}`
* **请求头**：
  ```http
  Accept: text/event-stream
  Authorization: Bearer <API_KEY>
  Last-Event-ID: 450
  ```
  > 💡 **关键点**：使用标准 SSE 请求头 `Last-Event-ID` 传递客户端当前已接收的字符偏移量（例如 `450`）。
* **服务端行为逻辑**：
  1. **解析 `Last-Event-ID`**：获取偏移量 `offset`（若无则默认为 `0`）。
  2. **读取服务端缓存**：从 `message_cache` 中取出当前已生成的全量文本 `full_text`。
  3. **计算并补发 Delta**：
     * 若 `offset < full_text.length`：服务端立即发送一个包含差异文本的初始化 `aurora` 帧，内容为 `full_text.substring(offset)`。
     * 若 `offset >= full_text.length`：不补发历史。
  4. **根据任务状态后续处理**：
     * **若任务已结束 (`status == "completed"`)**：补发 Delta 后，立即发射 `data: [DONE]` 并关闭连接。
     * **若任务仍在生成 (`status == "streaming"`)**：补发 Delta 后，将该 SSE 连接桥接到大模型实时的 Stream 管道，继续流式推送后续产生的 Token 帧。

---

## 3. 客户端（VCPMobile）流接续状态机

客户端在接收 SSE 流时，需根据 `active_generations` 和网络状态进行如下流转：

```
                [ 网络断开 / 前台回归 ]
                          │
                          ▼
            获取当前已渲染文本的字符长度:
           offset = msg.content.length
                          │
                          ▼
             发起 GET /api/chat/stream 
             携带 Last-Event-ID: offset
                          │
         ┌────────────────┴────────────────┐
         ▼                                 ▼
   [ 收到首帧 Delta ]               [ 服务端返回 404 / 失败 ]
   追加至内存中，UI 继续打字机       本地消息置为 error，
   并持续接收后续实时 Token          注销 active_generations
```

---

## 4. 服务端（VCPToolBox）伪代码参考

服务端跟随更新时，可参考以下 Node.js (Express/SSE) 实现框架：

```javascript
app.get('/api/chat/stream', async (req, res) => {
  const { msg_id } = req.query;
  const lastEventId = req.headers['last-event-id'];
  const offset = lastEventId ? parseInt(lastEventId, 10) : 0;

  // 1. 设置 SSE 响应头
  res.setHeader('Content-Type', 'text/event-stream');
  res.setHeader('Cache-Control', 'no-cache');
  res.setHeader('Connection', 'keep-alive');

  // 2. 获取当前生成任务的缓存
  const task = await db.get("SELECT status, content FROM message_cache WHERE msg_id = ?", [msg_id]);
  if (!task) {
    res.write(`event: error\ndata: ${JSON.stringify({ error: "Task not found" })}\n\n`);
    return res.end();
  }

  // 3. 补发差异数据 (Delta)
  if (offset < task.content.length) {
    const delta = task.content.substring(offset);
    res.write(`event: aurora\ndata: ${JSON.stringify({ chunk: delta })}\n\n`);
  }

  // 4. 判定后续行为
  if (task.status === 'completed') {
    // 已结束，直接关闭
    res.write('data: [DONE]\n\n');
    return res.end();
  } else {
    // 仍在生成，将 res 注册到广播订阅器中，监听 LLM 实时 Token 并转发
    streamManager.subscribe(msg_id, res);
  }
});
```
