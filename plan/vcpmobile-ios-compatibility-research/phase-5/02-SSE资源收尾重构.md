# SSE 资源收尾重构

> 2026-10-17：已完成。本轮承接统一接管与操作边界重构，只处理已审议的重复分配和资源释放，不改变 iOS 后台机制选型及验收边界。

## 用户决定与范围

- 单条正文规模沿用 LLM 单次生成的物理边界，总量沿用现有 8 路接收并发；对应约定写入 helper 的并发常量、正文缓冲和准入判断旁。不增加单条/总字节/驻留 Session 配额，不引入额外容量拒绝、截断、驱逐、长回复转文件或前端正文卸载。
- Rust 内部网络结果使用明确类型，正文通过所有权移动进入 finalizer；渲染 JSON 直接写入 zstd 编码器，消除完整未压缩 JSON 中间缓冲。
- worker 输入与终态事件的进一步共享优化不实施，也不安排对应验证；Aurora 内部表示不重构。
- 完整 blocks 成功接管前端显示后，释放失效的尾部 AST/更新帧；解析失败或迟到回调不清理仍在使用的展示数据。pending 身份、正文与冲突操作占用继续保留。
- helper 离线发布首次失败后只追加两次自动重试，分别在上次失败返回后等待 2 秒、4 秒；现有 5 秒 ACK 宽限保留。失败结果不自动丢弃，不重发 LLM 请求或完成通知。
- 使用两位子代理分工，完成验证后本地提交，不推送。

## 三方审查

- Melchior：transport 类型化只调整内部所有权，保持实际 IPC 和生成终态语义；共享输入和终态事件优化留在本轮之外。压缩保持 JSON 结构、zstd 读取兼容和现有解压约束。
- Balthasar：清理发生在完整渲染结果接管之后，避免解析失败时清空可见回复；等待重试与停止待确认继续保留操作占用。
- Casper：复用 session 的唯一交接任务，重试次数归 generation。ACK/discard/移除取消任务，resume 替换旧任务但不重置次数；退避期间不持交接 WakeLock，实际落盘继续使用有界交接窗口。

## 实施路径与验证问题

| owner | 变化与保持的契约 | 需要验证的事实 |
|---|---|---|
| Rust 网络与应用服务 | 网络结果正文移动到原 finalizer，原有消费者、取消和收据语义保持 | 流式与非流式结果、恢复、取消及持久终态实际调用链 |
| 渲染持久化 | JSON 直接写入 zstd，仍存储同一消息块结构 | 历史压缩数据可读、新数据可由原读取路径解压 |
| Vue chatStreamStore / MessageRenderer | 完整 blocks 接管后清理尾部字段；失败保留展示与生成活动分别判断 | 终态/partial 成功释放，失败时稳定前缀与原尾部仍可见且不显示生成状态，旧消费者回调不清理新尾部 |
| Android helper | 首次失败后 2 秒、4 秒两次重试；终态释放网络引用 | 次数耗尽保留正文、成功释放、ACK/resume/旧任务隔离、等待期间交接锁释放 |

本轮不使用源码中的对象数量推断固定内存倍数，也不将静态检查、JVM 测试或目标编译解释为设备峰值、Freezer 或 iOS 后台执行证据。

## 实际验证

| 实际执行 | 结果与证据范围 |
|---|---|
| `pnpm check` | 最终生产代码 TypeScript 与 host Rust 检查通过 |
| `pnpm exec vitest run src/tests/unit/chat` | 26 个文件、217 项通过；包含新增尾部释放、迟到回调、完整 blocks 接管与终态渲染失败的组件展示验证 |
| `cargo test --locked --lib vcp_client` | 30 项通过；流式、非流式正文和结束原因、取消、接管及离线结果 |
| `cargo test --locked --lib render_compression_tests` | 1 项通过；历史 bulk 帧与当前 stream 帧均由原读取路径还原相同 JSON，截断帧拒绝 |
| `cargo test --locked --lib group_turn_tests` | 1 项通过；群聊停止与持久正文语义 |
| `cargo test --locked --lib stream_lifecycle_tests` | 9 项通过；持久终态、取消、删除幂等及失败处理 |
| `cargo check --locked --target aarch64-linux-android` | 通过；临时使用现有 NDK 29 的 Android 26 clang/clang++/llvm-ar，未改构建配置 |
| Android `:tauri-plugin-vcp-mobile:compileDebugKotlin` | 通过 |
| Android 定向 `:tauri-plugin-vcp-mobile:testDebugUnitTest` | ownership 7 项、protocol 18 项，共 25 项通过；实际文件故障与 2/4 秒计时、次数耗尽、ACK/discard/新 generation、resume、销毁及网络引用清理 |
| `git diff --check` | 通过 |

本轮没有执行内存峰值测量、Apple 构建或手机 Freezer/长期后台验证。worker 输入与终态事件共享优化保持未实施，也未安排对应测试。主请求租约、持久 finalizer 和快照交付顺序保持原契约。
