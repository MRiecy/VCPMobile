# VCP 移动端云联协与双端协议规约（VCPMobile Cloud Spec）

本规约面向 VCP 开发者与生态架构师，旨在定义 **VCPMobile（移动端）** 与 **VCPToolBox（桌面/云服务端）** 之间的云协同理念、业务设想及底层通信规约。

---

## 1. 核心设计理念

### 1.1 算力锚定桌面，感知延伸移动
在 VCP 生态中，**生产力无法脱离桌面端**。桌面端（VCPToolBox）拥有完整的本地代码库、RustVexus 向量引擎、复杂的 Python/JS 多语言插件运行时以及高负载的 AI 模型。
移动端（VCPMobile）的定位并非一个独立的 Agent 运行环境，而是**桌面端 Agent 系统的“感官延伸”与“决策终端”**。

### 1.2 零后台常驻（Zero-Daemon）与事件驱动
由于 Android 系统对后台进程和能耗的极度严苛限制，移动端不适合维持高频、双向的实时长连接。VCPMobile 采用**完全事件驱动**的架构：
*   **前台状态**：使用标准的流式和实时通信。
*   **后台状态**：主应用彻底休眠（释放所有 WebView 内存），由一个极轻量的隔离进程（`:push`）维系单向的 **SSE（Server-Sent Events）** 监听通道。
*   **交互触发**：所有的决策和通知，均通过单向推送（Push）触达手机，用户通过通知栏或深链拉起主应用进行即时反馈，完事即走。

---

## 2. 核心协同设想

我们希望在 VCP 中实现以下两种跨越空间的全新交互范式：

### 设想 A：异步任务传呼（Async Task Paging）
VCP 拥有强大的异步插件系统。当 Agent 在后台执行长周期任务（如：生成视频、抓取并分析 100 个网页、跑测试集）时，用户无需在电脑前等待。
*   **协同流程**：任务完成或阶段性结束时，VCPToolBox 向移动端推送一个“任务完成卡片”。
*   **移动端表现**：通知栏弹出原生通知，点击后通过 Deep Link 直达对应的会话气泡，查看生成的多媒体结果或数据报表。

### 设想 B：带外工具审计（Out-of-Band Tool Auditing）
对于高风险命令（如 `rmdir`、执行 shell 脚本），VCP 默认会触发安全审计挂起。
*   **协同流程**：Agent 挂起执行流，向移动端推送一个“审核请求卡片”。
*   **移动端表现**：手机收到紧急通知。用户点击通知唤醒并打开 `VCPMobile` 主应用，在 App 内的审计面板上进行决策，从而安全地解除挂起。

---

## 3. 技术规约：双端通信协议

### 3.1 单向推送通道（SSE Push Protocol）

VCPToolBox 应当提供（或通过插件暴露）一个符合 ntfy 规范的 **SSE（Server-Sent Events）推送端点**。

*   **监听端点（手机端监听）**：`GET /vcp-push/<topic>`
*   **发布端点（桌面端发送）**：`POST /vcp-push/<topic>`
*   **鉴权方式**：请求头携带 `Authorization: Bearer <VCP_Key>`

#### 3.1.1 协议报文规范

所有推送数据必须采用标准 SSE 格式，每个数据块以双换行符（`\n\n`）结尾。

##### 报文格式示例：
```http
event: message
id: <时间戳/递增ID>
data: <结构化 JSON 载荷>
\n\n
```

##### 维系心跳（Heartbeat）：
服务端必须每隔 **15 - 30 秒** 写入一个空注释行，以防止移动网关或运营商因超时强行断开 TCP 连接：
```http
: keepalive
\n\n
```

---

#### 3.1.2 业务载荷协议（建议 JSON 示例）

以下载荷仅为移动端解析通知并进行 UI 渲染的**参考示例**。VCPToolBox 服务端可根据实际业务需要自由扩充或调整字段，移动端做弹性解析兼容：

##### 示例 1：`async_task_completed`（异步任务完成）
```json
{
  "event_type": "async_task_completed",
  "agent_name": "智能体名称 (如 Nova)",
  "task_title": "任务标题 (如 B站视频发布)",
  "summary": "任务执行结果的精简摘要",
  "topic_id": "关联的 VCP 会话 ID",
  "msg_id": "关联的 VCP 消息气泡 ID"
}
```

##### 示例 2：`tool_approval_requested`（工具执行审计）
```json
{
  "event_type": "tool_approval_requested",
  "agent_name": "智能体名称",
  "approval_id": "本次审计的唯一 RequestID",
  "tool_name": "调用的工具/插件名称 (如 PowerShellExecutor)",
  "detail": "具体的执行参数或命令文本 (如 rmdir /s /q ...)",
  "reason": "Agent 给出的一句话执行理由"
}
```
