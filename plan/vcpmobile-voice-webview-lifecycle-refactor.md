# WebView 重建时的语音采集所有权审查与重构方案

审查日期：2026-10-29。源码基线：`eaa7c794`。本方案遵从用户最新决定：**页面销毁视为用户离开，只关闭麦克风；逻辑通话、已接纳的识别、模型回复和通话朗读不因此结束。** 新页面恢复原通话，用户再次点击才开麦。显式挂断继续沿现有路径执行。

2026-10-29 实施完成：本次授权范围内的采集生命周期重构已落地，实际变更与验证见末尾实施记录。下文设计中提及的全局通话 target 投影和显式挂断错误处理仍未扩大进入本次范围。

此次根因是语音编排迁入 Rust 后，原生层仍保留了页面销毁直接结束通话的旧路径。修复应完成页面与采集的所有权边界，不重写已有 ASR、消息提交、群聊调度和 TTS，也不扩展通话结束机制。

## 已确认的根因与历史

[NativeVoiceAudio.swift](../src-tauri/plugins/vcp-mobile/ios/Sources/Voice/NativeVoiceAudio.swift) 的 `attach` 在第 177 行调用 `closeCall("webview_replaced")`，旧 WKWebView 的析构回调在第 184 行同样调用 `closeCall`。该函数停止采集后，将当前 ID 写入 `closedCalls`，再清空原生 `callId`。Rust 收到采集停止事件只关闭输入，仍保留活动通话。

新页面的 [voiceConversation.ts](../src/features/chat/voice/voiceConversation.ts) 第 85 行根据 Rust 快照恢复原 ID。后续原生 `start` 第 199 行因 ID 不匹配而返回 `voice_call_not_active`。恢复时补发 `begin` 也不能解决：第 122 行会拒绝已关闭的 ID。`closedCalls` 实际只保留最近 32 项，并非无限永久墓碑，但当前通话依然无法重新开麦。已静音时 `finish` 没有 active capture，原生撤销 callId 甚至不会产生采集终止事件。

| 提交 | 已解决的问题 | 遗留边界 |
| --- | --- | --- |
| `883d3fc9` | 接入原生 ASR 与 TTS | 引入 WebView 解绑直接结束原生通话的路径 |
| `2851d275` | 将通话编排下沉到 Rust | 改变了原生调用方，但没有移除上述原生终态路径 |
| `b5b7f301` | 提前挂断拦截迟到 begin，保留中断反馈 | 覆盖命令交错，没有覆盖真实 Swift WebView 解绑 |
| `ca299240` | 恢复页面通过 Rust 当前采集 owner 关麦 | 原生通话授权在 WK 对象替换后仍已失效，无法再次开麦 |

`git blame` 确认本次直接原因来自 `883d3fc9`，不是最近关麦修复引入的回退。这些修复各自解决了真实问题，但跨层生命周期契约没有同步收拢。

现有正确基础应保留：Rust 进程内持有通话、ASR、回复和 TTS；原生音频事件通过稳定 Channel 直接送到 Rust；Vue 恢复使用先订阅后快照及 revision 防倒退。相关入口为 [lifecycle_manager.rs](../src-tauri/src/vcp_modules/infra/lifecycle_manager.rs) 第 305–350 行、插件 [events.rs](../src-tauri/plugins/vcp-mobile/src/events.rs) 的 `subscribe`。

## 需要一起封闭的采集窗口

仅把 `closeCall` 改为 `finish`，可以修复原 ID 被关闭，却不能保证页面离开后麦克风始终保持关闭。

[input.rs](../src-tauri/src/vcp_modules/chat/voice_service/input.rs) 中，`start_voice_microphone` 先在 Rust 等待配置，再调用原生启动。如果页面销毁发生于配置等待期间，原生还没有 active capture，`finish` 无对象可关闭。新页面附着后，旧请求仍可能继续执行，重新开麦。这是生产调用顺序可以推导出的窗口，尚未取得设备复现证据。

