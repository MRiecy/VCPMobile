# Share Extension 工程与交接输入

`g1.py overlay` 将本目录复制至生成树的 `VCPShare/`，添加独立 iOS 26.0 `app-extension` target，并由主 target 以 embed dependency 嵌入 `PlugIns/VCPShare.appex`。扩展只编译 `ShareViewController.swift`，不链接 Tauri 主插件；`APPLICATION_EXTENSION_API_ONLY=YES`。版本读取当前 Tauri 配置，默认 `CFBundleVersion` 与产品版本一致。

扩展自己的 `PrivacyInfo.xcprivacy` 随 target 复制，磁盘容量调用仅用于复制前/期间检查可写空间，声明 `NSPrivacyAccessedAPICategoryDiskSpace / E174.1`。

用户从系统分享中保存内容，再自行打开 VCPMobile 选择助手。扩展不尝试通过私有路径唤起主 App。接受分享时即覆盖旧槽位，新分享失败或取消也不会恢复旧分享。一次可包含 16 项，文件总量 512 MiB、文本 UTF-8 1 MiB、共享暂存总量 1 GiB，复制保留至少 64 MiB 可用空间；这些是本轮资源预算，可在实测后调整。复制每次只读取 64 KiB，不解码图片。

主 App 与扩展共享 `ShareInbox/current.json` 和 `current.lock`。`flock` 只保护身份裁定与短提交；文件复制在锁外。每个 delivery 目录以 `lease.lock` 保护在途 provider 或 Rust 导入。当前 ready manifest 记录相对 itemId、原名、MIME、长度和 SHA-256。主 App 验证后复制到 CAS；SQLite `current_share` 只保存一条记录。分享首条用户消息、附件关系与 sent 状态在现有消息仓储的同一写事务提交。

Darwin 通知只提示重新读取当前槽位；启动/READY/回前台也会对账。进程终止后，未被持有的 receiving lease 表示获取已中断，显示失败；ready/draft 从本次原件恢复。终态先于文件校验处理，重复发送返回原 message ID。旧目录是待清理副本，不是分享队列。

Phase 6 重签时需要一起核对：

- 主 bundle ID：`com.vcp.avatar`；扩展 bundle ID：`com.vcp.avatar.share`。
- 两个 target 的 `com.apple.security.application-groups`：`group.com.vcp.avatar`。
- 主 `Info.ios.plist` 与扩展 `Info.plist` 的 `VCPShareAppGroup` 必须同步改写；修改组名时，两份最终 entitlement/profile 也必须一致。
- 保留嵌入的 `.appex`、扩展入口类和 `com.apple.share-services` activation rule；不能只签主可执行文件。
- 主/扩展版本与最低 iOS 26.0 一致。G1 会核对实际 archive 中的嵌入位置和 Info 字段。

无签 archive 只能证明工程编译/嵌入结构。App Group 授权、重签工具是否保留扩展、系统分享列表、provider 与真实用户交互仍取决于最终安装；本项目不建立 iOS 实机测试体系。
