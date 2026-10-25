# AGENTS.md — VCP Mobile 移动端原生插件

> 本文件适用于 `src-tauri/plugins/vcp-mobile/**`。它继承仓库根规则，只补充移动端原生能力边界内的代码结构、通信路径与修改闭环。

---

## 1. 定位与调用链

`tauri-plugin-vcp-mobile` 实现 VCP Mobile 的 Android 与 iOS 原生能力。它承接 Vue 与 Rust 核心的设备意图，通过 JNI 或 Swift 桥接调度移动端原生服务，并将硬件事件与执行结果送回调用方。

```text
Guest JS 包装 ─┐
Vue 直接 invoke ┴─> Rust 注册命令 ─┐
Rust 核心内部调用 ─> Rust 公开入口 ─┼─> PluginHandle / JNI / Swift
                                    v
                 VcpMobilePlugin ─> Manager / Service / Native Bridge
                                    |
                                    └─> Result / Channel / CustomEvent
```

---

## 2. 代码地图

| 路径 | 唯一职责 |
| ---- | -------- |
| `src/lib.rs`、`src/events.rs` | 插件核心组合、命令注册、事件 Channel 管理与底层状态注入 |
| `src/screen.rs`、`stream.rs`、`system.rs`、`voice.rs` | Rust 侧按功能划分的原生调用封装与生命周期租约 |
| `android/src/main/` | Android 原生 Kotlin 实现：按功能分包管理权限、前台服务、媒体、语音与系统能力 |
| `ios/Sources/` | iOS Swift 实现：按领域目录组织生命周期、权限通知、媒体转换、语音流与分享通道 |
| `permissions/`、`build.rs` | Tauri v2 插件权限配置 (`default.toml`)、代码生成与能力 schema |
| `guest-js/` | 提供被显式导出的前端 TypeScript 包级包装函数 |

---

## 3. 核心不变量

| # | 不变量 / 约束 | 锚点 | 破坏后果 |
| - | ------------- | ---- | -------- |
| P1 | 前端命令必须在 `src/lib.rs` 注册、`permissions/` 授权并在原生层实现；仅声明 `@Command` 不等于对前端开放 | `src/lib.rs` | 前端 invoke 遭遇未授权拒绝或命令找不到错误 |
| P2 | 原生耗时操作（媒体转换、录音编码、文件 IO、授权等待）必须调度到后台工作线程，严禁阻塞 UI 主线程或 JNI 调用栈 | `android/src/main/` | 引发 Android ANR (Application Not Responding) 导致系统强杀应用 |
| P3 | 前台服务、录音采集与解码资源必须由所属 Native Manager 独占拥有，操作取消必须等待硬件或系统真实终态 | `src/voice.rs` | 产生悬挂前台通知、麦克风占用泄漏或音频输出冲突 |
| P4 | Android 与 iOS 原生分支显式隔离，未实现的原生调用必须明确返回不可用错误，禁止返回伪成功假数据 | `src/lib.rs` | 掩盖真实平台未实现缺陷，导致上层状态机进入无效等待 |
| P5 | iOS UIKit 操作与系统视图展示必须在主队列执行，文件呈现与通知跳转必须绑定活跃 Scene | `ios/Sources/` | 触发 iOS 主线程违规断言闪退，或呈现位置异常引发崩溃 |
| P6 | 原生事件 Channel 订阅在运行时保持稳定，WebView 刷新重载时复用现有通道更新世代，禁止频繁重复创建 | `src/events.rs` | 原生事件注册表溢出或产生重复监听与内存泄漏 |

---

## 4. 原生调用与权限边界

- **内部调用 vs 前端命令**: Rust 核心直接通过 `PluginHandle` 或内部函数调用的能力，不得无意义包装为前端命令。
- **能力授权 (Capabilities)**: 插件命令的前端访问权限按平台 capability 文件逐项授予，非生产窗口不配置多余权限。
- **生命周期对齐**: 插件随宿主进程启动与销毁，系统配置变更或后台休眠时必须妥善暂停硬件采样与保活服务。

---

## 5. 修改与验证

| 修改范围 | 验证命令 |
| -------- | -------- |
| Rust 插件边界与命令声明 | 在 `src-tauri/` 运行 `cargo check --locked -p tauri-plugin-vcp-mobile` |
| Android Kotlin、Manifest 或前台服务 | 在 `src-tauri/gen/android/` 运行 `./gradlew :tauri-plugin-vcp-mobile:testDebugUnitTest` |
| 跨前端调用与 TypeScript 类型定义 | 在仓库根目录运行 `pnpm check` |

**完成判定**：前端命令、权限声明与平台实现三位一体对齐，后台资源有界释放，多端编译与类型检查无报错。
