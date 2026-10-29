# VCP Mobile 集群 β 深度审查

审查日期：2026年10月29日。范围为模块3群聊 JEV、模块4配置中心、模块7流式治理，以及这些模块实际依赖的同步、原生文件与渲染接口。

本次确认 **8项本轮新增或集成回归问题，其中5项P2、3项P3**；另确认 **5项当前仍存在的遗留P2问题**。优先处理群聊输入乱序后的取消归属、成员误邀约、连续排序保存冲突和终态提交失败后的正文所有权。没有证据支持将本次发现升级为P0/P1，也没有确认新增的内存泄漏或流式连接死锁。

审查基线为 `bdd21d2fed0ccf7603a779f243035ba12c03a330`，对比本地 `origin/main` 的 `a8482b607df13a74e7f8d63a2b0f751221eaaf1a`：99个提交，879个文件，132207行新增、33384行删除。审查开始时工作树干净；12位子代理分工审查，主审合并重复项并复核关键候选。审查阶段只新增此报告；随后用户逐项裁定并授权修复和提交，实施记录见末节。以下发现中的行号对应审查基线，修复入口以末节及当前生产代码为准。

P2表示应安排修复的功能、并发或效率问题；P3表示影响较小的交互、反馈或冗余工作。优先级不表示每次操作都会触发。以下均按当前生产调用链判定，测试和附件中的判断不反向定义行为。

## 本轮功能与竞态问题

### B1 P2 旧语音输入迟到提交会取消较新的群聊输入

主定位：[group_session.rs:140](E:/VCPMobile/src-tauri/src/vcp_modules/group/group_session.rs:140)。`reserve()` 分配递增序号，`commit()` 只用该序号删除pending条目，随后无条件替换 `latest_scope`，没有维护最新输入的准入顺序。

完整可达时序如下：

1. 已有JEV会话。语音V先取得reservation，完成消息持久化及 [voice_submission.rs:195](E:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_submission.rs:195) 的取消检查，进入 [submit_group_session_input:623](E:/VCPMobile/src-tauri/src/vcp_modules/group/group_chat_application_service.rs:623) 的配置、成员异步读取。
2. 用户挂断。通话中普通输入框确实隐藏，但 [voiceCallStore.ts:171](E:/VCPMobile/src/features/chat/voice/voiceCallStore.ts:171) 立即恢复非通话状态，结束请求没有等待后端提交收尾；后端取消语音token，群会话根token仍可接收新输入。
3. 新文字T取得较新reservation并先commit。V随后迟到commit，把 `latest_scope` 从T换回已取消的V；T若不是当前正在执行的scope，还会提前收到完成结果。
4. [run_jev_session:699](E:/VCPMobile/src-tauri/src/vcp_modules/group/group_chat_application_service.rs:699) 看到最新scope已取消，关闭会话并清空队列。T的用户消息仍在库中，但被接受的回复调度丢失。

取消发生在持久化取消检查之前时，V会正确退出，不能用那个时序复现本问题。群配置锁和SQLite写锁都不覆盖reservation到commit的整个阶段；两位审查员独立确认这个窗口。现有顺序voice→typed测试没有覆盖reserve V→commit T→commit V。

修复方向：让最新scope记录已提交输入的准入序号，迟到旧reservation不能覆盖较新的回复归属，并独立完成旧scope的释放。仅追加一次取消检查不能解决尚未取消的旧scope迟到覆盖。证据为生产调用链与状态交错分析，尚未执行设备或故障注入复现。

### B2 P2 JEV会误邀约名字为其他成员前缀的成员

主定位：[group_chat_application_service.rs:629](E:/VCPMobile/src-tauri/src/vcp_modules/group/group_chat_application_service.rs:629)。新增JEV提交链对每位成员独立执行 `contains("@" + name)`。群内有“小明”和“小明2”时，发送 `@小明2` 会把两者都放进优先队列，直接触发额外成员回复。

前端现有 [mention.ts:38](E:/VCPMobile/src/core/utils/mention.ts:38) 在同一位置只选最长名称，并按出现顺序去重；邀请模式只会选中“小明2”。这属于同一用户输入在两条生产路径中的语义不一致，可能造成额外模型调用。

