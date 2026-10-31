# 06 — Android 中转、写入看门狗与全文保留审查

审查日期：2026-10-31。源码基线：`a771315ec4b5e71652621cf7343bce2344031479`。范围：原报告维度二第 6 项，以及维度四对它提出的“Socket 空闲超时”替换建议。本报告只审查，不修改生产代码或现有测试。

## 裁决与紧迫性

**每个输出帧确实创建并取消一个写入超时任务；但原报告漏掉了已经启用的 `removeOnCancelPolicy = true`，未证明定时任务在延迟队列堆积，更未证明它造成显著 ART GC 停顿。全文保留也确实存在，但它有明确的恢复和终态交接消费者，并有正常释放路径，不能归为无用途的内存冗余。**

| 子结论 | 核实结果 | 当前紧迫性 |
| --- | --- | --- |
| 写帧前 `schedule`，完成后 `cancel(false)` | 成立，发生在单独 writer 线程；与实际输出帧数相关 | **P3**：微优化/测量项，暂无高优先级改造依据 |
| 取消任务在延迟堆中持续积累，导致严重 GC | 当前代码存在直接反证；仍会有短命对象，但 GC 频率、停顿和对吞吐的贡献均未测量 | 不采纳原“中高”严重性 |
| `fullContent` 保存完整正文且无单流字节上限 | 成立；它服务断连接管、主进程重建和离线完成交接 | **P2**：容量验证和方案评估项；不能直接删除 |
| 正常流结束后全文一直泄漏 | 不成立；ACK、discard 或成功离线发布均移除 session | 不构成已证泄漏 |
| 完成结果持续无法落盘时的总保留容量 | 条件性风险成立：这些完成态不计入“8 个正在接收流”上限 | **P2**：应覆盖故障容量场景；若真实规模复现内存耗尽，可升 **P1** |
| 长效 Socket 空闲超时可以等价替代当前 watchdog | 不成立：空闲与单次在途写阻塞是不同条件 | 不应直接执行该建议 |

这里的 P1 表示近期优先处理，不自动等于发布阻断；P2 为常规优化/验证，P3 为低成本微优化或观察。没有足够证据将本项判为 P0/P1 的普遍生产故障。

## 1. 实际链路与计数单位

生产链路为：

```text
OkHttp EventSource.onEvent
  → acceptUpstreamEvent：解析/筛选上游数据
  → fullContent.append(content)
  → 当前连接 enqueueFrames：一个 producer 占一个队列槽
  → 单 writer 线程按需产生 ≤ 16K UTF-16 code units 的 content 帧
  → 每帧 schedule(5s) → writeFrame + flush → cancel
  → Rust 解码事件 → pending_chunk → Aurora 渲染时钟
```

- 空白事件、无 `choices[0]` 的事件，以及没有 content/finish reason 的事件不会进入输出写入；`[DONE]` 进入终态流程。不能把所有 SSE 事件、模型 token、输出帧、前端渲染帧视为同一个计数单位。[上游事件处理 L538–584][coordinator-events]
- 一个上游 content 可以拆成多个输出帧，单个大 content 的 producer 仍只占一个 writer 队列槽；快照也通过 producer 分段生成。[分帧规则 L7–15][protocol]、[惰性 producer L59–87][connection-producer]
- Rust 会把 data/snapshot chunk 放入自己的 `pending_chunk`，由渲染时钟或快照边界刷新。Android watchdog 频率不等于 AST 更新频率。[Rust 读取/合并 L2251–2338][rust-read]
- 原生助手在独立 `:helper` 进程内。[Manifest L37–44][manifest] 因此助手的 ART GC 不是 WebView 主线程直接执行的 GC；资源竞争或流数据延迟的间接影响仍可能存在，但须测量。

## 2. Watchdog：短命分配属实，队列长期滞留不成立

### 2.1 已有保护不能省略

`ClientConnection` 已有以下实现事实：

