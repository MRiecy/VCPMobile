# Phase 2：验收与 Phase 3 交接

> **2026-10-09 更新：**第 1、4、5 节是当前交接，第 2、3 节明确保留首次施工的历史证据。R1 已通过精简 G1 处理，R2/R3 已按 11–13 实施；全局能力查询/四轴表已撤销。阶段仍缺真实 G1 结果，不以初次“源码完成”声明关闭验收。

> 2026-10-08。**阶段二源码接入、工程输入和本地验证已完成；G1 Apple 构建仍待手动执行，阶段验收尚未关闭。** 施工基线 `aff70257`。阶段二与后续 Tauri 2.12.1 升级按用户授权一并本地提交，未推送或触发 Actions；下表保留升级前的阶段验证记录。

## 1. 交付结果

- Swift 静态 package、FFI binding、`ios_path`、真实 iOS plugin handle 与唯一事件注册 selector 已连接。
- 平台身份改用 OS 插件同步信息；全局能力表/Swift 查询/前端 Store 和通用 Rust 能力门禁已删除。获批的通知、设备与 Distributed 原生实现交 Phase 3。
- Scene/App phase、NWPath Channel、Rust native sequence/epoch 和 Vue 恢复消费贯通。iOS 初始相位未知时不提前恢复，保留 inactive 与 background 的区别。
- 唯一原生注册独立等待真实结果、保留一个底层 receiver；外层取消不丢接收，真实失败由 live/snapshot 重放，新 WebView 可获知。
- 单 Scene、隐私和图标仍有可重放输入；minimum 已按用户后续 D4 改为 iOS/iPadOS 26.0，本阶段旧 15.0 为历史输入。G1 直接执行正常工具链，集中 resolution/shim 已删除。
- 官方 tao 0.37.1 替代旧 vendor；app `ios_window` 接入单窗口恢复，不重复建立 Rust 核心或原生 Channel。

入口与资源覆盖见 [01](./01-生产入口覆盖记录.md)，实现细节见 02–06。源码完成不代表已有可安装或可签名的 iOS 产物。

## 2. 首次施工的历史验证（升级及后续重构前）

| 检查 | 实际结果 | 能说明什么 |
|---|---|---|
| `pnpm check` | 最终收尾退出 0，Rust host 无警告 | Vue/TypeScript 与 Rust host 静态检查 |
| `pnpm test:run` | 60 文件、505 项通过 | 共享前端回归；此后补充首次 null 相位门禁，定向 7 项再次通过 |
| Rust plugin `cargo test --locked -p tauri-plugin-vcp-mobile --lib` | 12 项通过 | 含 DTO、调用方取消、超时、迟到完成和在途上限 |
| Rust app `cargo test --locked --lib lifecycle_` | 12 项通过 | native 顺序/快照、Android linger、原 Chat 生命周期回归及 lifecycle capability 映射 |
| Rust app `cargo test --locked --lib platform::tests` | 4 项通过 | iOS lifecycle 真实观测映射不启用其他 adapter/设备遥测 |
| Android `cargo check --locked --offline --target aarch64-linux-android` | 退出 0，25.11 秒 | NDK 29.0.14206865、API 26、目标分支检查；没有构建 APK 或运行设备 |
| G1 source input check | 通过 | minimum、Scene、privacy、resolution、18 个 icon slot 尺寸 |
| G1 Python 行为检查 | 8 项通过 | overlay 重放/边界、未知模板/依赖漂移拒绝、已知 opener 旧锁迁移 |
| workflow YAML 解析 | 通过；仅 `workflow_dispatch`、`contents: read` | 结构与触发边界，不等同于 Actions 执行 |
| 公开命令/权限抽取 | 42 plugin / 176 app / 59 Kotlin；九组权限无差异 | 旧 Kotlin-only 抽取器对新增 Swift selector 报 1 项 unmatched 并退出 1；已按真实 Swift producer核对，详见 01 |
| tao 原始源对比 | 115 个未修改文件逐字节一致，3 个 iOS 文件有审查过的差异 | 本地依赖副本的变更边界；不证明 Apple 类型/链接已通过 |
| 原有工作树内容 | 9 个路径 SHA-256 全部保持 | package、pnpm lock、app build.rs、六个 permission 文件未被本任务修改 |
| 补丁空白检查 | `git diff --check` 通过；22 个新增自有文本文件无空白/末尾换行问题 | 未为格式检查改写 vendor 原始内容 |
| 文档相对链接 | 11 份本轮文档、37 个相对链接全部有效 | 阶段文档、工程 README 与 vendor 补丁说明的本地目标 |

Android 检查中出现一条 vendored tao `window.rs::hit_test` dead_code 警告：该函数来自未修改的上游源码。Cargo 将 path dependency 作为本地包后暴露该警告；没有为了消除它修改 Android/通用源。