修复方向：JEV后端沿用现有最长匹配、出现顺序和去重语义。该结论不要求扩展新的提及语法。自然随机模式也有类似旧实现，此处登记的是新增JEV路径。

### B3 P2 连续拖拽排序会被前一次保存判为冲突

主定位：[AgentList.vue:105](E:/VCPMobile/src/features/agent/AgentList.vue:105)、[settings.ts:105](E:/VCPMobile/src/core/stores/settings.ts:105)。拖拽结束立即解除交互状态，没有保存期间的排序所有权或乐观顺序；`updateFields` 又在进入串行队列之前构造版本和字段基线。

第一次保存未返回时再次拖拽，第二次仍从旧 `orderedAgents/orderedGroups` 计算，并携带旧基线。第一写成功后，第二写触发 [repository.rs:541](E:/VCPMobile/src-tauri/src/vcp_modules/infra/settings_manager/repository.rs:541) 的字段冲突。组件仅打印控制台错误，最后一次调整不落盘。

实际Store和字段构造源码的定序探针得到：初值 `[A,B,C]`；首写 `[B,A,C]`；第二请求仍为 `expectedRevision=1, base=[A,B,C], changes=[A,C,B]`；第二写冲突，最终仍是 `[B,A,C]`。独立复核确认当前UI没有阻止该交错。旧版也可能从旧列表计算索引，本轮接入字段CAS后明确产生自己的后续请求被拒绝的问题。

修复方向：在排序所有者中维护待确认顺序，串行提交时基于已确认版本构造下一请求；较小方案是保存期间暂时禁用同一列表拖拽，并反馈失败。保留配置中心的冲突保护。

### B4 P2 配置导出拒绝权限后仍重复申请

主定位：[useConfigBackup.ts:284](E:/VCPMobile/src/features/settings/useConfigBackup.ts:284)。Android API26–28、尚未授予存储权限时，[NativeFileExports.kt:168](E:/VCPMobile/src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/files/NativeFileExports.kt:168) 已主动申请权限。拒绝后，前端捕获“储存空间权限”文案，再调用一次权限请求并无条件重试保存；[AndroidPermissions.kt:320](E:/VCPMobile/src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/permissions/AndroidPermissions.kt:320) 即使拒绝也resolve，因此重试保存又进入原生权限申请。

一次导出最多触发三次权限请求。永久拒绝时系统可能直接回调，不能据源码宣称必然出现三个弹窗。直接加载当前导出函数、用原生拒绝契约替代IPC的探针得到 `save_to_downloads → request_android_permission → save_to_downloads`。

旧原生实现仅检查权限，前端负责申请；原生接管申请后，旧前端补偿分支未移除，形成职责重复。修复方向：由原生统一处理本次申请与授权后保存，前端直接呈现拒绝结果。未进行真机弹窗验证。

## 本轮效率与轻微交互问题

### B5 P2 单群冷读触发全库规范化

主定位：[group_service.rs:377](E:/VCPMobile/src-tauri/src/vcp_modules/group/group_service.rs:377)。每次单群缓存未命中，都会调用没有group_id过滤的 `normalize_group_configurations`：先读取并逐项解析、序列化全部本机群偏好，再 `SELECT *` 扫描、解码全部活跃群配置，即使所有配置合法也要完成扫描。

[push_executor.rs:836](E:/VCPMobile/src-tauri/src/vcp_modules/sync/sync_executor/push_executor.rs:836) 对每个群调用该单群入口。冷缓存下推送G个群，可形成O(G²)行扫描与配置解析，并反复分配全库查询结果。同步并发上限为3，不是无界并发。

修复方向：单群读取只规范化目标群，列表或启动批量修复再全库处理。现有agent仓储已支持按单个ID规范化，可沿用同样尺度。复杂度由实际调用链确定，未测手机耗时与内存峰值。

### B6 P3 每轮JEV仲裁都加载不会使用的附件

