# JEV 协议与决策生命周期审计

日期：2026-10-19

视角：Melchior（逻辑与系统）
范围：VCPChat 当前生产 JEV client、全局服务、群聊决策及会话编排；VCPMobile Rust owner 的适配边界。本文只报告源码事实、推导风险和实施建议，不改变产品行为。

## 1. 结论与证据范围

**源码事实**：JEV 是独立的非流式结构化决策调用，不走普通聊天完成接口。它决定发言成员及是否继续；真正的回复仍由现有群聊邀请链执行，因此 Agent 的模型、流式输出、工具及消息持久化不应被复制到第二套实现。VCPChat 的 JEV 会话以 `groupId + topicId` 为 owner，支持运行中接纳用户消息和插队；VCPMobile 当前群轮则是一次独占的串行接力，需要扩展生命周期，而不是在 Vue 中追加一个循环。

**建议**：先定协议和群配置的数据边界，再实现独立 JEV transport/decision，最后把持续会话接入现有群轮、输出、语音和恢复 owner。同步 DTO 只传持久群配置；运行队列、仲裁 revision、取消 token 和服务 API Key 不应混进群配置 DTO。

本次已阅读根及 Rust 局部 `AGENTS.md`、跨平台规范；未在 VCPChat 发现 `AGENTS.md`。未修改生产代码，未调用外部 JEV API，未验证供应商当前可用性、价格、上下文额度或设备行为。本文供应商 URL/模型仅表示 VCPChat 当前生产常量。

## 2. 生产入口及职责

| 层 | 当前职责 | 代码证据 |
|---|---|---|
| 全局服务建立 | `GlobalJevService` 读取 settings manager，注册 `globalThis.vcpJev`；每次调用重新读设置 | [main.js](E:/VCPChat/main.js) L1166–1175 |
| 通用 IPC | `jev:get-status` 与 `jev:decide`；后者只接收 `state/questions`，返回原始结果或结构化错误 | [main.js](E:/VCPChat/main.js) L1220–1252 |
| 配置与启用 | `GlobalJevService` 提供启用判断、配置标准化、状态；不进行群聊决策 | [globalJevService.js](E:/VCPChat/modules/services/globalJevService.js) L37–85 |
| HTTP client | 参数校验、Key 池轮询、headers、代理、HTTP 请求、重试；原样返回 response.data | [jevClient.js](E:/VCPChat/modules/services/jevClient.js) L141–235、L277–409 |
| 群决策 | 组装 `state/questions`，把返回概率转为串行成员队列或停止原因 | [jevDecisionMode.js](E:/VCPChat/Groupmodules/modes/jevDecisionMode.js) L100–274 |
| 持续会话 | 保存队列、当前成员、revision、轮次、停止状态及运行 Promise | [jevGroupSessionOrchestrator.js](E:/VCPChat/Groupmodules/jevGroupSessionOrchestrator.js) L43–89、L209–421 |
| 回复执行 | orchestrator 调 `handleInviteAgentToSpeak(..., {skipSummary:true, signal})`，继续使用普通 VCP 请求 | [groupchat.js](E:/VCPChat/Groupmodules/groupchat.js) L296–328、L1879–1925 |

## 3. 服务配置、Key 与 URL

### 3.1 全局配置字段

以下是 `GlobalJevService._resolveConfig` 的实际映射，不是从 UI 文案推断。来源：[globalJevService.js](E:/VCPChat/modules/services/globalJevService.js) L5–24、L37–54。

| settings 字段 | client 字段 | 默认/范围及行为 |
|---|---|---|
| `jevEnabled` | `enabled` | 只有严格 `true` 才启用 |
| `jevProvider` | `provider` | `typesafe` / `openrouter`；其他值归一为 `typesafe` |
| `jevApiUrl` | `url` | TypeSafe：`https://api.typesafe.ai/v1/systemone`；OpenRouter：`https://openrouter.ai/api/alpha/decisions` |
| `jevApiKey` | `apiKey` | trim 后字符串；允许多个 Key |
| `jevModel` | `model` | TypeSafe：`jev-latest`；OpenRouter：`~typesafe/jev-latest` |
| `jevTimeoutMs` | `timeoutMs` | 30000；整数，1000–300000 |
| `jevMaxRetries` | `maxRetries` | 2；整数，0–10；总尝试数是 `1 + maxRetries` |
| `jevRetryBaseDelayMs` | `retryBaseDelayMs` | 500；整数，1–30000 |
| `jevProxyUrl` | `proxyUrl` | 空；可选 HTTP/HTTPS 代理 |
| `jevHttpReferer` | `referer` | 空；仅 OpenRouter 添加 `HTTP-Referer` |
| `jevAppTitle` | `title` | `VCPChat`；仅 OpenRouter 添加 `X-OpenRouter-Title` |

