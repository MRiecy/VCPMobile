# Mobile 群聊核心与重构边界审计

日期：2026-10-19。角色：Casper（务实与交付）。本篇以 `E:\VCPMobile`、`E:\VCPChat` 当前生产源码为证据，区分现状、差异和建议；只新增审计材料，不实施生产修改。源码审阅不等于编译、真机或端到端验证。本轮未为本篇运行构建、测试或设备操作。

## 1. 结论

现有 Mobile 群聊已经具有可复用的串行发言执行器、事务化消息终结、按话题排他执行、WebView 输出重新接入、语音回复精确关联与取消。必要的重构集中在**配置契约、发言调度 owner、群会话状态投影**，不需要重写模型请求、消息仓储、附件处理或语音核心。

顺序排列是低复杂度的独立配置能力；JEV 是在当前队列结束后反复裁决、接纳运行中人类输入和手动插队的会话能力，不能只在现有 `if mode` 中增加一个返回静态成员数组的分支。同步字段贯通必须先于新配置的编辑与回写，否则界面和执行器即使实现正确，也不能证明跨端保存安全。

## 2. 当前 Mobile 生产调用链

```text
文字提交 / 重新生成 / 语音接纳 / 显式邀请
    → 群话题执行 owner（ActiveGroupTurns）
    → 群配置、成员、历史
    → 发言计划
    → run_group_speaker_turn
    → 统一上下文装配与 VCP 请求
    → durable finalizer 提交消息
    → RequestOutput / 群轮事件 → Vue 与 Rust TTS
```

| 边界 | 源码事实 | 证据 |
|---|---|---|
| 配置存储 | `GroupConfig` 是固定字段结构，没有 `modeSettings`、独立顺序或群成员历史窗口字段；`mode` 是字符串 | [group_types.rs](../../src-tauri/src/vcp_modules/group/group_types.rs)，56–95 行 |
| 配置写入 | 读取实体锁下的配置，`update_group_config` 合并顶层值后重新反序列化成 `GroupConfig`；未知字段不会成为持久化字段 | [group_service.rs](../../src-tauri/src/vcp_modules/group/group_service.rs)，305–326 行 |
| 配置提交 | 同一事务写 groups、重建有序 group_members、计算同步 DTO hash；提交后才发布缓存，已有缓存代次防止同步后的旧快照迟到回填 | [group_service.rs](../../src-tauri/src/vcp_modules/group/group_service.rs)，20–70、432–547 行 |
| 自动发言 | 顺序模式直接使用当前 members 顺序；自然随机使用既有标签策略；邀请模式返回 `no_ai_response`；未知模式返回 `mode_not_implemented` | [group_chat_application_service.rs](../../src-tauri/src/vcp_modules/group/group_chat_application_service.rs)，438–479 行 |
| 串行执行 | 每位发言者完成后才进入下一位；仅 `Settled` 继续，取消、准备失败、恢复待决、丢弃等停止本轮 | [group_chat_application_service.rs](../../src-tauri/src/vcp_modules/group/group_chat_application_service.rs)，57–95 行 |
| 单次回复 | 创建独立消息身份与 request lease，写入活动生成，再请求模型；finalizer 提交成功后才把该消息加入下一位的上下文 | [group_chat_application_service.rs](../../src-tauri/src/vcp_modules/group/group_chat_application_service.rs)，104–344 行 |
| 请求复用 | 模型路由、CAS 多模态准备、冻结连接、stream/non-stream 分发已经集中；模型回复不应另造 JEV 专属实现 | [vcp_client.rs](../../src-tauri/src/vcp_modules/infra/vcp_client.rs)，1254–1356 行 |
| 显式邀请 | 一次调用接收 ID 数组，先校验所有成员并去重，再按邀请顺序复用同一 speaker runner | [group_chat_application_service.rs](../../src-tauri/src/vcp_modules/group/group_chat_application_service.rs)，718–830 行 |
| 重新生成 | 在写历史墓碑之前检查群模式；当前仅顺序、自然随机允许，邀请和未知模式被明确拒绝 | [topic_service.rs](../../src-tauri/src/vcp_modules/chat/topic_service.rs)，839–895 行 |