另一个同源缺口是采集快照只有 `closed/listening`，前端的 `requesting` 和 `pendingStop` 随页面消失。新页面无法完整观察尚在启动或关闭的采集。应将这些必要的操作状态归入 Rust，避免再通过前端 Promise 补偿。

## 所有权与行为契约

| 对象 | 所有者 | 页面销毁后的行为 |
| --- | --- | --- |
| 逻辑通话及 callId | Rust `VoiceCallState` | 保留；结束只走现有显式挂断流程 |
| 当前页面的开麦意图、在途采集 | Rust `VoiceServiceState` | 撤销旧页面启动资格，关闭对应采集 |
| 权限等待、输入 tap、PCM 与封存尾部 | Native capture owner | 拒绝迟到启动，执行已有停止与封尾，回报真实终态 |
| 已接纳 ASR、回复、通话 TTS | 各自现有 Rust owner | 按已有语义继续，不被页面关麦批量取消 |
| 音频图、路由、播放输出 | Native renderer/session/output owner | 沿现有静音及播放占用规则管理 |
| 快照、音量与频谱显示 | Vue | 可全部丢弃；新页面重新订阅并读取快照 |

页面离开只采用现有关麦语义：撤销采集、移除输入 tap、静音、封存已接纳音频并更新快照。**不将关麦扩大为销毁所有音频图和输出资源。** 现有通话内静音会保留 Voice Processing 图或通信路由；本次不为页面离开重设计该策略，也不以清空 callId 触发资源清理。

必须保持以下性质：

1. 页面销毁不写入原生 `closedCalls`，不取消 Rust call token，不创建新的逻辑通话。
2. 同一个页面的旧请求在页面失效后不能重新取得采集权限，包括尚未被 Rust 调度的请求。
3. 迟到的旧页面关闭、旧 capture 回执及旧 WK 析构回调不能作用于新采集。
4. 关麦保留已接纳语音；挂断仍按现有语义取消未提交工作，两条路径不合并。
5. 采集槽位依据真实停止回执释放；停止失败不能伪装成资源已经释放。
6. 新页面和回前台不自动开麦、不自动弹权限、不重放 begin、ASR 提交或 TTS。同一 document 的重复快照恢复只读取状态，不额外关麦；普通组件卸载、折叠或重绘不等于 WebView document 销毁。

## 推荐的最小重构

### 原生关闭采集与结束通话分开

在 `NativeVoiceAudio` 中明确一个页面离开入口，例如 `suspendCaptureForDocument`。它复用现有 `finish/stop` 的撤销、静音与 seal 路径，保持 `callId`；`endCall/closeCall` 只由现有 Rust 挂断路径调用。

原生可在观察到页面失效时立即停止输入，再经进程 Channel 报告采集状态。即使没有 active capture，也必须报告该页面已失去采集资格，才能撤销 Rust 中仍等待配置的启动。通知只描述页面或采集事实，不生成“通话结束”事件。

保留已有的 attachment UUID、Run 对象身份、audioEpoch、权限回调检查、取消记忆、有界采样队列、封存配额、导出释放和 playback generation。原生 `closedCalls` 继续用于显式挂断与迟到 begin 的竞争，不能因为它曾被错误使用而整个删除。

### 用页面代次约束开麦入口

复用现有 [voiceDocument.ts](../src/features/chat/voice/voiceDocument.ts) 与 [tts.rs](../src-tauri/src/vcp_modules/chat/voice_service/tts.rs) 的 Rust 单调页面票据思路，将页面身份提取为语音域内的小型 `VoiceDocumentState`。它只回答“哪个页面还能发起采集”，不拥有通话、ASR 或播放任务。

推荐契约如下，名称在实施时可沿现有命名统一：