主定位：[group_chat_application_service.rs:800](E:/VCPMobile/src-tauri/src/vcp_modules/group/group_chat_application_service.rs:800)。该处的 `load_chat_text_history_for_context(..., false)` 只关闭提取文本，并不关闭附件加载。[message_service.rs:528](E:/VCPMobile/src-tauri/src/vcp_modules/chat/message_service.rs:528) 仍查附件，解析 `image_frames`，逐附件等待路径刷新；[file_manager.rs:363](E:/VCPMobile/src-tauri/src/vcp_modules/infra/file_manager.rs:363) 再派发阻塞任务检查文件和缩略图。

JEV请求只消费正文、身份和时间等字段，不读取附件。自主多轮会反复承担这些查询、分配、解析与串行任务切换。修复方向：仲裁采用纯文本历史投影，明确区分附件元数据与提取文本需求；实际成员回复仍使用所需的多模态上下文。

### B7 P3 群名自动保存可能吞掉首次返回操作

主定位：[GroupSettingsView.vue:410](E:/VCPMobile/src/features/agent/GroupSettingsView.vue:410)、[GroupSettingsView.vue:335](E:/VCPMobile/src/features/agent/GroupSettingsView.vue:335)。修改名称后直接返回，输入失焦先发起保存并同步置 `isSaving=true`；返回按钮随后禁用，关闭处理也直接拒绝保存期间的关闭，保存完成后不续接关闭意图。

真实SFC加deferred保存的DOM探针确认：blur后 `saved=1, disabled=true, closed=0`；点击返回和保存完成后 `closed` 仍为0。用户需要再次返回，没有发现数据损坏。修复方向：记录关闭意图并等待同群已有保存，成功后关闭、失败留页；不重复启动保存。此为DOM逻辑证据，未做触屏实测。

### B8 P3 墓碑群偏好被跳过但仍报告恢复成功

主定位：[group_service.rs:314](E:/VCPMobile/src-tauri/src/vcp_modules/group/group_service.rs:314)、[useConfigBackup.ts:134](E:/VCPMobile/src/features/settings/useConfigBackup.ts:134)。偏好写入正确排除已删除群，但导入层忽略是否实际写入；前端只要备份中有群偏好，就把“群聊设置”加入已恢复项。

当旧备份只含已经删除的群偏好时，实际未恢复任何群配置，界面仍报告成功。未发现墓碑复活或同步污染。修复方向：区分墓碑跳过和内容已经相同，将跳过结果反馈前端；不能把所有零行更新都当作失败。

## 当前仍存在的遗留问题

以下5项在 `origin/main` 已有对应结构，不能归因于本次99个提交；当前代码仍可触发。

### L1 P2 群共享配置增量保存可覆盖刚同步到的其他字段

[group_service.rs:605](E:/VCPMobile/src-tauri/src/vcp_modules/group/group_service.rs:605) 在写事务外读全配置，合并本地补丁，741行才获取写事务；之后全量写共享列与成员。同步writer使用数据库写协调器，但不取得群实体锁，因此“本地读取 → 同步更新提示词 → 本地只改名并保存”会把提示词改回旧值。缓存代次不能防止这个落盘覆盖。

从生产源码提取两条实际UPSERT，在内存SQLite按此顺序执行，结果为 `("Locally renamed", "old prompt")`。这是SQL交错复现，未运行整个IPC调度。修复方向：在同一写事务读取当前共享配置并合并补丁。纯本地参数保存已有事务内合并，不受此问题影响。

### L2 P2 无Helper路径在终态保存失败后丢失正文

[message_service.rs:1608](E:/VCPMobile/src-tauri/src/vcp_modules/chat/message_service.rs:1608) 在终态提交失败时仅把正文随error事件交给前端，然后返回；请求lease释放后Rust不再持有待提交结果。iOS流式和双端非流式都可能没有Helper副本。

前端只在内存保留正文。下一次恢复中，[vcp_client.rs:3308](E:/VCPMobile/src-tauri/src/vcp_modules/infra/vcp_client.rs:3308) 的无Helper路径用空正文生成失败终态；新的end又覆盖前端已有回答。触发需要终态事务暂时失败、随后恢复成功，原进程不必死亡。两位审查员独立追链确认，未做数据库故障注入。