JEV 服务独立于普通 `vcpUrl/vcpApiKey`，两者不能因都使用 Bearer 就共用一个 endpoint 推导器。普通群回复仍通过 `resolveGroupChatUrl(globalVcpSettings.vcpUrl, ...)` 发往 VCP，见 [groupchat.js](E:/VCPChat/Groupmodules/groupchat.js) L1912–1925。

### 3.2 配置优先级与环境变量

**源码事实**：通用 client 的字符串配置使用 `overrides || constructor options || environment || default`；数值使用 `??` 后解析。环境字段为 `JEV_PROVIDER/API_URL/API_KEY/MODEL/TIMEOUT_MS/MAX_RETRIES/RETRY_BASE_DELAY_MS/PROXY_URL/HTTP_REFERER/APP_TITLE`。见 [jevClient.js](E:/VCPChat/modules/services/jevClient.js) L62–138。

实际全局服务先填入 provider、URL、model、数值默认，因此这些非空字段通常遮住 client 环境回退；空 API Key、空 proxy、空 referer 仍可能落到进程环境。仅含空白的配置字符串会先 trim 成空，再触发 client 回退。不能把通用 client 支持环境变量直接等同为“所有全局面板字段均由环境变量覆盖”。

**建议**：Mobile 使用现有 Rust 配置 owner 提供的显式本机设置作为产品配置，不为了复制 Node 开发环境增加手机端环境变量入口。若需要 host fixture 注入 endpoint、Key 或 client，应在明确的依赖注入边界完成，不能从 `state/questions` 接受 transport 参数。

### 3.3 Key、token 与 endpoint 行为

- Key 按英文逗号、中文逗号或 `|` 拆分，逐项 trim，删除空项；没有去重。每组 Key 列表以 JSON 字符串为池身份，各自维护游标。每次 decide 前同步选择一次 Key，全部重试固定该 Key，不因 401/429 自动换 Key。见 [jevClient.js](E:/VCPChat/modules/services/jevClient.js) L32–36、L141–148、L354–356。
- HTTP header 为 `Authorization: Bearer <key>` 和 `Content-Type: application/json`。client 的内部 `options.headers` 最后展开，可覆盖默认 header；通用 renderer IPC 没有传入这项权限。见同文件 L277–286；[main.js](E:/VCPChat/main.js) L1235–1240。
- 请求 body 没有 `max_tokens`、temperature、stream 或 `messages`；仅有 `model/state/questions`。`usage` 是返回的原始附加值，不参与停止算法。见 [jevClient.js](E:/VCPChat/modules/services/jevClient.js) L349–353；[jevDecisionMode.js](E:/VCPChat/Groupmodules/modes/jevDecisionMode.js) L231。
- client 直接 `axios.post(config.url, body)`，不追加 `/chat/completions`、不剥后缀、不做相对路径解析或供应商 URL 自动转换。`getStatus` 只检查 URL、Key 列表、model 是否非空，非空相对路径也可能显示 configured；是否可请求由 HTTP 执行阶段决定。见 [jevClient.js](E:/VCPChat/modules/services/jevClient.js) L316–331、L362–371。
- API endpoint 没有显式 URL/protocol 校验；代理单独通过 `new URL()` 并限 `http:` / `https:`。redirect 禁用（`maxRedirects:0`）。见同文件 L237–275、L365。

**建议**：Mobile 明确要求完整 HTTP(S) JEV endpoint，并把校验归配置/transport owner；不能复用普通聊天 endpoint 自动补全。provider 用于默认值和 headers，保留自定义完整地址能力。状态查询只说明配置完整性，不能伪称网络连通。

## 4. HTTP 请求与错误语义

### 4.1 通用协议

`state` 接受非 null 的 string/object/array；`questions` 须为非空非数组 object。每个 question 须是非数组 object，`instructions` 为非空字符串，`type` 按小写判断属于 `noul/choice/score`。`choice.criteria` 必须是非数组 object 且有 2–255 个选项；`score.criteria` 必须是至少 2 个元素的数组。client 没有进一步校验 criteria 值类型，也不改写传出的 question type。见 [jevClient.js](E:/VCPChat/modules/services/jevClient.js) L151–235。

