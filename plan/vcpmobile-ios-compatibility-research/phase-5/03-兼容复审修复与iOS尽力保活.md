# 兼容复审修复与 iOS 尽力保活

日期：2026-10-17。施工基线：`16558bb0`。本记录区分用户决定、源码实施及尚未执行的验证，不以源码审查替代目标编译或后台运行结果。

## 1. 本轮决定与范围

- 用户接受 iOS SSE 原生循环静音音频的尽力保活及系统中断，不再要求严格后台不断流。
- 常亮维持当前任务语义：仅 Sync 和 Prerender；聊天 Stream 不常亮。不增加手动常亮开关。
- 六项兼容复审问题统一修复；用户要求提交，检查、测试及构建后续再安排。本轮不执行 `pnpm check`、单元测试、Cargo/Gradle/Swift 编译或 Apple Actions。
- Android helper 的 5 秒 ACK、仅两次落盘重试及既定容量决定不变。worker 输入和终态事件共享优化不实施。
- 无结束标记的 SSE EOF 保留当前兼容行为，未借本轮修复收严上游协议。
- Phase 6 用户安装包、App Group/扩展签名身份改写及最终验收继续留在其阶段，不执行 push 或发布。

实施前按 Magi 三个角度收敛方案：Melchior 负责附件身份、持久化与 GC；Balthasar 负责输入、复制和浮层的现有交互 owner；Casper 负责原生资源生命周期及尽力后台边界。第四位代理负责选择器取消接纳与图片缓存恢复；根代理整合、复审和文档维护。没有建立额外业务状态总线。

## 2. 六项兼容修复

| 问题 | 当前实现与边界 |
|---|---|
| 容器绝对路径变化导致读取失败和 GC 误删 | `file_manager` 以受校验的 hash/文件名结合当前附件根定位；旧路径只提供文件身份，不决定访问根。bootstrap 在 GC 前重投影本地目录、消息关系和当前分享记录；迁移失败跳过 GC。GC 按索引及存活引用 hash 保护原件，不能仅因旧绝对地址不匹配而删除。同步写队列显式接收当前附件根，缺失原件不清空可恢复文件身份。 |
| 输入法确认键误触发提交 | 论坛回复、Prompt、日记重命名、任务手动添加及主聊天快捷键在处理 Enter/Escape 前检查 composition、`isComposing` 和 `keyCode=229`；只对真正的提交键阻止默认行为。 |
| 历史全文复制跨 IPC 丢失 WebKit 用户激活 | iOS 点击时同步发起 `clipboard.write`，正文通过 `ClipboardItem` 的 Promise 交付；现有正文同步 `writeText`。保留 Android 路径，取回后核对消息身份，读取/写入失败提供真实反馈，不以渲染摘要替代全文。 |
| 缩略图缓存缺失后持续破图 | 历史附件路径投影将失效缓存视为可重建派生；图片组件在缓存加载失败时每个资源身份最多请求一次现有有界派生。切换、卸载和迟到结果释放派生资源；派生再失败显示占位，不循环重试。 |
| 取消先于原生 picker 创建而丢失 | iOS 先 `prepare_file_operation` 登记实际调用 WebView/Scene，再以返回 token 启动 `pick_file`。前端共用 `nativeFilePicker` 在首个 await 前记录取消意图；prepare 期间取消不再启动，start 之前到达的原生取消撤销 reservation。原生仅保留一个当前 reservation，无时间淘汰的取消墓碑。 |
| 附件查看/头像裁剪等浮层缺少键盘焦点边界 | AttachmentViewer、AvatarCropper、TarvenSelector、GroupModeHelpDialog 将实际根节点接入现有 modal owner，沿原历史栈处理焦点、返回和关闭，不新增焦点状态机。 |

路径重投影不改变消息时间或同步 fingerprint。单个附件不可用不应阻断整条消息或话题正文；原始 CAS 与可清理的缩略图缓存分开处理。持久字段保留现有存储格式，但所有本地定位使用当前根，不再将数据库中的旧绝对根作为事实。