1. watchdog 是共享的单线程 `ScheduledThreadPoolExecutor`，在 **L23 显式设置 `removeOnCancelPolicy = true`**。[执行器定义 L17–37][connection-executors]
2. 每个连接的 writer 只有一个工作线程，等待队列为 `ArrayBlockingQueue(128)`；拒绝策略是 `AbortPolicy`，捕获拒绝后关闭连接，不继续无界积压任务。[执行器定义][connection-executors]、[入队/溢出 L39–56][connection-enqueue]
3. 计时器在任务实际开始写帧前创建，而不是在每次入队时预先创建。排队的 128 个 producer 不各持有一个运行中的 watchdog。[惰性 producer L64–76][connection-producer]
4. 写完成后取消；超时时 `close()` 先关 socket，再关原始输出流，避免先 flush 缓冲流而等待阻塞 writer。[关闭顺序 L127–134][connection-close]

Android 官方 API 明确说明，该 policy 为 true 时，取消任务会立即从执行器工作队列移除；该 API 从 21 起可用，覆盖本项目 minSdk 26。[Android `ScheduledThreadPoolExecutor` 文档](https://developer.android.com/reference/java/util/concurrent/ScheduledThreadPoolExecutor#setRemoveOnCancelPolicy(boolean))

据此可以否定“取消任务继续留在 5 秒延迟队列中累计”的推断。执行器仍需创建任务、维护队列并取消任务，短命对象也仍需回收；**立即移除不是零分配，也不是已经证明没有性能成本**。但本次未采集 allocation rate、执行器 CPU、GC pause 或流转发延迟，不能把这些操作直接升级为严重 GC 震荡。

串行 writer 意味着，一个连接正常只为当前正在写的那一帧挂一个超时任务；总量随并发正在写的连接数变化，而不是按“输出频率 × 5 秒”累积已取消任务。这个描述不应扩展为“系统最多永远只有 8 个 watchdog”：8 限制的是未完成接收会话，不是全部控制、接管和完成态连接。

### 2.2 不应换成空闲超时

当前超时覆盖的是 **`writeFrame`（包括 UTF-8 编码、输出与 flush）从开始到结束持续超过 5 秒**。写前没有任务，写后取消任务，因此上游几秒未产出任何文字并不触发这个超时。

若改为“距最后一次网络活动超过某时长就关闭”，将同时改变两件事：长时间正常等待模型输出可能被关闭，而单次写阻塞也不再一定具有原本的截止时间。若所谓替换仅指 `socket.soTimeout`，它约束的是 InputStream 的阻塞读取，不能替代输出写入截止时间。[Java `Socket.setSoTimeout` 官方说明](https://docs.oracle.com/en/java/javase/17/docs/api/java.base/java/net/Socket.html#setSoTimeout(int))

当前接收连接已经在读取首条控制命令时使用 `soTimeout = 5000`，start/resume 建立后改为 `0` 并等待断开，说明这两个超时所有者和用途本就不同。[控制命令与连接生命周期 L194–231][coordinator-socket]

若未来测量证明 schedule/cancel 有显著成本，可研究带“当前写开始时间 + 写操作世代”的共用定期检查器；但它必须保留在途写截止、无写时不超时、旧检查不关闭新写、关闭幂等等语义，并承担检查粒度带来的额外延迟。它是需要验证的改造方案，不能作为本次审计已经确定更优的替换。

## 3. 全文保留：有成本、有消费者、有回收边界

### 3.1 确实没有本地全文长度上限

`StreamSession.fullContent` 是 `StringBuilder`，每次收到正文后 append；未见这里按正文长度拒绝、截断或转存。[字段 L9–20][session]、[append L561–575][coordinator-events]

目前不能把下面三个现存边界误当成正文内存硬上限：

| 边界 | 实际约束对象 | 不保证什么 |
| --- | --- | --- |
| `MAX_ACTIVE_SESSIONS = 8` | `!isCompleted` 的正在接收会话 | 不限制每条正文长度；不限制保留中的完成态数量 |
| `ArrayBlockingQueue(128)` | 待执行 writer 任务/producer 数量 | 不提供总字节上限；一个 producer 可捕获较大的上游 content |
| `CONTENT_FRAME_CHARS = 16 * 1024` | 每个输出 content 片段，且避免切断代理对 | 不限制 `fullContent`，也不限制上游事件原始字符串大小 |

证据：[接收准入 L306–318][coordinator-admission]、[writer 配置][connection-executors]、[分帧规则][protocol]。agent 和 group 请求确实向上游传 `max_tokens`，但这是上游生成配置，不是 helper 接收正文的本地字节检查。[agent 载荷 L203–208][agent-payload]、[group 载荷 L154–159][group-payload]

因此，长回复、多并发的 helper 内存占用随保留正文总量增长，是确实存在的容量成本。这里的生产调用是 `StringBuilder.append`，不是每个 chunk 主动重建全文字符串；不应把“保留 N 字符”写成“每帧完整复制 N 字符”。具体堆字节数还取决于 ART 字符串实现、容量余量、上游/队列暂存和编码临时对象，本次不编造固定倍率。

### 3.2 这份全文不是无消费者镜像

| 消费者 | 实际行为与必要性 |
| --- | --- |
| `handleResumeStream` | 在同一 session monitor 下替换消费者并捕获正文边界；writer 按片段读取此前全文、发送 snapshot，再顺序发送后续 live data。删除此前正文会使接管丢失前缀。[L421–450][coordinator-resume] |
| `CompletedMailbox.publish` | 写入终态 metadata 和完整正文，flush/fsync 后原子移动成 `.completed` 文件；正文按片段输出，避免一次性再构建全量 JSON 列表。[L19–50][mailbox] |
| 完成通知 | 从正文生成完成预览。[L172–182][notifications] |
| Rust 终态持久化 owner | 在正式提交后发 `ack_committed`，不是在 WebView 收到文字时即释放 helper 结果。[L670–674、748–774][rust-ack] |

原报告的“JVM 层堆积全文，造成内存冗余”作为跨进程重复保留的描述有物理依据，但不能据此推导“删除全文即为无损优化”。恢复能力正依赖于这份尚未完成权威提交的正文。

### 3.3 正常释放与失败保留必须分别判断

- ACK/discard 校验 generation 后调用 `removeSession`，并拆掉网络源和连接。[控制路径 L499–518][coordinator-control]
- 流完成后，若仍有连接，等待 5 秒 ACK 窗口；若已断连，尝试立即离线发布。发布成功后移除 session、关闭连接。[完成路径 L587–648][coordinator-complete]、[发布路径 L651–690][coordinator-publish]
- 发布失败时不删唯一正文；2 秒、4 秒各自动重试一次，预算耗尽后停止自动重试，仍保留 session 供接管。[发布路径 L678–687][coordinator-publish]
- `removeSession` 同时取消 handoff job；service 销毁也会清空 session 集合并关闭资源。[移除 L694–702][coordinator-remove]、[销毁 L88–120][coordinator-destroy]

**更值得跟踪的条件风险**是：磁盘发布持续失败、Rust 又没有提交 ACK/discard 时，完成态正文保留在集合中；准入只计未完成会话，因此后续继续生成可以使保留完成态超过 8。它与“不丢失尚未持久化回复”的语义有关，不是普通成功链路遗忘释放。

此外，正常 ACK/发布释放的是 session 所有权；仍在退出的 writer/回调短时间持有引用时，堆回收不会和 map remove 完全同步。这里没有取得长期引用泄漏或实际 OOM 的证据。

## 4. 应如何推进

1. **保持现有写阻塞超时和正文恢复语义。** 不按原报告直接移除 watchdog、改为普通空闲超时，或删掉 `fullContent`。
2. **容量验证优先于定时器微优化。** 覆盖 1/8 个接收会话、产品允许的长回复、慢消费者/接管，以及完成态持续发布失败的积累；区分接收正文、writer 暂存、完成保留正文三部分。
3. **若容量实测超预算，讨论保真存储方案。** 例如把全文所有者的存储介质改为可顺序读取的文件或分段存储，而不是自动截断。须同时设计写盘失败、恢复快照、generation/ACK 原子交接；这会扩大生命周期复杂度，不能只为消除一个 StringBuilder 引入平行状态机。
4. **优先级上调条件明确化。** 若可复现 helper 因长回复/并发/失败积累被杀，或 watchdog 调度占用明显 CPU 并造成可归因的转发停顿，升为 P1 并按实测主因处理。未取得这些证据前，不将“理论可增长”写成“普遍极高开销”。

本次没有新增长度限制、自动降级或恢复兜底；若后续提出这些行为变更，需按仓库规则先讨论产品语义。

## 5. 证据范围与必要验证

本次完成了生产代码调用链、所有权、准入、背压、失败和释放路径审查，并核对官方执行器/Socket API。读取了现有 `slowClientWriterUsesABoundedQueue`、`publicationFailureRetainsTheOnlyBodyForResume`、`exhaustedRetriesRetainBodyWithoutAnotherTimerOrWakeLock` 等测试以定位已有覆盖；**没有执行这些测试，也没有用它们替代生产代码定义行为**。[现有测试 L134–146、483–544][existing-tests]

没有运行 Android 实机、ART profiler、GC trace 或端到端吞吐实验；没有运行 `pnpm check`，因为本项仅新增审查文档，没有生产/跨层逻辑修改。没有以 host JVM 的 GC 数字推断 Android 表现。

后续验证应至少能够推翻以下假设：

- 正常高速输出时，取消后的 watchdog 队列长度不随累计帧数增长；同时测短命分配速率与 CPU，而不只看队列长度。
- 慢消费者触发有界拒绝或写截止后，恢复得到的全文无缺失、无乱序；正常上游长停顿不会被错误中断。
- ACK/发布成功后，helper 正文 retained size 能回落；发布失败且不 ACK 时，报告实际保留容量和会话数量变化。
- 对具名 Android 设备同时记录 helper 进程和主进程指标，才评估 GC/资源竞争是否影响用户看到的流式延迟。任何设备操作遵守仓库 debug 工具和包名边界。

[connection-executors]: ../../src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/sse/ClientConnection.kt#L17
[connection-enqueue]: ../../src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/sse/ClientConnection.kt#L39
[connection-producer]: ../../src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/sse/ClientConnection.kt#L59
[connection-close]: ../../src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/sse/ClientConnection.kt#L127
[protocol]: ../../src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/sse/SseProtocol.kt#L7
[session]: ../../src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/sse/StreamSession.kt#L9
[coordinator-destroy]: ../../src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/sse/SseSessionCoordinator.kt#L88
[coordinator-socket]: ../../src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/sse/SseSessionCoordinator.kt#L194
[coordinator-admission]: ../../src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/sse/SseSessionCoordinator.kt#L306
[coordinator-resume]: ../../src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/sse/SseSessionCoordinator.kt#L421
[coordinator-control]: ../../src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/sse/SseSessionCoordinator.kt#L499
[coordinator-events]: ../../src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/sse/SseSessionCoordinator.kt#L538
[coordinator-complete]: ../../src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/sse/SseSessionCoordinator.kt#L587
[coordinator-publish]: ../../src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/sse/SseSessionCoordinator.kt#L651
[coordinator-remove]: ../../src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/sse/SseSessionCoordinator.kt#L694
[mailbox]: ../../src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/sse/CompletedMailbox.kt#L19
[notifications]: ../../src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/sse/SseNotifications.kt#L172
[manifest]: ../../src-tauri/plugins/vcp-mobile/android/src/main/AndroidManifest.xml#L37
[rust-read]: ../../src-tauri/src/vcp_modules/infra/vcp_client.rs#L2251
[rust-ack]: ../../src-tauri/src/vcp_modules/infra/vcp_client.rs#L670
[agent-payload]: ../../src-tauri/src/vcp_modules/agent/agent_chat_application_service.rs#L203
[group-payload]: ../../src-tauri/src/vcp_modules/group/group_chat_application_service.rs#L154
[existing-tests]: ../../src-tauri/plugins/vcp-mobile/android/src/test/java/com/vcp/mobile/service/SseProxyConnectionOwnershipTest.kt#L134