群聊的 choice 额外加入 `end_conversation`，因此按该 client 的检查，最多能请求 254 位活跃成员；`maxSpeakersPerRound <= 32` 只限制结果入队数，不限制输入 choice 的成员总数。

### 4.2 重试与取消

重试覆盖 HTTP `429/500/502/503/504/529`、`ECONNABORTED/ETIMEDOUT` 以及“有 request、无 response”的错误。401/403 等不在重试集合。若 `Retry-After` 能被 `parseFloat` 解析，使用非负秒值；否则指数退避 `retryBaseDelayMs * 2^attempt`。没有抖动、总 elapsed budget 或 Retry-After 上限，HTTP date 型 Retry-After 不被按日期处理。见 [jevClient.js](E:/VCPChat/modules/services/jevClient.js) L17、L289–313、L360–404。

signal 交给 HTTP request，catch 会在 signal 已 aborted 时结束重试；但退避 `sleep` 本身不可取消，等待结束后才进入下一次尝试。因此“支持 signal”不等于退避期间能立即完成取消。见同文件 L39–40、L366、L389–403。

错误对象包含 `code/status/retryable/provider`。响应摘要最多截取 1000 字符。全局 disabled 错误码为 `JEV_DISABLED`；缺配置为 `JEV_NOT_CONFIGURED`；无 answers 为 `JEV_INVALID_RESPONSE`。见 [globalJevService.js](E:/VCPChat/modules/services/globalJevService.js) L72–85；[jevClient.js](E:/VCPChat/modules/services/jevClient.js) L289–313、L338–346、L373–382。

**建议**：Rust 保持有限重试、每个 decide 固定 Key、错误可定位的行为；退避与请求均监听取消。按真实运行需求明确总耗时或退避上限，不复制取消后仍挂起的实现缺口。

### 4.3 返回值、response.id 与无效结果

client 只检查 `response.data` 是 truthy object，且 `answers` 是 truthy object；没有排除 answers 数组，没有校验具体 question 的答案是否存在。返回整体 response.data，因此通用 IPC 可以透传服务返回的额外字段。

群决策只读取 `answers.speaker_routing`、`answers.continue_discussion` 和 `usage`；**不读取、不校验、不回传 `response.id`**。请求没有本地 request ID 或 idempotency key。会话的陈旧结果判断使用本地 `historyRevision`，不是供应商 response.id。见 [jevClient.js](E:/VCPChat/modules/services/jevClient.js) L349–384；[jevDecisionMode.js](E:/VCPChat/Groupmodules/modes/jevDecisionMode.js) L160–232；[jevGroupSessionOrchestrator.js](E:/VCPChat/Groupmodules/jevGroupSessionOrchestrator.js) L319–350。

**建议**：把本地运行/仲裁身份与可选供应商 ID 分开；供应商 ID 可作错误诊断附加值，但不能变成群会话身份或同步主键。至少验证本次消费的 `speaker_routing.probabilities` 和 `continue_discussion.noul`；明确协议错误与有效零概率，不将缺字段默认为自然收敛。严格程度见 §10 待决定项。

## 5. 群配置与决策输入

### 5.1 `modeSettings.jev`

来源：[jevDecisionMode.js](E:/VCPChat/Groupmodules/modes/jevDecisionMode.js) L3–61。

| 字段 | 默认 | 范围/标准化 |
|---|---|---|
| `navigatorPrompt` | 内置中文导航提示 | 空或空白恢复默认 |
| `speakerThreshold` | 0.16 | 0–1 |
| `continueThreshold` | 0.45 | 0–1 |
| `stopThreshold` | 0.55 | 0–1 |
| `maxSpeakersPerRound` | 3 | 1–32，四舍五入 |
| `maxAutonomousRounds` | 12 | 1–100，四舍五入 |
| `historyWindow` | 12 | 1–200，四舍五入 |
| `continueDebounceMs` | 800 | 0–10000，四舍五入 |
| `fallbackPolicy` | `stop` | 只有精确 `top_speaker` 保留，否则 `stop` |
| `memberStyles` | `{}` | 只保留当前 memberIds 的 string style，逐项 trim；缺失成为空串 |