未执行 Kotlin/Gradle 编译，因为本阶段没有修改 Kotlin、Android Manifest 或 Gradle 输入；取得的是 Android Rust 目标证据。未进行 Apple 编译、Simulator/设备操作、签名或发布。

## 3. 初次审查的历史处理（非当前待办）

1. **tao 单 Scene**：把“采用 Scene 生命周期”与“允许多个 Scene”分开，`UIApplicationSupportsMultipleScenes=false` 仍保留；首窗口创建继续等待 Scene connect。
2. **delegate 返回对象**：用 `Retained::autorelease_ptr` 保留返回的 UISceneConfiguration 至调用者取得它，避免原局部 Retained drop 后悬垂。
3. **迟到原生响应**：独立接收任务加两个有界槽位；测试确认调用方超时/abort 后底层发送仍成功。
4. **启动恢复竞态**：Swift lifecycle 回调虽然先入 Rust，但状态归约在线程池排队；network true 可能先到 Vue。iOS 在真实 active snapshot 前禁止恢复，消除这个窗口。
5. **SwiftPM 独立图**：VCP exact dependency 不能锁住 Tauri/opener 各自的 swift build。G1 为三个 root 设置受控 resolution，并校验实际 checkout revision。
6. **opener 自带旧锁**：锁定 opener 2.5.4 带 SwiftRs 1.0.6 的 v1 lock。只对包目录版本和完整 SHA-256 都匹配的上游输入迁移到受控 1.0.7，保存旧输入证据；未知漂移仍拒绝。
7. **签名输入名**：按当前 CLI 的真实 `IOS_CERTIFICATE`、`IOS_CERTIFICATE_PASSWORD`、`IOS_MOBILE_PROVISION` 读取点清理 unsigned G1 子进程环境，避免误读无关运行环境。

## 4. G1 尚待执行

当时已提供手动工作流和工程说明，本轮没有推送或触发 Apple 构建。当前维护入口见 [iOS 构建说明](../../../docs/IOS_BUILD.md)。

G1 提供 Swift/Objective-C 类型与 ABI、iOS Rust 编译、实际链接/配置/资源的工具诊断和 device archive，不再检查人工三 root 集合。产物审查与编译结果分开，build-only 不能证明系统回调已经执行；具体诊断按真实 owner 修复。

## 5. Phase 3 可使用的输入与仍需完成的工作

| Phase 3 输入 | 已有 owner/行为 | 下一具体任务 |
|---|---|---|
| 原生 lifecycle | Rust LifecycleState、transition epoch、nullable snapshot | 按 active/inactive/background 将 Chat/Sync cancel/stop/join/flush 接在原领域 owner |
| 前台判断 | iOS 真实 background 更新现有 is_foreground | 回扫日志缓冲与回前台 flush；不复制 Android linger |
| 网络恢复 | 原 native Channel → Vue 同一恢复入口 | 回扫服务器连接恢复、错误重试与 trusted LAN 策略；NWPath 不代表服务器可达 |
| SQLite/Settings/Diary/CAS | Phase 1 目录边界及现有业务 owner | 验证初始化、持久化、迁移、错误与数据提交点；SQLite 不启用新动态扩展路径 |
| Sync | iOS 仍无 Phase 5 后台租约 | 先实现前台有界流程，并修正当前 `is_android` 不能代表所有移动平台的 loopback 策略 |
| native request/Channel | 唯一事件注册有界接收，bootstrap 单次订阅，真实失败进程内保留 | 后续长任务沿真实 owner 定义 generation/terminal；前端重试不重装 Channel，不新增无界注册 |
| 平台身份与权限 UI | OS 插件同步身份；Gate 共用外壳，当前 iOS 尚无实际权限项目 | Phase 3 接入真实权限检查/请求/拒绝/返回刷新，不恢复能力查询或虚构 Android 同名权限 |
| 通知、设备与 Distributed | 通知 producer 和设备 adapter 未实现；Distributed 可浏览/保存配置但尚不运行 | 按 Phase 3 P3-3 至 P3-5 接通实际结果、部分数据缺失、通知点击和节点清理；Root/CLI 无法迁移时逐工具说明 |
| 文件媒体与双向分享 | HTML 子路径、现有 CAS 及共享选择器 owner 可复用，原生获取/分享未接入 | Phase 4 接拍照/相册保存/Files/预览导出/双向分享/缓存；Extension 与自签输入交 Phase 6 |
| 工程 | 现行 iOS/iPadOS 26.0、受控单 Scene 与 G1；原 15.0 只保留在历史证据 | 保存实际 Apple 诊断；G2 集成打包仍在 Phase 6 |

平台身份不表示已获系统授权，前端 bootstrap 不再查询能力。首个真实 phase、系统权限、任务终态仍各由实际 owner 决定；后台 SSE 的严格目标和空音频候选归 Phase 5，不能把“尽力后台”文案当作该目标完成。

后续独立源码任务可按依赖推进，但阶段二 G1 必须保持“待执行”，直到获得真实 Actions 结果。
