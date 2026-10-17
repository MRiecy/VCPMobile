# SSE 统一接管与操作边界重构

> 2026-10-17：按用户逐项审议完成本轮重构。本文覆盖共享聊天生命周期与 Android helper 逻辑修复，是 P5-2 的基础工作；不表示 iOS 后台执行机制已选定或 P5-1/P5-3/P5-4 已完成。实现事实以生产代码为准，实际验证见下文。

## 本轮用户决定

1. 对会移除旧消息的用户操作，前端统一检测受影响会话或所有者范围中的生成活动，禁用对应入口并在执行前复核。覆盖气泡删除、编辑重发截断、重新生成、删除话题、Agent 和群组。不为这些入口建立复杂删除竞争与回滚框架；现有外部删除和终态提交仍按真实持久状态幂等收尾。
2. 主动停止保留正文原样，以消息状态表达停止，不追加错误文字，为后续语音中断留下相同的取消语义。
3. Android helper 完成后的提交 ACK 宽限为 **5 秒**。超时先可靠发布离线结果，再断开和释放会话；不能把 TCP 写成功视为数据库提交。
4. 使用 `reconnect_chat_streams` 表达一次重新连接聊天输出的意图。Rust 协调运行中请求、群聊换棒、helper 与离线结果；Vue 不再先查询再逐条调用恢复。
5. 重新连接命令不等待全部生成完成。持续恢复由请求 owner 持有，快照、后续增量和持久终态经 Channel 送达；生产者负责快照先于增量。
6. 前端连接令牌只保护连接回调；请求 owner 负责生成、取消和恢复；finalizer 负责持久终态。不引入第二份全文缓存或通用事件总线。
7. 本轮允许至多六位子代理协作，并授权完成后提交；未授权 push 或发布。

其余沿用本次逻辑缺陷审议：恢复失败保留正文并结束空转提示；群聊停止取消当前发言及后续接力；划卡后已有 Android 流继续，完成交接后退出；通知跳过思考链展示正文开头；移除无消费者的 `sendToVCP` IPC 入口。

## 三方审查与工程边界

- Melchior：消费者身份与请求 attempt 分离；接管不重发上游请求；停止意图需要在跨进程恢复时仍可辨认；终态只服从持久提交。
- Balthasar：操作屏蔽按受影响范围，包含请求准备和群聊换棒空窗；已打开菜单与确认框在执行时仍使用同一判断；其他会话不受全局锁影响。
- Casper：复用现有请求租约、session 锁与 finalizer。helper 的准入与停止互斥，ACK/离线发布/discard 按 generation 裁定；网络锁和有界交接锁分别释放。

本轮不选择新的单流正文截断额度、不调整尚未审议的内存预算，不选择 iOS 静音音频或 continued task 的生产机制。Android 内存与执行资源优化中，只纳入与本次逻辑闭环直接相关且不改变功能范围的改动。

## 实际接入契约

- `reconnect_chat_streams({ connectionId, channel? })`：首次登记 Channel；同一 WebView 的后续对账只传连接 ID。锁定 Tauri 2.12.1 每次将 JS Channel 反序列化为 Rust Channel 时会创建新的消息序号计数，因此不能重复传同一 JS Channel。可选 `JavaScriptChannelId` 在首次登记时转换为 Channel，后续复用 Rust 原实例。
- Rust 的当前界面连接不保存正文。注册核对实际 WebView 实例；同 label 的旧实例不能覆盖新连接，异步扫描也不能重新绑定过期消费者。
- 收据仅含 `generations`、`groupTurns`、`activeRequests` 活动元数据。Channel 负责消息状态/正文，以及群轮和请求 lease 的活动事件；前端按消息、turn UUID 或 request attempt UUID 处理迟到事件和收据。
- 单聊 request lease 在读取设置和准备上下文之前建立，不提前创建数据库消息。准备失败释放真实活动，WebView 重建也能知道尚在准备；`request_activity` 不创建消息气泡。
- 显式多人 `@` 邀约使用同一次 `agentIds` 调用，在 Rust 同一群轮内顺序发言。停止和未确认收尾会阻止后续邀约；已持久的普通成员错误保持原有可继续策略。
- 已经通过 Channel 告知的暂时失败，应用命令返回 `recovery_pending`/`stopping` 收据；未产生对应事件的前置失败仍返回错误，避免重复或漏掉用户通知。
- `0102_stream_stop_intent.sql` 为 pending 生成记录增加停止意图与必要的停止 partial。partial 只在停止无法确认时暂存，不按每个 SSE chunk 写库。

## 实施路径