数字标准化先执行 JavaScript `Number(value)`，因此 null/空串等不一定使用默认：例如 null 阈值成为 0，null historyWindow 被夹到 1。源 object 先 `...source`，未知 JEV 字段保留；memberStyles 则重建为当前成员映射。Mobile 不能将这两种保留策略混为一谈。

群层 `modeSettings` 同时拥有 `sequential/naturerandom/invite_only/jev`，切换模式不删除其他模式设置。顺序模式使用 `modeSettings.sequential.speakerOrder`，旧 `sequentialSpeakerOrder` 作为读取 fallback；标准化去重、过滤已退出成员，把未列出成员追加到末尾。见 [groupchat.js](E:/VCPChat/Groupmodules/groupchat.js) L458–521。

### 5.2 `state` 和 `questions`

`buildDecisionRequest` 产生的结构如下，字段取值见 [jevDecisionMode.js](E:/VCPChat/Groupmodules/modes/jevDecisionMode.js) L64–151：

```text
state.group = { id, name, prompt }
state.recentMessages[] = { id, role, speakerName, agentId, text, timestamp }
state.members[] = {
  agentId, name, triggerStyle, messagesSinceLastSpeech,
  totalSpeechCount, autonomousRunSpeechCount, manuallyQueued
}
state.runtime = { trigger, autonomousRound, forceContinue, latestUserMessageId }
questions.speaker_routing = {
  type: "choice", instructions: navigatorPrompt,
  criteria: { agent_0: "名称：风格", agent_1: "名称：风格", ..., end_conversation: "结束标准" }
}
questions.continue_discussion = { type: "noul", instructions: "继续是否自然、有价值、不重复" }
```

- `recentMessages` 取最新 `historyWindow` 条；文本来自 string、对象 `.text` 或多模态数组中的 text part。没有把图像/音频二进制或附件提取文本额外塞给 JEV。
- `totalSpeechCount` 和 `messagesSinceLastSpeech` 扫描传入的整段 history，`autonomousRunSpeechCount` 取 session 内计数。
- `agent_N` 映射取活跃成员数组顺序，活跃成员来自 group.members 顺序，**没有按 UUID、名称、概率字段键或 speakerOrder 做 canonical sort**。顺序模式的显式排列不会自动改变 JEV 候选顺序。
- state 中没有 Agent system prompt 或其完整模型配置；只有 group prompt、成员名称/风格和上述运行信息。

**资源风险**：桌面先读完整 history，再为每个成员扫描一次；消息条数窗口限制了 recentMessages 数量，没有限制单条文本或整个序列化 body 字节数。Mobile 可复用轻量历史查询，按窗口读取仲裁正文；计数如需全历史语义，可由仓储统计，不必把全历史和附件内容载入 JEV 请求。现有轻量读取入口见 [group_chat_application_service.rs](../../src-tauri/src/vcp_modules/group/group_chat_application_service.rs) L438–446。

## 6. 概率解析与停止算法

来源：[jevDecisionMode.js](E:/VCPChat/Groupmodules/modes/jevDecisionMode.js) L155–232。

1. `normalizeProbability` 先 `Number(value)`；有限数夹到 `[0,1]`，其他归 0。数字字符串、boolean 会被接受，null 变 0；没有检查概率和等于 1。
2. 只枚举本地 `optionToAgentId`，忽略未知 option 的概率。每个成员缺失概率变 0。按概率降序排序；并列保持原候选顺序。
3. 普通仲裁先检查 `continueProbability < continueThreshold`；成立就停止。否则，当 `endProbability >= stopThreshold` 且 `endProbability >= highestSpeakerProbability` 时停止。相等边界允许结束选项胜出。
4. 未停止时选概率 `>= speakerThreshold` 的成员，截取前 `maxSpeakersPerRound` 位。
5. `forceContinue` 跳过两个停止判断，且没有达阈成员时选第一名；它不强制只选一个，可能仍选多位。
6. 普通仲裁没有 stopReason、没有达阈成员且 fallbackPolicy 为 `top_speaker` 时，也选第一名。
7. 仍无成员时停止原因为 `no_speaker_above_threshold`。

`routingAnswer.choice` 仅作为 `rawChoice` 返回，**不驱动路由**；confidence 也不影响选人或停止。`fallbackPolicy` 不处理网络失败、disabled、缺配置或协议异常；这些错误退出 session，不能描述成“JEV 不可用时自动回退最高分/顺序模式”。