历史代码若已把 `internal_path` 清成空值，本轮不遍历目录猜测文件名；GC 仍按 hash 保全可能存在的原件，正常重新登记可恢复本地定位。已经被旧版本删除的文件不能由路径修复凭空恢复。

原生选择器的真实消费者为附件单/多选、相机、头像和配置导入。Android 仍沿现有单次原生选择路径，Android-only CLI 保留原入口。选择结果交付后的 CAS 登记、失效消费者判断和临时文件清理由原有 owner 承担。

## 3. iOS 任务资源

`ForegroundLease` 继续由 Rust 任务持有；Swift `NativeBackgroundExecution` 为进程级单例，在主队列管理租约和系统对象，WebView 替换不重置它。

| 任务 | 音频 | 前台常亮 | 释放时点 |
|---|---|---|---|
| Stream 聊天请求 | 请求尽力播放支持 | 否 | 原请求的生成、取消及持久收尾退出后释放自身租约 |
| Sync | 不主动请求 | 是 | 同步会话及已接受写入实际收尾后释放 |
| Prerender | 不主动请求 | 是 | 缓存编译与写入任务完成或错误收尾后释放；无工作不申请 |

音频使用 `.playback`、`.mixWithOthers` 和 `AVAudioPlayer`，循环播放内存生成的一秒单声道 PCM 静音，不读取麦克风、不增加音频文件 staging。Info.plist 只增加实际使用的 `audio` 后台模式，不堆叠其他后台模式。多条 Stream 共用播放器；最后一个音频租约释放后停止播放并撤销自己的音频激活。

正常音频中断只暂停并记录，不伪造聊天错误或重新发送生成请求。中断结束后按系统恢复许可及仍存活的租约处理；媒体服务重置后等待新的前台聊天任务，不在任意 active 通知上自动复播。实际获取/复播失败沿已有 guardian 通知链告知；未交付的原生失败通过已有 Channel 注册时重放，不新建通道或复制业务终态。常亮只在前台生效，最后一个常亮租约结束后恢复 idle timer。

这些是自签实验的资源尝试，不保证系统持续调度、锁屏网络不断流或进程被杀后继续接收。进程内没有第二个 iOS helper，也没有额外正文缓存。

## 4. 源码依据与后续证据

- 文件容器变化和缓存可删除性：[Apple TN2285](https://developer.apple.com/library/archive/technotes/tn2285/index.html)、[文件系统目录用途](https://developer.apple.com/documentation/foundation/using-the-file-system-effectively)。
- WebKit 用户激活与异步复制：[WebKit Clipboard](https://webkit.org/blog/10855/async-clipboard-api/)。
- 音频配置与恢复：[playback](https://developer.apple.com/documentation/avfaudio/avaudiosession/category-swift.struct/playback)、[音频中断](https://developer.apple.com/documentation/avfaudio/handling-audio-interruptions)、[媒体服务重置](https://developer.apple.com/documentation/avfaudio/avaudiosession/mediaserviceswereresetnotification)。
- 前台常亮：[isIdleTimerDisabled](https://developer.apple.com/documentation/uikit/uiapplication/isidletimerdisabled)。

本轮完成调用链与差异的人工源码复审，包括原生 selector/DTO、插件注册/权限、旧 WebView、取消、单附件故障及最后租约释放。Git 差异检查只说明提交内容和格式，不记作静态编译或行为验证。

当前唯一既有 Apple 诊断为 [2026-10-16 Phase 3/debug](https://github.com/MRiecy/VCPMobile/actions/runs/38884164944)，对应旧提交 `a8482b60`，实际 Xcode 16.4 / SDK 18.5。固定 Xcode 26.2 后的 Phase 4/5 及本轮源码仍待检查和目标构建，不能复用旧归档为通过证据。项目仍不建设 iOS 实机测试体系，也没有本轮真实后台效果数据。