| 操作 | 契约 |
| --- | --- |
| 分配页面票据 | Rust 签发单调 documentId；分配本身不启动音频 |
| 接入新页面 | 接纳新代次，撤销旧代次的待启动采集并关闭其当前采集；不能结束通话 |
| 显式开麦 | 携 documentId、callId 和请求关联 ID；Rust 接纳前、配置等待后、Native 实际启动前均检查代次 |
| 显式关麦 | 先校验调用页面 documentId，再在同一接纳临界区固定对应 capture/request；后续异步回执不重新查找并关闭较新的采集 |
| 页面失效 | 按具体 documentId/attachment 撤销资格；旧通知不能撤销新页面 |
| 页面恢复 | 安装观察者、接入当前页面、获取通话快照；保持麦克风关闭，直到新的用户开麦意图 |

页面票据采用“先分配、再激活”的现有模式，禁止迟到旧激活覆盖新页面。Native 也保留当前已授权票据及失效状态，防止 Rust 已发出的旧命令在桥接队列中迟到。重复失效幂等；旧票据不能通过再次 connect 复活。

Vue 发出的所有采集变更命令均校验来源页面，包括关麦。否则旧页面尚未被调度的 `close(callId, None)` 仍可能在新页面开麦后才解析当前采集并关闭它。有效的新页面可以重试关闭尚未完成收尾的旧采集；失效页面的命令不能重新解析当前采集。原生页面失效通知则按它携带的旧页面和采集身份执行清理。

提取共享页面身份时，不合并资源的清理屏障：麦克风等待其自身停止回执，独立试听/气泡朗读沿现有 TTS 页面策略，Chat TTS 不受影响。快照订阅不能因某项资源清理失败而不可用。

保留前端生成的 callId 或 requestId 作为幂等和早取消关联键是合理的；生成 UUID 的位置不等于资源所有权。需要下沉的是创建、取消、关闭确认和迟到回调处理。保留现有 end 早于 begin、close 早于 start 的拦截，不为消除前端 UUID 引入额外协议。

### 采集状态与收尾归 Rust

沿现有短锁、CancellationToken 和 operation generation 实现，不建立 actor 框架或逐请求等待队列。接纳操作时先更新意图、撤销旧权限并发布快照；配置、权限及原生调用在锁外执行，完成后核对具体页面及采集代次。

快照的麦克风状态补齐 `closed / starting / listening / stopping`。停止失败保留实际未确认完成的状态及错误，使新页面仍能重试；不能仅由 JS Promise 的 resolve/reject 判定硬件终态。高频音量和频谱继续使用有界展示事件，不放进全量快照。

`microphoneMonitor.ts` 最终只保留订阅、帧身份检查、数值裁剪和显示清理。移走 `pendingStop` 对资源启动的排序责任、旧 start 返回后补 close 的正确性依赖，以及当前 CaptureRequest 存在才可关麦的假设。Vue 可以保留短暂按钮 pending，但它不改变 Rust 的采集事实。

停止必须抢占等待中的启动，不能先 await 权限完成。页面失效只撤销 capture token；已封存且被接纳的语音继续使用原有 ASR 取消域。显式挂断的取消域和顺序不在本次重构中扩大。

## 页面消失的实际接入点

不能依靠 `beforeunload`、Vue unmount 或新页面成功启动才能关麦。现有锁定依赖提供了可用的原生入口：`tauri 2.12.4`、`tauri-runtime-wry 2.12.2`、`wry 0.57.0`，见 [Cargo.lock](../src-tauri/Cargo.lock)。

| 场景 | 推荐接入 | 约束 |
| --- | --- | --- |
| iOS WK 对象替换或析构 | 现有 `voice.attach` 与 lifetime 回调 | 改为撤销页面采集资格并关麦，保留 callId；沿用 attachment 身份保护 |
| 同一 WebView 文档替换 | Tauri `on_page_load` 的 Started 通知 | 在组合根转发到语音域；旧文档失效，新文档取得新票据 |
| iOS WebContent 进程终止 | Tauri `on_web_content_process_terminate` | 即使新页面未恢复，也先撤销旧页面采集并安排关闭 |
| 进入后台 | 现有原生生命周期关麦路径 | 保留现有 inactive 与 background 区别，权限弹窗不能被误判为离开 |
| 新页面已开始恢复 | 页面票据接入时再次核对 | 这是幂等核对，不是唯一的停止触发器 |