已对生产纯函数做本地只读调用，确认以下执行结果；这些是当前实现观察，不是产品目标：

| 输入 | 当前结果 |
|---|---|
| `answers = {}`，普通仲裁 | 所有概率为 0，`continue_probability_below_threshold` |
| `answers = {}`，forceContinue | 选第一位候选，即使其概率为 0 |
| 两个成员同为 0.5，response 概率键按反序出现 | 仍按原成员数组先后入队 |
| `agent_0=true, agent_1=null, noul="0.8"` | 分别解析为 1、0、0.8 |
| response 含 `id` | 群决策结果不带该 ID |

## 7. 会话触发、排队与取消

来源：[jevGroupSessionOrchestrator.js](E:/VCPChat/Groupmodules/jevGroupSessionOrchestrator.js)；生产调用见 [groupchat.js](E:/VCPChat/Groupmodules/groupchat.js) L878–897、L2479–2582，以及 [groupChatHandlers.js](E:/VCPChat/modules/ipc/groupChatHandlers.js) L201–274。

| 操作 | 源码行为 |
|---|---|
| 用户发言 | 消息先落盘；historyRevision++，更新 latestUserMessageId，dirty=true。若已有 run，只更新当前 session，不新建 run |
| 用户 `@成员` | 按成员名称大小写无关的字符串包含判断；将非当前成员移到 queue 前端，去掉 manualQueue 同成员 |
| 运行中普通新消息 | 不取消当前回复，也不清空已排队成员；已有队列消耗完才重新仲裁 |
| 手动点名 | 当前正在发言者忽略；去重后，运行中 unshift 到 manualQueue，空闲时插到 queue 并启动 |
| 随机开场 | 空 queue 时从启动时活跃成员随机挑一位先发言，随后仲裁 |
| 继续 | 当前运行则拒绝；按 continueDebounceMs 限流；启动时 forceContinue=true |
| 生成 | queue 和 manualQueue 串行执行；手动队列优先，每次只运行一个 Agent |
| 仲裁 | 队列空后检查最大轮数，autonomousRound++，捕获 historyRevision，再读 history、发请求 |
| 过期仲裁 | 等待期间 historyRevision 改变则丢弃结果，继续循环；不是取消远程请求 |
| 群队列停止 | 清空待发队列；speaking 时保留当前回复并设置 stopRequested，待当前回复结束退出；arbitrating 时 abort |
| 单条回复停止 | `interruptGroupRequest(messageId)` 中断当前请求并尝试远程 interrupt，不取消整轮剩余成员 |
| 终态 | 清空 queue/manualQueue、currentAgentId、dirty、forceContinue、abortController、runPromise，发 idle 快照 |

会话状态事件有 `group_queue_state`、`group_queue_updated`、`group_queue_stopped`、`jev_arbitration_started`、`jev_arbitration_result`。快照包含 running/status/currentAgentId/queue/manualQueue/autonomousRound/historyRevision/dirty/stopReason/stopRequested；不是持久化群配置。

JEV 失败会在 `_startRun.catch` 发停止事件，然后 finally 回 idle。Agent 生成错误另由邀请链处理：多数错误写为 assistant 错误消息后被吞掉，orchestrator 的 runAgent resolve 后仍增加 speechCounts/historyRevision。不能把桌面 `Promise<void>` resolve 等同为“成功生成且持久化”；Mobile 已有明确的 speaker outcome，应继续使用。证据：[groupchat.js](E:/VCPChat/Groupmodules/groupchat.js) L2142–2154；[jevGroupSessionOrchestrator.js](E:/VCPChat/Groupmodules/jevGroupSessionOrchestrator.js) L286–300。

## 8. 不应照搬的生命周期缺口

以下为生产源码推导出的风险，未修改 VCPChat 非同步代码，也未声称已获用户确认的目标行为。

