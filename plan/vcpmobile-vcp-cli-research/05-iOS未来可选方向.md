# 05｜iOS 未来可选方向

> 本文只记录未来方案，不改变 VCPMobile 当前 Android arm64 产品边界，也不授权新增 iOS 代码。通用平台结论以现有 [VCPMobile iOS 兼容适配专项](../vcpmobile-ios-compatibility-research/README.md) 为准。

## 1. 可复用的不是 Android Runtime，而是上层合同

iOS 未来可以复用：

- `VCPMobileCLI` 工具名、action、VCP block parser 和结果 DTO；
- LocalVcpTurnOwner 的 step/digest/idempotency；
- 规范 manifest 与 VCPToolBox 侧的 Skill/提示词配置；
- Job 状态 `queued/running/waiting_user/completed/failed/timed_out/cancelled/interrupted`；
- UI 的工具块、来源标记、输出 tail、取消与人工终端交互合同。

iOS 不能直接复用：

- Android PRoot 二进制、Alpine rootfs 启动方式和 Foreground Service；
- WakeLock/WifiLock、`:helper` 进程和 Android notification service；
- Root/Shizuku、任意 APK 或 Android shell；
- “划掉任务后服务继续”的 Android 语义。

因此 future adapter 是：

```text
VcpCliProtocol / MobileCliRuntime domain
                 │
          IOSCliProcessHost
                 │
      curated native / WASI / iSH
```

iOS 若无法满足 Android manifest 冻结的 Alpine Bash 合同，就必须发布平台专属 manifest 版本并清楚列出差异；不能靠同一个工具名暗示二进制与命令集相同，也不由 Mobile 注入临时提示词修补。

## 2. 方案 A：受控原生命令 + WASI（首选低风险基线）

只实现明确需要的文件、HTTP、Git 子集、压缩、文本处理和设备能力 handler；可移植纯计算命令使用 WASI/Wasm。它最容易保持：

- App container 与用户明确选择的 Files scope；
- 每条命令的 CPU/内存/输出预算；
- 无 Root、无任意 host syscall；
- 许可证和包体可枚举；
- App Review 可解释。

缺点是不能承诺完整 Linux，`fork`、socket、动态扩展和复杂包管理会受限。产品应称“受控本地 CLI”，不是“完整 Alpine”。

这是首个 iOS CLI 实验最推荐的路线：协议兼容优先于二进制兼容。

## 3. 方案 B：a-Shell / ios_system 风格