锁定依赖源码已核对：Tauri `src/app.rs:1848/1910` 暴露上述 hook；Wry `src/wkwebview/navigation.rs:17–25` 的 Started 来自 **didCommitNavigation**，不能把它描述为导航请求刚发起就触发；`tauri-runtime-wry/src/lib.rs:5048–5073` 默认在 WebContent 终止后 reload。

**安装自定义 WebContent 终止 hook 会替换默认 reload。** 实施时必须在撤销旧采集后明确保留原有 reload 行为，不能修好音频却让页面失去自动恢复。该接线仅用于 iOS，`lib.rs` 只转发事实，业务处理仍留在语音域。无需接管 WKNavigationDelegate、修改依赖源码、添加 JS 心跳或把 `useWebViewResume` 的绘制 token 变成语音所有者。

原生 WK 替换、文档 reload、WebContent 进程死亡和宿主进程死亡分别处理。此次保证的是宿主进程存活时的关麦及通话恢复；不新增跨宿主进程的活动通话持久化或自动重新开麦。

## 实施顺序与改动范围

| 步骤 | 改动 | 完成条件 |
| --- | --- | --- |
| 1 原生语义分离 | `NativeVoiceAudio.swift` 的页面解绑与通话结束入口分离 | 正在采集及已静音两种情况均保留 callId，原 ID 可再次显式开麦 |
| 2 采集来源闭环 | voice document 票据、Rust input admission、插件 Rust/Swift/Kotlin 参数及页面事件接线 | 配置等待、权限等待和桥接队列中的旧请求均不能跨页面重新开麦 |
| 3 前端投影收敛 | 采集阶段快照、`voiceConversation`、`voiceCallStore`、`microphoneMonitor` 与恢复接入 | 新页面不需要旧 Promise/current/pendingStop 即可正确显示、关闭和重开麦克风 |
| 4 跨层验收与清理 | 定向测试、删除已替代 IPC/字段及旧假设、更新模块文档 | 直接故障和迟到启动窗口均被覆盖，现有通话行为无回退 |

步骤 2 与 3 的 IPC 参数和快照变更必须原子更新调用方，不能留下两套长期路径。步骤 1 可独立审阅，但它单独完成不代表整个问题已经收口。

主要路径是 `src-tauri/src/vcp_modules/chat/voice_service/{call,input,tts,mod}.rs`、插件 `src/{voice,events}.rs` 及原生 voice adapter、`src/features/chat/voice/`。`commands.rs`、`lib.rs` 只做注册和生命周期接线。没有必要修改数据库 schema、同步 DTO、声音库格式、JEV 或发布流程。

审查另发现全局通话快照目前会被当前页面 target 筛掉，且 JS 挂断失败主要写入 console。这些可单独审议；本次不借页面销毁关麦扩展自动导航、通话结束重试或完整 call phase 状态机。涉及的恢复代码仍应保留先订阅后快照、revision 防倒退以及消息历史按目标隔离。

## 验证与验收

本次基线运行结果：前端 `VoiceConversation`、`VoiceMicrophoneRecovery`、`VoiceCallStore`、`VoiceHistorySync` 共 **36 项通过**；Rust `vcp_modules::chat::voice_service` 共 **188 项通过**。host 构建存在已有的移动专用未使用代码警告。结果不能证明 iOS 解绑正确：当前前端恢复测试 mock 了 start 成功，Rust 恢复关麦测试主要覆盖 Registry，不执行 Swift `attach → closeCall`。

实施验证应覆盖以下真实操作边界，而不是另写一份预期状态机与自身对照：