修复方向：在现有请求owner中保留待提交终态及原因，直到SQLite提交或明确放弃；恢复先重试待提交结果。这不同于用户已接受的进程死亡后未落盘正文丢失，不要求新增生成期间持续落盘。

### L3 P2 多气泡尾部更新重复计算稳定块HTML

[MessageRenderer.vue:362](E:/VCPMobile/src/features/chat/MessageRenderer.vue:362) 在最后稳定块为 `<!--brk-->` 时，随tailBlock更新创建新气泡数组；[1034行的内层v-memo](E:/VCPMobile/src/features/chat/MessageRenderer.vue:1034) 编译后被多个外层气泡共用同一个缓存槽。至少两个稳定气泡时，缓存互相覆盖，稳定AST又被遍历并生成HTML字符串。

两位审查员分别编译当前真实SFC模板，用当前Vue 3.5.44和happy-dom验证：两个稳定气泡加尾部气泡，初始稳定HTML计算2次，连续3次仅更新尾部，每次仍计算2次；稳定区DOM mutation为0。因此这是CPU与分配冗余，不能描述为全量DOM重建。

修复方向：使稳定气泡具有独立组件缓存，或按稳定块身份缓存HTML投影，避免尾部变化影响稳定列表引用。本次隔离稳定投影和调整memo写法没有彻底消除旧问题。

### L4 P2 持续输出可能让恢复快照一直追赶

[chatStreamStore.ts:445](E:/VCPMobile/src/core/stores/chatStreamStore.ts:445) 重建快照期间，只要正文或cursor前进，就丢弃结果并立即全文重建。Rust仍按33ms周期发送实时帧，每次恢复也重新编译全文。如果每次编译加IPC期间都有新帧，恢复就无法提交，结构化尾部保持恢复文本，同时反复承担全文编译。

提取真实恢复函数、替代外部依赖的定序探针中，连续3次在回复前推进cursor，得到 `calls=4, commits=0`；暂停输出后成功提交，durable end到达也会退出。它不是永久死锁，设备上实际达到该条件的消息长度、耗时尚未测量。新的reset接管也不会主动取消已有重建job。

修复方向：让快照与后续增量可接续，并结束已经过时的重建工作，避免依赖整个重建窗口没有新帧才能提交。

### L5 P2 Android HTTP2失败丢失诊断信息

[SseSessionCoordinator.kt:361](E:/VCPMobile/src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/sse/SseSessionCoordinator.kt:361) 使用 `t?.message ?: response?.message`。OkHttp SSE在HTTP非成功状态可能传入null异常，而HTTP/2的reason phrase为空字符串；于是401、429、500等都发送空错误，最终消息只剩“VCP流式错误:”而没有状态码。