| 风险 | 因果链与证据 | Mobile 建议 |
|---|---|---|
| 轮次累计到话题进程寿命 | autonomousRound/speechCounts 只在 `_createSession` 初始化；`_startRun/finally` 不重置；达到最大轮次后继续和新消息仍会在仲裁前退出。L43–64、L209–237、L311–314 | 区分 session identity 与一次 autonomous run 的预算；每次明确的新 run 重置 run 计数 |
| 强制继续被过期响应消耗 | `forceContinue=false` 在 revision 校验之前执行；新用户消息令结果过期后，下一次仲裁已失去强制标记。L343–350 | 仅接纳有效仲裁时消费 forceContinue |
| 仲裁中的手动点名可能丢失 | enqueueAgent 改 manualQueue 不增加 historyRevision；decision.shouldStop 在合并 manualQueue 之前返回，finally 清空手动队列。L143–162、L353–393 | 将用户明确点名作为可独立消费的命令；仲裁停止不覆盖后到的手动请求 |
| 停止等待期间的新消息无后续触发 | speaking 停止设置 stopRequested；若当前回复完成前又收到用户消息，调用者见 running 不启动新 run；当前回复完成因 stopRequested 返回，finally 清队列。L116–140、L301–307、L397–420；groupchat.js L884–896 | 明确接纳时序：已接纳的新请求要么形成下一 run，要么返回明确不可自动触发结果 |
| 成员变化不一致 | `_runLoop` 一次性捕获 groupConfig/activeMembers/memberMap；enqueue 外层按最新 groupConfig 校验，因此新加入成员可被接纳却被旧 memberMap 跳过。L244–251、L279–281；groupchat.js L2511–2520 | 明确以 run 快照还是下一仲裁刷新为准，提交队列前统一校验 |
| 已移除成员仍可能发言 | 原队列保留旧成员；runAgent 走邀请函数后重新读 group/agent，但该函数未校验 invitedAgentId 仍在 group.members。groupchat.js L311–319、L1547–1574 | 发言实际开始前按选定策略校验成员有效性，避免新旧快照混用 |
| 内存 session 永不释放 | getState 也 `_getOrCreate`，sessions Map 无删除入口；已结束话题保留状态。L40、L67–72、L112–113 | 空闲会话的保留范围由恢复消费者决定，话题删除时释放；不持有永远增长的孤立状态 |
| 队列停止与硬取消不能混为一谈 | Desktop 群停止保留当前回复；Mobile 当前群取消会取消当前子请求和对应语音 | 保留独立 hard cancel，新增/明确 graceful queue stop 语义 |

## 9. Mobile 适配 owner 与分步建议

### 9.1 可以复用的生产能力

- [group_chat_application_service.rs](../../src-tauri/src/vcp_modules/group/group_chat_application_service.rs) L84–104、L201–287：串行 speaker 执行、ActiveRequestLease、现有 VCP transport、durable finalizer；只有落盘后才将结果加入下一棒上下文。
- 同文件 L180–208：每条成员回复沿现有 MessageKey/TopicKey，并从群轮复制 `originUserMessageId`。不要绕过该关联，否则语音/TTS 和恢复会失去真实归属。
- [vcp_client.rs](../../src-tauri/src/vcp_modules/infra/vcp_client.rs) L952–1129：已有 ActiveGroupTurns 独占、输出重新绑定、群轮活动事件、取消 token。应扩展或明确调整此 owner，不另建与它争抢相同 topic 的独立群执行器。
- [voice_submission.rs](../../src-tauri/src/vcp_modules/chat/voice_submission.rs) L43–72、L97–138：语音提交先注册真实群轮再持久化，WebView 可以缺席。持续群聊必须从同一入口接纳语音，而不是只修 Vue 发送按钮。
- [group_chat_application_service.rs](../../src-tauri/src/vcp_modules/group/group_chat_application_service.rs) L438–446：轻量纯文本历史；可用于仲裁窗口，不必加载 UI 渲染块和大体积附件提取正文。

### 9.2 必须补齐的内容