[a-Shell](https://github.com/holzschu/a-shell) 把许多命令编译进 App，通过 `ios_system` 执行，也支持把 C/C++ 编译为 WebAssembly。其公开说明明确：Wasm 命令没有 sockets 和 fork，Python 只能安装纯 Python 扩展等受限包。

优点：

- 已有 Unix-like 命令与交互终端范式；
- BSD-3-Clause 主仓库许可相对宽松；
- 比完整 Linux 模拟器小、边界更清楚。

风险：

- 每个打包命令及依赖仍需逐项审计；
- Wasm/ios_system 的进程、网络和扩展能力与 Android 不同；
- 不能把 `pkg/pip` 营销成任意动态功能安装；
- 与 Tauri/Rust/Swift 的 PTY、事件和取消桥接需专项 spike。

结论：适合“较丰富的受控命令集”，不适合作为完整 VCP CLI 等价承诺。

## 4. 方案 C：内嵌 iSH 用户态 Linux（高兼容、高风险）

OpenMinis iOS 当前采用 iSH 路线：每次 Agent 命令在同一 guest 内核中建立独立 `/bin/sh` 进程，文件系统和挂载持久；超时/取消会按进程组/子孙做 TERM→KILL。相比其 Android 长期 shell，这个取消模型反而更接近本专项要求。

- [`ISHExecutionCoordinator.swift@9cf3a85`](https://github.com/OpenMinis/OpenMinis/blob/9cf3a855fecd27bb5735b84cacbd56852a3ab8dd/src/ios/Agent/ISH/ISHExecutionCoordinator.swift#L26-L38)
- [`ISHExecutionCoordinator.swift@9cf3a85` 取消](https://github.com/OpenMinis/OpenMinis/blob/9cf3a855fecd27bb5735b84cacbd56852a3ab8dd/src/ios/Agent/ISH/ISHExecutionCoordinator.swift#L208-L226)
- [`ISHShellExecutor.m@9cf3a85` 超时与 TERM→KILL](https://github.com/OpenMinis/OpenMinis/blob/9cf3a855fecd27bb5735b84cacbd56852a3ab8dd/src/ios/iSH/ISHShellExecutor.m#L507-L628)

它的优势是 Linux 用户态兼容、可运行更多现成 CLI、与 Android 的命令环境较接近。代价是：

- 模拟内核、syscall translation、rootfs 和调试面复杂；
- 包体、启动、内存、耗电和攻击面显著增加；
- [OpenMinis 因 iSH/PRoot 组合整体使用 GPLv3](https://github.com/OpenMinis/OpenMinis/blob/9cf3a855fecd27bb5735b84cacbd56852a3ab8dd/README.md#L196-L203)，不能当成普通 Swift 文件复制；
- 动态下载和执行代码的 App Review 论证更困难。

结论：只进入独立 iSH 可行性专项。未完成许可证、包体、真机和审核路线前，对首个 iOS CLI 判 `NO-GO`。

## 5. 方案 D：真实远端 VCP 插件

对 iOS 本地无法运行的二进制或超长工作，用户可以显式选择 `vcp_plugin`，把命令派到其自有 VCP 节点。它复用本专项协议，却仍受网络、VCPToolBox 在线和远端设备安全边界影响。

远端只能是可选 route，不能成为 iOS 本地 CLI 的默认生存条件；否则又回到本专项要解决的手机弱连接困境。

## 6. iOS 后台工作的可信边界

### 6.1 iOS/iPadOS 26+

Apple 的 [`BGContinuedProcessingTask`](https://developer.apple.com/documentation/backgroundtasks/performing-long-running-tasks-on-ios-and-ipados) 面向用户明确发起、希望立即开始并持续数分钟以上的工作，可继续使用 CPU/网络并由系统展示进度和取消入口。

它适合有明确 Job、真实进度/状态和 expiration handler 的非交互 CLI；不适合无限 PTY、无目标 daemon 或伪造进度的 shell。系统仍可拒绝、取消或因资源压力终止，因此：

- 任务开始前持久化 Job/attempt；
- expiration 先失效 owner，再杀进程组并写 `interrupted`；
- 用户从系统 UI 取消映射到同一 cancel owner；
- 完成时 exact-once 结束系统 task；
- 不能承诺系统一定接纳或持续到完成。

### 6.2 iOS 25 及以下

`UIApplication.beginBackgroundTask` 只给有限收尾时间。进入后台后应完成短清理或终止目标进程组，把状态写成 `interrupted`；回到前台由用户显式重新发起。它不是常驻服务。

文件上传/下载交给 background `URLSession`，不要用 shell keepalive 模拟。

### 6.3 保活策略

不得借鉴静音音频、虚假定位或其他与任务真实用途无关的后台模式。只能使用 Apple 为实际工作类型公开提供的能力，并把系统拒绝、到期或终止如实写为 `interrupted`。


## 7. 动态代码与 App Review

Apple [Guideline 2.5.2](https://developer.apple.com/app-store/review/guidelines/) 要求 App 自包含，通常不得下载、安装或执行会引入/改变功能的代码；教育类可执行代码只有有限例外并要求源码对用户可见可编辑。

因此：

- 完整 Alpine + `apk add`、Node/npm、pip native wheel 不能在计划中默认视为可上架；
- Wasm、脚本和 Skill 是“数据还是改变 App 功能的代码”需结合产品用途与发行方式单独评审；
- OpenMinis/iSH/a-Shell 已存在不等于 VCPMobile 必然通过同样审核；
- 实验性自签与 App Store 是两条不同发行合同，文档必须明确。

## 8. 推荐顺序

| 顺序 | 路线 | 裁决 |
|---:|---|---|
| 1 | 统一 VCP 协议 + 受控原生命令/WASI | `RECOMMENDED FOR SPIKE` |
| 2 | a-Shell/ios_system 子集 | `OPTIONAL RESEARCH` |
| 3 | iSH 完整用户态 Linux | `SEPARATE HIGH-RISK STUDY` |
| 4 | 真实远端 VCP | `OPTIONAL FALLBACK` |

未来 iOS 开工前需先复用并更新 [iOS 专项的生命周期门禁](../vcpmobile-ios-compatibility-research/04-生命周期后台网络与同步策略.md)，再新增 CLI 能力矩阵；不得为了“共用代码”给当前 Android 产品加入 iOS 业务分支。