**未知 JEV 模式当前不会静默执行为顺序模式。** `GroupConfig` 的 sequential 默认只在 `mode` 缺失时生效；`mode="jev"` 会保留，并在执行时返回未实现。新版本应保持这一点，读取未支持配置时不能擅自把模式改写成默认值。前端提示和编辑器呈现见本目录的 UI 审计。

## 3. 顺序模式的实际新语义

VCPChat 的权威配置为 `modeSettings.sequential.speakerOrder`，运行策略也接受旧 `sequentialSpeakerOrder`。它对活跃成员排序：已配置成员优先，未配置或新加入成员按原成员数组顺序追加。保存归一化先去重、过滤非成员 ID，再补齐当前成员，并镜像旧字段。

证据：[sequentialMode.js](E:/VCPChat/Groupmodules/modes/sequentialMode.js)，20–36 行；[groupchat.js](E:/VCPChat/Groupmodules/groupchat.js)，458–483、507–524 行。

这不是轮次数组、重复发言脚本或多次出现同一个成员的流程。一次用户触发仍生成一个串行队列，每个活跃成员最多出现一次。成员集合与发言顺序是不同配置；不宜通过重排 `members` 或 `group_members.sort_order` 模拟新能力，否则会改变成员展示顺序并丢失独立排列的含义。

建议：新增明确的顺序解析函数，由 group 配置归一化负责去重、过滤与补齐，策略函数只对可加载成员投影。默认顺序沿用现有 members。成员暂未同步到本机、成员已删除、退群后重新加入应分别处理：当前配置写入已经保留缺失 Agent 的前向引用，仅移除明确墓碑成员，不能把暂时未加载成功等同于用户退群（`group_service.rs`，454–470 行）。

## 4. 两种历史窗口必须区分

VCPChat 新增的 `enableContextMessageWindow` 和 `contextMessageWindowSize` 限制**发送给群成员回复模型**的历史。默认关闭；大小默认 100，归一化为 1–10000；按历史消息条目保留最新 N 条，不改变持久化历史、展示或话题总结。见 [groupContextWindow.js](E:/VCPChat/Groupmodules/groupContextWindow.js)，3–38 行；成员请求调用点见 [groupchat.js](E:/VCPChat/Groupmodules/groupchat.js)，954–956 行。

JEV 的 `historyWindow` 则属于**裁决模型输入**，决定裁决所见最近消息。两者不能合并为一个开关，也不能让 JEV 的较短上下文截断成员回复上下文。

Mobile 当前在自动发言与邀请路径读取全部已落盘纯文本/附件历史，再传给统一上下文装配（`group_chat_application_service.rs`，483–490、802–809、162–171 行）。仓储已经支持按最新 N 条查询且排除仍在生成中的消息，见 [message_service.rs](../../src-tauri/src/vcp_modules/chat/message_service.rs)，478–524 行。

建议：在群领域选择成员历史窗口，再进入现有 Tavern/邀请词/语音指导装配；尽量在仓储查询时限制，避免先加载长话题所有附件提取文本再裁剪。每位成员发言前以最新已提交历史选择窗口，保证上一位回复和运行中新增的人类消息可见。关闭窗口时维持现有产品语义，不借本任务擅自强加统一截断。

自然随机另有既有差异：策略按“排除最后用户消息后的 8 条历史”编写（[group_speaking_policy.rs](../../src-tauri/src/vcp_modules/group/group_speaking_policy.rs)，19–29 行），调用层只加载总共 8 条（application service，438–445 行），实际最多留下 7 条旧消息；VCPChat 策略直接取 9 条再排除当前条（[natureRandomMode.js](E:/VCPChat/Groupmodules/modes/natureRandomMode.js)，30–31 行）。这是可单独修正的输入窗口差异，不足以要求重写自然随机算法。

## 5. JEV 的会话能力与重构范围

### 5.1 需要改变什么

VCPChat 群入口在 JEV 模式下先记录真人消息并通知运行中会话；会话未运行才 start。会话提供随机开场、继续、手动成员插队；在队列耗尽后再次裁决；仲裁期间如果 history revision 改变，则丢弃旧裁决。见 [groupchat.js](E:/VCPChat/Groupmodules/groupchat.js)，878–897、2479–2540 行；[jevGroupSessionOrchestrator.js](E:/VCPChat/Groupmodules/jevGroupSessionOrchestrator.js)，116–207、268–394 行。