1. **配置与 DTO**：当前 [group_types.rs](../../src-tauri/src/vcp_modules/group/group_types.rs) L56–95 没有 modeSettings；当前 mode=jev 只会落到 `mode_not_implemented`，见 [group_chat_application_service.rs](../../src-tauri/src/vcp_modules/group/group_chat_application_service.rs) L449–473。先确保存储/同步/编辑的 group modeSettings 是一份权威配置，再加实际执行。
2. **独立 JEV transport + 决策逻辑**：Rust 持有连接快照、Key 轮询、有限重试、取消、响应解析；群领域负责 state、候选顺序和停止算法。通用 `score` 支持只有在真实消费方需要时才扩大对外接口，不为桌面通用 IPC 的存在增加无人调用的手机 command。
3. **持续 run 与控制入口**：持有一次真实群轮租约，接纳用户消息、点名、随机开场、继续、优雅停止；运行中接纳不能重复调用 try_acquire。当前重复 acquire 会报 `Group turn is already active`，见 [vcp_client.rs](../../src-tauri/src/vcp_modules/infra/vcp_client.rs) L973–1001。
4. **停止与恢复**：当前 [vcp_client.rs](../../src-tauri/src/vcp_modules/infra/vcp_client.rs) L1135–1172 的 interruptGroupTurn 会设置 active_generations.stop_requested、取消群轮，并停止对应语音，不能全局替换成 soft stop。队列控制、当前请求硬取消、语音插话/挂断的取消分别接到真实 owner。
5. **活动状态与后台租约**：现有 ForegroundLease 覆盖单个 speaker 请求，见 [group_chat_application_service.rs](../../src-tauri/src/vcp_modules/group/group_chat_application_service.rs) L218–219；JEV 仲裁与退避发生于成员间隙，需要确定 run 生命周期的后台资源归属和释放。共享业务保持在 Rust，Android/iOS 只承担各自已有原生资源能力。
6. **消息版本与引用**：仲裁结果绑定本地 generation/revision；新用户消息落盘后通知 session，生成前读取选定最新历史。不能只更新 Vue 缓存，也不能在一次长 run 中把后续所有回复永久关联到最早一条用户消息而忽略语音消费者。

实施顺序建议：先完成群配置/同步无损往返与顺序模式，再交付独立服务配置和 JEV client，最后接入持续会话与 UI 控制；每一步都沿真实调用链闭环。核心改动按仓库规则运行 `pnpm check`。此分步建议由总方案综合，不构成实现完成声明。

## 10. 待决定项与推荐取向

只列影响产品行为、不能靠机械翻译解决的选项。字段命名、共享 helper 位置和普通 Rust 锁实现属于工程决策，不额外请用户批准。

| 待决定 | 推荐取向 | 其他可选语义及代价 |
|---|---|---|
| 最大自主轮数的计量周期 | 按一次明确启动/继续/新消息触发的自主 run 重置 | 按话题累计会复现“达到上限后再也不能继续”，需要另加显式清预算入口 |
| 运行中成员/模式设置变化 | 当前正在生成的回复保持既定快照；下一次仲裁/发言按明确的新 revision 刷新并校验成员 | 整个 run 冻结配置也可行，但新成员点名须明确拒绝或延后，不能显示接纳后丢失 |
| 无效 JEV response | 报协议错误并停止仲裁，不伪装自然结束、不凭空挑成员 | 完全复刻 JS 宽松 coercion 可接受 null/boolean/数字字符串，但空 answers + continue 会产生无依据发言 |
| 群队列停止入口 | 对齐新版 VChat：保留当前回复，停止后续队列；保留独立单条/语音硬取消 | 若保留 Mobile 原来一个按钮立即停止整轮，须明确它与新增队列停止是不同动作 |

服务提供商、默认阈值及 Key 轮询本身有明确源码参考，可作为对齐默认值，不应再扩大为冗长审批表。JEV 服务凭据建议为本机配置，群 DTO 只承载模式设置；若另有跨设备凭据同步目标，应由同步方案独立裁定。

## 11. 后续有价值的验证

本次仅执行生产纯函数的只读本地 probe，结果见 §6；没有写测试来反向规定产品。实施后优先验证真正存在不确定性的边界：

- 使用受控 HTTP 服务观察真实 body/headers/完整 endpoint；证明不会自动套聊天路径，Key 每请求轮询、重试固定 Key，401 不重试、429/5xx 按规则重试，取消能中止退避。
- 让服务返回缺 answers、空 answers、错类型、null、越界概率、未知 option、并列概率，分别观察协议错误和有效决策，避免“恰好默认 0”掩盖无效响应。
- 仲裁未完成时落盘新用户消息、插入点名、删除成员、优雅停止，确认旧结果不会覆盖用户的新命令。
- 连续完成最大轮数后点击继续或发送新消息，确认 run 预算按最终决定重新生效。
- 在当前成员流式输出/工具调用期间停止队列，确认当前 durable finalizer 正常收尾且下一位未启动；硬取消仍使用原请求终态链。
- WebView 重建或重新绑定期间，持续群聊状态由 Rust 恢复；不重放 start/continue，不重复写用户消息；语音 originUserMessageId 的归属保持可核对。
- Android 后台仲裁/退避/成员切换需要对应设备证据；iOS 按已有 G1/G2 与源码审计边界取得证据，不新增 iOS 实机测试体系。host 检查不能替代上述运行证据。
