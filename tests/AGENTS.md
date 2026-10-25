# AGENTS.md — VCP Mobile Android 验证工具

> 本文件适用于 `tests/**`。它继承仓库根规则，只约束 Android Debug、设备旅程与性能采集脚本的副作用、输出和证据边界；这些工具不定义产品行为。

---

## 1. 定位

`tests/` 提供面向指定 Android Debug 设备和本地构建产物的受控验证工具。脚本选择明确的输入，调用 ADB、Cargo 或本机归档工具，把有限的终端输出或报告写入允许的位置。

```text
`pnpm android:debug:*` ──> android-debug-agent.cjs ──> adb-env.cjs
                                   |                        |
                                   v                        v
                          Debug 会话与有界输出        指定设备 / 固定 Debug 包

`pnpm perf:*` ──> 对应采集脚本 ──> ADB / APK / Cargo 输出 ──> 本地报告
```

## 2. 地图

| 路径 | 唯一职责 |
| ---- | -------- |
| `e2e-android/scripts/adb-env.cjs` | ADB 定位、设备选择、Debug 包常量 |
| `e2e-android/scripts/android-debug-agent.cjs` | `doctor/dev/status/logs/snapshot/database-backup` 等统一入口 |
| `e2e-android/scripts/android-gesture-check.cjs` | 固定 Debug WebView 目标的触屏检查 |
| `perf/scripts/` | APK 体积、冷启动、同步期资源、系统快照与 Criterion 的采集或归档 |

脚本的行为以这些 JavaScript 入口为准；README、历史报告和旧采样值只是材料。

## 3. 不变量

### 3.1 设备与会话

| # | 不变量 | 锚点 | 破坏后果 |
| - | ------ | ---- | -------- |
| T1 | 设备写操作只作用于 `com.vcp.avatar.debug`；不提供包名覆盖、Release 模式或任何操作 `com.vcp.avatar` 的路径 | `adb-env.cjs` | 损坏用户的 Release 数据 |
| T2 | 多设备必须显式选择 serial；选择不唯一、设备状态异常、Dev 使用网络 ADB 一律失败，不猜测目标 | `adb-env.cjs` | 操作到错误的设备 |
| T3 | 安装前读取 APK application id，仅 Debug 包可继续；清除 Debug 数据必须带显式参数 | `android-debug-agent.cjs` | 覆盖 Release 或误清数据 |
| T4 | `dev` 只建立并清理自己拥有的 reverse，同值映射可复用但不冒领，冲突映射不覆盖；`stop` 禁止 `adb reverse --remove-all` | `android-debug-agent.cjs` | 破坏用户的其他 adb 会话 |
| T5 | 日志只读当前 Debug 主进程 PID 并限行数；进程不存在时不回退全局 logcat，任何路径都不清空设备日志 | `android-debug-agent.cjs` | 读到无关日志或毁掉他人现场 |
| T6 | `gesture-check` 只连接指定 USB 设备的 Debug PID socket，使用本次分配的临时 forward；目标仅限固定主题按钮与普通正文，不接受任意选择器、脚本、包名或页面 URL | `android-gesture-check.cjs` | 退化为任意页面的远程执行入口 |
| T7 | 截图只由用户命令或显式参数触发，一次一张，输出限于仓库或系统临时目录 | `android-debug-agent.cjs` | 批量采集个人内容 |
| T8 | `collect_android_dumpsys.cjs` 只在用户明确要求全量诊断时运行 | `perf/scripts/collect_android_dumpsys.cjs` | 日常采集大体量系统信息 |
| T9 | 同步采样绑定当前 Debug PID 与指定设备；进程、serial、包任一不匹配即停止 | `perf/scripts/measure_sync_pipeline.cjs` | 报告混入其他进程的数据 |

### 3.2 证据语义

| 来源 | 只能说明 |
| ---- | -------- |
| `status` / `logs` / `snapshot` / 截图 | 脚本实际读取的字段，不等于完整 E2E 或发布验收 |
| `gesture-check` | CDP 输入与观测序列的回放，输出绑定实际加载的指令与文档身份；不等于手指实测 |
| `measure_startup_adb.cjs` | Android `am start -W`，不是业务就绪时间 |
| `measure_apk_size.cjs` | 文件字节总是可得；内部 section 依赖本机归档工具，缺失即保持缺失，不推算 |
| Criterion、历史基线、已有报告 | 某次运行的产物；新结论须重新绑定当前代码、输入、环境与实际采样 |

## 4. 修改与验证

修改脚本时，从 `package.json` 的命令入口追踪参数解析、设备选择、副作用目标、输出限制与清理路径。新增写设备能力必须同时满足 Debug-only、目标可判定、失败闭合、所有权可回收。

| 改动 | 验证 |
| ---- | ---- |
| 脚本语法、纯解析 | 对应 Node 入口的有界检查 |
| 需要 ADB、安装、重载、截图、性能采集 | 先取得用户对该操作的明确指令 |

**完成**：命令入口、目标设备、Debug 包、副作用所有者、输出预算与清理路径一致，并报告实际执行了什么、生成了什么、哪些设备或产品结论仍未取得。