Mobile `ActiveGroupTurns` 当前是按 `TopicKey` 的单租约容器，包含 turn ID、一个取消 token、输出 Channel、一个 origin 和语音 feed；第二个同话题租约直接返回“already active”。见 [vcp_client.rs](../../src-tauri/src/vcp_modules/infra/vcp_client.rs)，952–1002 行。它能保护一次固定队列，但没有运行中输入接纳、动态队列、仲裁状态、revision 或手动插队能力。

建议把既有群轮所有权演进为**群领域的单一会话调度 owner**，让顺序、自然随机、邀请、JEV 使用同一成员回复执行链：

- 静态模式提交固定计划；JEV 提交动态调度事件。
- 同话题已经运行时，真人消息和插队投递给原 owner，不再次占有一份群轮。
- 仲裁绑定其观察的历史 revision；真人消息、相关删除/修改使旧裁决失效。拒绝过期结果后读取已提交事实重新判断。
- 当前成员请求与后续队列分别停止；会话终结等待真实成员请求和持久化收尾，不能发出结束事件后留孤立任务。
- 删除群/话题时也要通知没有活动消息行的仲裁阶段；现有删除路径主要从 `active_generations` 找请求，不能作为未来 JEV owner 的唯一发现方式（`group_service.rs`，695–716 行）。

这是 owner 演进，不建议在 `ActiveGroupTurns` 之外再维护一个可以独立声称同话题完成的 JEV 状态表。也不应为了本次适配全面拆分 `vcp_client.rs`；仅提取群领域状态及其消费接口即可，流协议、请求输出与恢复继续留在原 owner。

### 5.2 明确保留什么

| 保留的结构 | 原因 |
|---|---|
| 单个 speaker 执行器 | 已把上下文、模型路由、请求注册、消息身份和持久化终态串成一条有效路径 |
| `GroupSpeakerOutcome` 的区分 | `recovery_pending`、`stopping`、`discarded` 不能被折叠成“完成”，否则会在上一个请求未结算时调度下一位 |
| 统一上下文装配 | 现有 Tavern、邀请词、语音指导已经按正确次序组合；JEV 决策输入不应污染成员上下文 |
| SQLite 消息仓储与 finalizer | 仅已提交的回复进入后续模型上下文；删除后迟到结果不复活 |
| `RequestOutput` 与输出重接 | 当前与下一位发言者都能取得最新 WebView 消费者；JEV 会话状态增加快照投影即可 |
| 原生前后台能力边界 | 平台资源仍由原生 adapter 持有，调度与结果仍由 Rust 持有 |

相关证据：`group_chat_application_service.rs`，229–342 行；[context_assembler.rs](../../src-tauri/src/vcp_modules/chat/context_assembler.rs)，18–80 行；`vcp_client.rs`，693–722、1014–1038、2634–2676 行。

## 6. 取消语义不能直接替换

| 操作 | VCPChat 当前行为 | Mobile 当前行为 |
|---|---|---|
| 停止整组后续队列 | 清除后续发言；当前已开始回复自然完成；仲裁尚未产生可见消息时可取消 | `interruptGroupTurn` 标记话题活动生成 stop_requested，取消群 parent token，进而取消当前 child；同时停止关联朗读 |
| 停止当前回复 | `interruptGroupRequest` 仅 abort 当前请求；普通队列后续仍可继续 | `interruptRequest` 取消当前请求；speaker 返回 Cancelled 后 `run_group_speakers` 不再继续 |
| 语音插话/挂断 | 本篇不推断桌面语音行为 | 精确取消本通话已接纳 reply token，取消当前模型和后续群轮；TTS 另有停止链 |

证据：[VCPChat groupchat.js](E:/VCPChat/Groupmodules/groupchat.js)，2457–2472、2543–2588 行；[VCPChat orchestrator](E:/VCPChat/Groupmodules/jevGroupSessionOrchestrator.js)，397–421 行；[Mobile vcp_client.rs](../../src-tauri/src/vcp_modules/infra/vcp_client.rs)，1132–1172、2440–2510 行；Mobile application service，84–95、283–287 行。