使用实际OkHttp 4.12.0依赖执行无网络JShell探针：解析 `:status=401`，交给 `RealEventSource.processResponse`，输出 `status=401, throwable=null, emitted error length=0`。依据为 [RealEventSource官方源码](https://raw.githubusercontent.com/square/okhttp/parent-4.12.0/okhttp-sse/src/main/kotlin/okhttp3/internal/sse/RealEventSource.kt) 与 [Http2ExchangeCodec官方源码](https://raw.githubusercontent.com/square/okhttp/parent-4.12.0/okhttp/src/main/kotlin/okhttp3/internal/http2/Http2ExchangeCodec.kt)。

修复方向：HTTP失败至少保留状态码，使用非空异常信息；如消费服务器错误正文，应限定读取范围。该缺陷影响诊断，未发现把HTTP错误误当成功。

## 内存与效率观察

下列项目与确认缺陷分开，避免将未测风险或产品取舍升级为故障。

- **JEV的4 MiB闸门只约束编码输出。** [group_speaking_policy.rs:77](E:/VCPMobile/src-tauri/src/vcp_modules/group/group_speaking_policy.rs:77) 在编码前已把历史正文复制成Value；历史最多200条不等于累计字节有界。超长合法历史会先支付查询和复制成本，再在编码处被拒绝。可减少中间副本，并把与现有字节限制等价的检查前移；没有OOM或手机峰值测量证据，本项不计入确认故障，也不建议自动截断历史。
- **过期JEV仲裁仍会完成请求和重试。** [group_chat_application_service.rs:812](E:/VCPMobile/src-tauri/src/vcp_modules/group/group_chat_application_service.rs:812) 等待decide后才检查revision。默认30秒超时、2次重试及500毫秒指数退避下，三次超时约91.5秒；新输入或手动邀请可能等待这些已无效的工作。研究文档也明确描述了“丢弃过期结果而不取消远程请求”的行为。故记录为响应性改进议题，若要改成revision失效即取消，应明确调度语义，不把它混同于网络错误fallback。
- **排序比较器有既有二次查询成本。** [agentOrder.ts:12](E:/VCPMobile/src/features/agent/agentOrder.ts:12) 比较时反复 `indexOf`，可预建排名Map。host Node单次探针在500、2000、5000项下分别约3.72、63.70、613.33ms，Map等价结果约0.34、1.26、2.43ms；这些不是移动端指标，通常列表规模下的收益需结合实际数据。
- **折叠ToolBlock仍建结果DOM属于旧成本。** 当前 `v-show` 与按50项计数缓存不能证明字节峰值有界，但普通token到来不会重解析闭合稳定工具块，没有依据升级为本轮OOM缺陷。
- **首次建库存在旧的两阶段登记窗口。** `db_manager.rs:632–686` 的baseline schema执行与迁移记录登记不在同一事务；中途被杀后的重启路径值得单独故障注入核验。这不是0102–0104新增迁移的原子性问题，本轮未做物理强杀实验。

## 附件疑点的复核结论

| 疑点 | 结论 |
| --- | --- |
| JEV网络错误必须自动fallback | 当前fallback只处理有效决策下无人入选；有限重试后终止有前端错误提示，未据此判缺陷或增加自动降级。 |
| 配置JSON搬运中断会部分覆盖 | 归档、领域插入、完成标记及旧行删除在同一写事务；保存owner独立于IPC等待者。源码与中断回滚用例均支持原子性。 |
| 一个配置字段损坏使整个domain重置 | 逐字段规范化保留合法兄弟；容器损坏及连接凭据的关联规则另有明确语义。未发现附件推定的整域误重置。 |
| ConfigBackupSection非Prop告警是生产缺陷 | 来源是测试通用挂载参数；生产唯一调用不传settings。不能为消除测试告警给生产补无用Prop。 |
| 本机群偏好进入同步DTO或hash | DTO白名单、hash投影及同步UPSERT未包含这些本机字段。 |
| 墓碑未清理群本机设置 | 0104覆盖UPDATE墓碑、硬删除、INSERT墓碑三种情形；先导入偏好再收到墓碑也会清理。 |
| localhost代理无超时 | Rust连接2秒、服务就绪约3秒、控制I/O3秒；Native命令读取和写watchdog也有限时。 |
| 停止与重连重复发起上游生成 | 当前停止先落盘，恢复以唯一lease认领；Android恢复使用resume。未发现确认的重复start或ABA清理新lease。 |
| iOS没有Swift SSE helper即不完整 | iOS真实路径为Rust reqwest与SseDecoder；Swift只负责尽力保活租约。当前D2明确接受该范围。 |
| 未收到DONE或无正文总配额必须修复 | 正常EOF可结束、进程死亡可丢未落盘正文、生成正文不设累计字节截断，均有现存用户决定。没有擅自增加门禁、LRU或兜底。 |
| 设置成功返回时更新baseline存在可达外部写竞争 | 人工插入额外外部writer可构造该机制，但当前生产写入口通过单Store串行队列，未找到真实第二写者，不计确认缺陷。 |

原生iOS租约的主队列串行化、独立tag、最后使用者释放、旧播放器身份校验，以及Swift弱引用链没有发现确认泄漏。SSE decoder通过已扫描位置避免跨chunk从头扫描。共享前端的消费者世代、完整会话键、终态墓碑、rAF清理和后台mutation上限也有实际闭环；这些结论不等于峰值内存已达标。

## 已执行验证

| 验证 | 结果与范围 |
| --- | --- |
| `pnpm check` | 通过；包含config只读一致性、ESLint、Vue TypeScript、构建配置TypeScript和host Cargo check。 |
| 前端专项第一批 | 12文件137用例通过：GroupJevControls、GroupSettingsView、GroupInviteBar、SettingsActions、useConfigBackup、ConfigBackupNativeOperation、SettingsEditSession、JevSettingsPage、StreamRenderBackpressure、StreamRenderCommit、ChatConcurrencyGuards、PersistedConversationRestore。 |
| 前端专项补充 | AgentListDragLifecycle与PresentationControls共2文件13用例通过。前端合计14文件150用例。 |
| `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib vcp_modules::group` | 36通过。 |
| 同一Cargo命令过滤 `vcp_modules::infra::settings_manager` | 65通过。 |
| 同一Cargo命令过滤 `vcp_modules::infra::vcp_client` | 29通过。Rust专项共130通过。测试构建有22条host移动语音入口未使用警告，未为消除警告改产品代码。 |
| Android `:tauri-plugin-vcp-mobile:testDebugUnitTest --tests com.vcp.mobile.service.SseProxyConnectionOwnershipTest` | 构建成功，7通过，0失败。 |
| 内存SQLite读取仓库0100至0105原始迁移 | stop列、配置表、三种墓碑清理、普通共享更新隔离及事务回滚断言通过；另外的生产UPSERT交错探针复现L1。 |
| 无文件定序与运行探针 | 验证B3、B4、B7、L3、L4、L5；替代依赖的范围与可证明内容见对应条目。 |

既有选定测试合计287项通过，并不覆盖本文新增指出的全部交错。B1、B2、B5、B6、B8和L2主要依赖生产源码与调用链证据。未进行Android真机操作、性能采集或iOS archive；iOS兼容结论严格限于共享逻辑及Swift源码，不宣称Apple编译或设备运行已通过。

建议修复顺序：先处理B1、B2、B3和L2的输入及结果所有权，再收敛B4的权限所有者与L1事务边界；随后处理B5、L3、L4和B6的可消除计算，最后完成轻微交互与诊断反馈。修复时应围绕各条真实触发时序验证，保持用户既定的同步边界、正文策略和JEV失败语义。

## 用户裁定与修复落实

修复开始于 `728c966b`。此前其他集群提交的原生与语音改动全部保留。本节记录用户在审查后作出的新决定，优先于上文审查时的建议与原有行为描述。

| 项目 | 落实方式 |
| --- | --- |
| B1 输入归属 | reservation准入序号保护最新scope；旧输入迟到提交与取消回调不能夺回较新输入的归属。 |
| B2 成员提及 | 在现有提及语法内做最长名称匹配、出现顺序及去重，避免前缀成员误邀约。 |
| B3 与排序比较器 | [agentOrderStore.ts](E:/VCPMobile/src/features/agent/agentOrderStore.ts) 每类列表只保留最终顺序，立即更新显示，300ms无新拖拽后保存；在途保存后的变化继续合并，确认后才构造后续版本基线。比较器预建Rank Map。 |
| B4 导出权限 | 前端移除重复申请和重试，原生文件保存者统一处理本次权限与保存。 |
| B5 单群规范化 | 单群冷读按ID查询与事务内重读，仅列表入口全量规范化。 |
| B6 仲裁历史 | JEV使用纯文本历史投影，不读附件表、不刷新附件路径。 |
| B7 关闭联动 | 保留群名失焦保存；返回等待同群已有保存，成功后关闭，失败留页，编辑世代防止误关新页面。 |
| B8 本机配置导入 | 只恢复本机存活群；缺实体提示先同步后再次导入，墓碑静默跳过，其他设置继续恢复。新导入不预存无主群偏好，历史无主行不额外清扫。同步DTO/hash不扩展，本机配置沿默认值和vcpcfg补充。 |
| L1 | 用户明确接受当前风险，本轮不修复共享配置读改写窗口。 |
| L2 终态所有权 | [vcp_client.rs](E:/VCPMobile/src-tauri/src/vcp_modules/infra/vcp_client.rs) 的现有RequestOutput保留无Helper的待提交终态Arc，恢复优先提交原结果；成功或删除释放。停止等待producer退出后仅结算遗留终态一次，不增加生成期间落盘。 |
| L3 稳定渲染 | 稳定HTML采用消息局部WeakMap，尾部气泡存在性单独计算，避免尾部替换使稳定块重复计算。 |
| L4 恢复接续 | 固定正文/cursor编译一次，按同一归约器回放在途增量并原子接管；回放缓冲有界。临时IPC失败保留重试，新owner或终态接管与真实失败分开，不累计失败次数、不误清新DOM。 |
| L5 网络诊断 | Android SSE保留HTTP状态码和非空异常原因，不依赖HTTP2空reason phrase，也不额外读取错误正文。 |
| JEV请求预算 | 上限提升为64MiB，计入固定包络、model与JSON转义；先限制进入Rust的正文，再裁成最近连续完整消息，不截半条。最新单条装不下时历史为空；固定元信息本身超限仍报错。计数装配及编码移入blocking任务，正文所有权移动或Arc共享，避免额外全文复制。 |
| JEV抢占与网络回退 | Session持有当前决策子令牌，revision失效取消旧请求及退避；用户硬停关闭会话，过期结果直接重新调度。仅真实网络/超时/可重试HTTP错误耗尽有限重试后按fallback执行；用户已确认top_speaker按当前成员顺序首位选择，stop结束，不借用其他模式配置。 |
| 工具结果DOM | 第一次展开才挂载和解析正文，此后折叠保留DOM；首次挂载通知现有重内容渲染器，数学与图表继续生效。 |
| 新建库事务 | baseline DDL和迁移seed同一事务提交；不新增旧库自动修复或改变迁移校验策略。 |

本地备份IPC新增专用 `ConfigBackupImportResult` 以返回缺实体标志；它不是跨端同步DTO。配置类型由真实导出器生成，没有手改生成产物。相关技术文档同步修订。

### 修复验证结果

| 检查 | 结果 |
| --- | --- |
| `pnpm check` | 最终通过，包含生成契约一致性、ESLint、Vue/构建配置TypeScript和host Rust检查；新增的仅测试wrapper已限定为test编译。 |
| `pnpm test:run --silent --reporter=dot` | 104个文件、905个用例全部通过。 |
| `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib --quiet` | 721个用例全部通过；保留既有host移动语音入口未使用警告。 |
| Android Rust `cargo check --locked --manifest-path src-tauri/Cargo.toml --target aarch64-linux-android` | 使用本机NDK 29.0.14206865、API26编译器环境后通过。直接运行未配置NDK的首次尝试缺少clang，属于构建环境问题，不以该失败推断源码缺陷。 |
| Android JVM专项 | SseProxyProtocolTest 25项、SseProxyConnectionOwnershipTest 7项、RequestBodyFileTest 3项，共35项通过。 |
| 差异与边界复核 | `git diff --check`通过；同步DTO/hash、已有SQL迁移文件未修改；L1共享配置保存链未修改。 |

新增测试覆盖反序输入提交、仲裁抢占、网络回退分类、64MiB完整JSON包络与转义、纯文本历史预算、防抖与在途拖拽、失焦保存关闭、缺群导入与墓碑静默、终态SQL失败后恢复/停止/删除、快照增量回放与接管、工具首次展开以及baseline seed失败整库回滚。首次全套验证揭示的旧惰性挂载断言和新增fixture类型/模型规范化差异均已按真实生产语义修正，未通过放宽业务接口迁就测试。

本轮未安装或操作真机，未执行Apple archive；Android目标静态检查与JVM结果不替代设备体验、性能或iOS运行结论。