| 路径与 owner | 输入与输出 | 取消、异常、资源释放 | 验证问题 |
|---|---|---|---|
| Vue `chatStreamStore` 与聊天操作策略 | 当前会话/owner、准备期操作、Rust 消息和群轮事件 → 同一冲突判断与 UI 状态 | 释放本地操作占用；迟到连接回调不覆盖当前状态；待重试保留正文 | 首帧前、换棒、确认框打开后状态变化、其他会话不受影响 |
| Rust `vcp_client` | 单次重连 Channel → 既有请求接管或后台恢复工作安排 | 不重 POST；停止意图跨恢复保留；旧连接不能抢回新消费者 | 无新 chunk 时接管、并发换界面、重复重连、停止失败与后续对账 |
| 单聊/群聊应用服务 | 用户生成/整轮停止 → 请求租约与持久结果 | 群轮取消传至当前发言；准备阶段检查取消；前台资源持有至提交或明确失败收尾 | 当前发言未显示、换棒时停止、提交期间停止、租约释放顺序 |
| `message_service` finalizer | 生成正文/停止原因 → 持久正文和终态或已删除结果 | 不污染停止正文；已终态和已删除幂等；真实数据库失败仍报告 | 取消正文精确保留、重复终态、已删除结果不进入群聊上下文 |
| Android `SseProxyService` | 上游流、ACK/cancel/discard/resume → 在线流或离线结果 | 5 秒 ACK、有界 CPU 交接、原子端口发布、准入与停止互斥 | ACK 与超时竞争、落盘失败、旧 generation、终态后接管、长思考链摘要 |

## 验证与交接

完成时运行 `pnpm check`、对应前端状态/交互验证、Rust 请求与持久终态验证，以及 Android 插件编译和 helper JVM 验证。只登记真实执行结果，不把 host 检查或 JVM 模拟等同于手机系统冻结、触控或长期后台运行证据。

iOS 后续继续按 P5-1 核对严格 SSE 目标与真实系统执行条件，并按 P5-3 接入原生资源。共享 owner 与终态收尾完成不能替代 Apple 编译或后台连续性结论；不建设 iOS 实机测试体系。

## 实施结果与实际证据（2026-10-17）

共享接管、操作屏蔽、停止正文、群聊取消与批量邀约、持久终态幂等、Android 5 秒 ACK 交接及相关生命周期修复已接入。独立审查发现的 Tauri 通道计数、迟到群轮活动、已删除消息的孤儿离线文件、单聊准备期跨 WebView 接管、全局 Channel 的重复通知等接缝均已收口；新界面即使复用了数值 callback ID，也按新的连接身份替换消费者。

| 实际执行 | 结果与证据范围 |
|---|---|
| `pnpm check` | 最终版本 TypeScript 与 host Rust 检查通过 |
| `pnpm test:run` | 76 个文件、633 项通过；包含操作禁用、确认框状态变化、连接登记、迟到回执、准备期与群轮活动归约 |
| `cargo test --locked --lib vcp_client` | 29 项通过；包括实际本地 HTTP SSE 接管、取消、离线提交、旧 WebView、准备 owner 与连接版本 |
| `cargo test --locked --lib stream_lifecycle_tests` | 9 项通过；覆盖真实 SQLite 终态、取消正文、删除幂等与失败回滚 |
| `cargo test --locked --lib group_turn_tests` | 1 项通过；真实本地 HTTP SSE 与 SQLite 验证停止首位后没有第二个 POST |
| `cargo test --locked --lib db_manager::tests` | 12 项通过；新装数据库实际执行 0102，而不是只修改测试夹具 |
| `cargo check --locked --target aarch64-linux-android` | 通过；临时使用已有 NDK 29.0.14206865 的 Android 26 clang/clang++/llvm-ar，未改仓库构建配置 |
| Android `:tauri-plugin-vcp-mobile:compileDebugKotlin` | 通过 |
| Android 定向 `:tauri-plugin-vcp-mobile:testDebugUnitTest` | `SseProxyConnectionOwnershipTest` 7 项、`SseProxyProtocolTest` 12 项通过；包括真实 5 秒 timer、ACK/落盘、失败保留及通知摘要 |
| `git diff --check` | 通过 |

本轮未取得手机 Freezer/长期后台运行证据，未执行 Apple 构建或 iOS 实机验证。不将源码、host、Android 目标编译和 JVM 结果互相替代。后续 P5-1 继续核对严格 SSE 目标与系统机制，原生 adapter、进度与常亮仍按 P5-3/P5-4 实施；另行审议的正文/总内存硬上限不在本轮完成范围内。