建议定义三个有实际消费者的动作：停止后续队列、停止当前回复、取消整个会话/通话回复。新 JEV 队列停止可对齐桌面优雅停止；语音插话、挂断以及删除仍须硬取消其真实工作。是否把既有顺序/自然随机的两个停止按钮也整体改成桌面语义属于产品决定，不能只改底层 token 让既有 UI 文案和行为暗中变化。

“停止当前后继续下一位”只能在当前请求已确认终结且消息已提交/丢弃后发生。现有 Android helper `stop_pending` 和 `recovery_pending` 不能因希望继续聊天而跳过。

## 7. 语音关联、后台与恢复

### 7.1 精确的来源关联

当前语音提交在持久化前取得真实 Agent/Group owner，并把同一个 reply cancellation token 交给它；保存用户消息以后调用群应用服务，允许 UI Channel 缺席。见 [voice_submission.rs](../../src-tauri/src/vcp_modules/chat/voice_submission.rs)，45–93、102–161 行。

群轮保存一个 `originUserMessageId`，每位 speaker 的 request lease 复制该 origin。TTS 只接受匹配当前 narration origin 的事件，见 `group_chat_application_service.rs`，201–207 行；[tts.rs](../../src-tauri/src/vcp_modules/chat/voice_service/tts.rs)，581–615 行。插话和挂断取消真实 reply，见 [call.rs](../../src-tauri/src/vcp_modules/chat/voice_service/call.rs)，146–160、369–388 行。

**建议**：JEV 会话可拥有多条真人输入，但每次选定 speaker 时必须固定其触发来源和 voice reply policy；已有回复的 origin 不能因为后来出现一条真人消息就被统一覆盖为 latest。来源变化只影响之后调度的请求。手动开场/继续没有新用户消息时，采用明确的会话触发身份或无语音来源；不能伪造用户消息来迁就现有字段。

交叉复核补充：固定 origin 还不够。[chat_feed.rs](../../src-tauri/src/vcp_modules/chat/voice_service/chat_feed.rs) 95–126 行以匹配 `turnId` 的 `group_turn(active=false)` 结束群朗读，并忽略群成员的 `request_activity`；TTS 在进入 feed 前又按 origin 筛选。因此多来源的长会话需要每个语音回复作用域自己的完成/取消通知，不能只在最后对最新 origin 发一个结束事件。调度 owner 仍只有一个，语音来源只是它负责结算的输出作用域；验收应证明各作用域的模型与实际播放均能收尾。

需要在实施前裁定：语音对 JEV 是维持现有“说话即打断本次回复”的交互，还是把转写也当普通文本插入正在说话的会话。建议维持已明确的语音插话语义；新的文本输入则按 JEV 动态接纳处理。二者可共用调度 owner，但不能共用一个不断改写的 origin。语音 owner 必须等待其对应工作真正结束，才向后续语音提交释放注册机会（`call.rs`，89–121、611–680 行）。

### 7.2 后台和重接

当前每个成员模型请求持有 Stream 前台租约（application service，218–219 行）；语音工作还持有从识别到实际播放收尾共享的 WorkLease（`call.rs`，48–73、648、729 行）。当前输出重接会重新绑定所有活动请求及群轮（`vcp_client.rs`，2634–2676 行），群轮 drop 发出结束事件（1104–1128 行）。

JEV 增加了成员请求之外的仲裁等待阶段，因此建议由有界的会话/仲裁工作复用现有租约能力覆盖该阶段，不能假设“没有流请求就没有工作”，也不能让前端组件寿命决定后台调度。具体 Android/iOS 执行结果需要分别验证；host 编译不能证明后台连续运行。若进程死亡，建议恢复已落盘消息与已有活动请求事实，默认不自动重放尚未执行的 JEV 队列；需要跨进程自动续聊应作为单独获准的产品能力。

## 8. 存储与 IPC 改动地图

以下是实施地图，尚未实施；同步协议细节以本目录同步审计为准。