| 场景 | 必须观察到的结果 |
| --- | --- |
| 已静音后销毁页面，再恢复和开麦 | callId 不变；Native 不因 closedCalls 拒绝 |
| 正在采集时页面失效 | 旧采集停止，尾部只封存一次；新页面显示关闭 |
| Rust 配置读取未完成时页面失效 | 旧请求不调用或不能通过原生启动；无需 JS 补 close |
| 权限或 VAD 初始化未完成时页面失效 | 迟到允许或初始化完成不重新开麦 |
| 旧请求尚未被调度，新页面已接入 | 旧 documentId 的启动被拒绝 |
| 新页面开麦后收到旧关闭/析构/回执 | 新采集不受影响 |
| 同 WK 文档 reload | 仅关麦，保留通话，不自动再开麦 |
| WebContent 终止且页面尚未恢复 | 已撤销旧采集，保留框架原有 reload；不等待新 JS 收尾 |
| 关麦失败、重建页面、再次关闭 | 快照不伪报关闭；仍可针对真实旧 owner 重试 |
| 尾部导出、ASR、回复、通话 TTS 期间离开 | 已接纳工作继续，不重复提交或朗读，封存资源最终释放 |
| 页面失效与显式挂断交错 | 仍保持已有挂断不复活、只影响对应 call 的性质 |
| 多次 reload 与重复通知 | 无重复监听、无无界票据/PCM 积累、无残留输入 tap |

Rust 测试应调用实际 admission/close 函数，故障注入仅控制配置读取、权限和原生回执的完成顺序；前端测试使用全新 Pinia 与监听器恢复。Native 层覆盖实际页面失效入口与实际 capture 状态转移，不能用同名测试替身重新实现 Swift 业务逻辑，也不能用字符串匹配断言代替行为验证。

按改动运行 `pnpm check`、相关 Rust 测试、前端语音测试及 Android 插件定向测试和 arm64 编译。iOS 按既有 Windows 源码审计与按需 Apple G1 编译链验证；本方案不建设 iOS 实机测试体系。若当前 Apple 链只能提供编译证据，直接标注原生运行行为尚无证据，不把 Windows 测试成功写成 iOS 硬件行为通过。

最终验收标准是：**页面消失后，无论旧 JS 是否完成最后一次调用，旧页面都不能再开启麦克风；存活通话无需挂断重建，新页面显式开麦即可继续。**

## 实施记录

`VoiceDocumentState` 现已从 TTS 提取为语音域页面身份 owner。`allocate_voice_document/connect_voice_document` 管理新页面准入，`connect_tts_document` 继续作为有真实消费者的独立播放清理屏障。开麦、关麦在同一个 document 接纳锁内固定采集目标，Native 启动也检查相同票据；原生自主 WK 解绑即使没有 active capture，也会通知 Rust 撤销仍在准备的请求。

采集取消令牌与 ASR 取消令牌分开。旧页面关麦只撤销 capture，保留已接纳 ASR；重复关闭共享对应 session 的关闭屏障，真实回执后才封尾和释放。启动失败后的停止失败也保留 owner，不再取消已接纳语音或伪报完成。快照提供四种麦克风状态，前端仅投影；页面接入清理失败或悬挂不阻塞 App 显示和通话快照读取。

Tauri 页面加载及 iOS WebContent 终止 hook 已接入采集撤销，后者保留默认 reload 行为。Swift 页面解绑复用现有 `finish`，不清空 callId、不写已挂断集合、不强制销毁 VP 图。现有显式挂断、通话 TTS、播放渐出与尾部导出所有权保持。

验证结果：

- `pnpm check` 通过，包含配置契约、ESLint、Vue/构建 TypeScript 和 host Rust 检查。
- 前端全量 109 个文件、974 项测试通过；新增页面接入不阻塞显示、四态恢复、关闭快照先于旧启动响应等场景。
- Rust 语音模块 196 项测试通过，插件 Rust 28 项测试通过；覆盖页面失效、旧命令拒绝、ASR 保留、共享关闭屏障、旧回执隔离及 Native 事件契约。
- Android 插件 JVM 31 个套件、207 项测试通过，含真实 NativeVoiceAudio 的权限等待和文档失效路径；Android arm64 Rust 检查通过。
- iOS 已逐路径复核 Swift 解绑、权限、输入停止、事件与 Tauri hook；本轮没有执行 Apple G1 或实机运行，以上测试不代表 iOS 音频硬件运行验证。
