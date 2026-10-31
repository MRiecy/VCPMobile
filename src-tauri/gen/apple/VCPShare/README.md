# Share Extension 的输入与交付

本目录是独立的 iOS 分享扩展 target。主应用将扩展嵌入 `PlugIns/VCPShare.appex`。扩展使用系统分享接口接收数据，经 App Group 容器交给主应用，原生代码分别负责以下步骤。

- `ShareViewController.swift`：原生界面、系统输入与 `extensionContext` 完成/取消回传。
- `ShareAcquisition.swift`：provider 获取、有界复制、取消、在途读取排空与唯一终态裁决；异步工作持有自身，UI 回调弱引用控制器。
- `ShareInboxStorage.swift`：delivery 身份、槽锁、文件租约及原子清单写入；不持有另一套业务终态。

成功、错误和取消先进入同一个终态入口，读取排空后才领取提交权。此前的取消优先；提交开始后的迟到取消不再覆写 `ready`，也不提前结束扩展。提交失败仍发布失败结果，写入收尾后释放租约。状态锁只保护短状态裁决，不跨 `flock`、文件写入或 `fsync`。

扩展自己的 `PrivacyInfo.xcprivacy` 随 target 复制，磁盘容量调用仅用于复制前/期间检查可写空间，声明 `NSPrivacyAccessedAPICategoryDiskSpace / E174.1`。

用户从系统分享中保存内容，再自行打开 VCPMobile 选择助手。扩展不尝试通过私有路径唤起主 App。接受分享时即覆盖旧槽位，新分享失败或取消也不会恢复旧分享。一次可包含 16 项，文件总量 512 MiB、文本 UTF-8 1 MiB、共享暂存总量 1 GiB，复制保留至少 64 MiB 可用空间；这些是本轮资源预算，可在实测后调整。复制每次只读取 64 KiB，不解码图片。

主 App 与扩展共享 `ShareInbox/current.json` 和 `current.lock`。`flock` 只保护身份裁定与短提交；文件复制在锁外。每个文件开始和结束时检查共享身份，64 KiB 读取循环仅检查本地取消与资源预算，不逐块加跨进程锁或解析槽位 JSON；最终发布仍在槽锁内核验身份。每个 delivery 目录以 `lease.lock` 保护在途 provider 或 Rust 导入。当前 ready manifest 记录相对 itemId、原名、MIME、长度和 SHA-256。主 App 验证后复制到 CAS；SQLite `current_share` 只保存一条记录。分享首条用户消息、附件关系与 sent 状态在现有消息仓储的同一写事务提交。

Darwin 通知只提示重新读取当前槽位；启动/READY/回前台也会对账。进程终止后，未被持有的 receiving lease 表示获取已中断，显示失败；ready/draft 从本次原件恢复。终态先于文件校验处理，重复发送返回原 message ID。旧目录是待清理副本，不是分享队列。

工程归档、安装者自签和两个 target 的 App Group 配置见 [iOS 构建说明](../../../../docs/IOS_BUILD.md)。