| 切面 | 应核对的实际位置 | 必要闭环 |
|---|---|---|
| 群配置类型 | `group_types.rs`、Vue 群类型、同步 DTO | 新模式配置、顺序、成员历史窗口具有一致含义；未知非空模式不改默认 |
| 配置持久化 | `group_service.rs` 的读单项/列表/创建/更新、SQLite migration、同步入库与导出 | 新字段可读、可写、可哈希；同步直写后缓存正确失效；只保存旧字段不能证明新配置保留 |
| 默认和归一化 | 群领域的配置边界 | members 与 speakerOrder 分离；旧 natural 字段与嵌套配置只保留一个业务权威；桌面镜像字段留在适配边界 |
| 调度 owner | 群应用服务、现有 `ActiveGroupTurns` 消费位置 | 动态接纳、仲裁 revision、队列、三种停止、生命周期与终态归同一 owner |
| IPC | [commands.rs](../../src-tauri/src/commands.rs)，80、135、212、244–245 行附近的现有群/请求命令组 | 启动/继续/插队/停止/状态快照沿中央 invoke 总闸；内部 speaker 接口不必暴露成 UI 命令 |
| 文字与邀请 | 群 handler 与前端发送链 | 活动 JEV 会话可接收输入；显式邀请复用 runner，不能每次偷偷创建第二会话 |
| 语音 | `voice_submission.rs`、`voice_service/call.rs`、RequestOutput/TTS feed | 保持接纳、落盘、origin、policy、取消、真实播放收尾的因果链 |
| 历史操作 | `topic_service.rs` 重新生成、群/话题删除、消息删除修改 | 未支持模式在截断之前拒绝；活动仲裁收到状态变化；JEV 重新生成语义先定再开放 |

推荐在群组持久层用明确类型描述各模式配置，存储形式可采用 canonical 模式配置 JSON，减少每增加模式字段都改多张 SQL 投影的成本；这不是使用任意 JSON 作为业务 API 的理由。究竟沿用独立列还是增加 JSON 列，需要与同步 DTO/hash 的统一规范一次确定，不能只为 Mobile 编辑器单方面选择。

## 9. 可分切的实施与验收内容

1. **契约与保存闭环**：补齐新字段的双方 DTO、hash、migration、读写与缺失字段策略。以含独立顺序、JEV 配置、旧 natural 配置和成员历史窗口的真实形状做双向往返；验证只改群名不会擦掉其它已存在配置。
2. **顺序与成员窗口**：复用现有固定队列执行器，增加顺序投影和成员历史选择；验证新增/缺失/删除成员、下一位看到上一位已落盘回复、窗口不改历史数据库、关闭窗口保持现状。
3. **会话 owner 演进**：先在原有模式保住请求排他、finalizer、恢复和语音，再接入可取消的 JEV 裁决。用可控延迟的请求/持久化边界验证：仲裁中来人类消息、准备中停止、当前回复收尾时停止、helper 尚未终结、删除发生在仲裁空档。
4. **JEV 交互与跨输入闭环**：随机开场、继续、插队、静默收敛、配置/网络失败、状态快照；同时验证语音插话/挂断及 WebView 重建，避免功能只在前台文字发送上成立。

跨层或核心实现完成后运行 `pnpm check`。有价值的行为验证应驱动真实 owner 和可控外部响应，不以再次实现一套相同算法的静态断言证明正确。Android Debug 设备验证只操作 `com.vcp.avatar.debug`；iOS 遵循现有阶段性 Apple 构建验证边界，不新增实机体系。

## 10. 待裁定与不应照搬的细节

- **停止行为范围**：只为 JEV 新增优雅停止，还是同时升级所有群模式的当前停止按钮语义。
- **语音与会话来源**：建议保留语音插话硬取消，并对未来每位 speaker 固定触发来源；不全局改写 origin。
- **JEV 重新生成**：是重新执行一个成员回复、从某条用户消息重跑整个会话，还是首期继续明确不支持；现有历史截断入口不能直接放行。
- **运行时计数寿命**：VCPChat `autonomousRound`、`speechCounts` 在 `_createSession` 初始化，`_startRun` 与 finally 没有归零，而运行循环以累计轮数判断上限（orchestrator，43–64、209–236、311–319 行）。这意味着同话题的继续/新输入可能沿用旧计数。移植前应明确上限按一次自动运行还是话题进程寿命计算，不能把此实现现象直接当作已批准产品语义。
- **非本次扩大范围**：VCPChat 还识别 game 模式，但本次用户提出的是 JEV、群组配置升级与同步 DTO；游戏引擎不应因群类型扩展被顺带实现。

本篇的工程建议是保住既有可靠提交与移动端生命周期，把新增复杂性放到真正需要它的群会话调度层。
