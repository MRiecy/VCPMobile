# AGENTS.md — VCP Mobile Android 验证工具

> 本文件适用于 `tests/**`。它继承仓库根规则，只约束 Android Debug、设备旅程与性能采集脚本的副作用、输出和证据边界；这些工具不定义产品行为。

---

## 1. 子系统定位

`tests/` 提供面向指定 Android Debug 设备和本地构建产物的受控验证工具。脚本选择明确的输入，调用 ADB、Cargo 或本机归档工具，并把有限终端输出或报告写入允许的位置；结果只说明本次实际采集到的状态。

### 1.1 正常执行链

```text
`pnpm android:debug:*` ──> android-debug-agent.cjs ──> adb-env.cjs
                                  |                        |
                                  v                        v
                         Debug 会话与有界输出        指定设备 / 固定 Debug 包

`pnpm perf:*` ──> 对应采集脚本 ──> ADB / APK / Cargo 输出 ──> 本地报告
```

---

## 2. 代码结构

```
tests/
├── e2e-android/scripts/
│   ├── adb-env.cjs                 # ADB 定位、设备选择与 Debug 包常量
│   └── android-debug-agent.cjs     # doctor/dev/status/logs/snapshot/database-backup 等统一入口
└── perf/scripts/
    ├── measure_apk_size.cjs        # APK 文件与可读取 section 的体积报告
    ├── measure_startup_adb.cjs     # adb am start -W 冷启动采样
    ├── measure_sync_pipeline.cjs   # Debug PID 的同步期资源与有限日志采样
    ├── collect_android_dumpsys.cjs # 显式全量 Android 系统快照
    └── run_rust_bench.cjs          # 运行或归档现有 Criterion 输出
```

脚本当前行为必须从上述 JavaScript 入口恢复。历史报告、README、测试名称和旧采样值只能作为材料，不能决定新的产品阈值或实现方向。

---

## 3. Android Debug 安全边界

### 3.1 设备与包

- 所有设备写操作固定作用于 `com.vcp.avatar.debug`；不得增加包名覆盖、Release 模式或任何操作 `com.vcp.avatar` 的路径。
- 多设备时必须显式选择 serial；选择不唯一、设备状态异常或 Dev 使用网络 ADB 时直接失败，不猜测目标。
- 安装前先读取 APK application id，只有匹配 Debug 包才允许继续；清除 Debug 数据仍需显式参数。

### 3.2 ADB 与会话所有权

- `dev` 只建立并清理本次 supervisor 实际拥有的 reverse；已有同值映射可以复用但不能冒领，冲突映射不能覆盖。
- `stop` 只收敛当前 supervisor 和自有映射；禁止执行全局 `adb reverse --remove-all`。
- 日志只读取当前 Debug 主进程 PID，并保持行数上限；进程不存在时不回退到全局 logcat，任何路径都不得清空设备日志。
- 截图必须由用户命令或显式参数触发，一次只采集一张；输出只允许位于仓库或系统临时目录。

---

## 4. 性能与证据语义

- `status`、`logs`、`snapshot` 和 screenshot 各自只报告脚本实际读取的字段，不自动升级为完整 E2E 或发布验收。
- 启动采样当前测量的是 Android `am start -W`，不是应用业务就绪时间。
- APK 报告始终可以说明文件字节；内部 section 取决于本机可用归档工具，缺失时保持缺失而不是推算。
- `collect_android_dumpsys.cjs` 会收集大体量系统信息，只在用户明确要求全量诊断时运行，不作为日常 Agent 命令。
- 同步采样绑定当前 Debug PID 与指定设备；进程、serial 或包不匹配时停止，不把其他进程数据并入报告。
- Criterion、历史基线和已有性能报告都是某次运行产物；新的性能结论必须重新绑定当前代码、输入、环境与实际采样。

---

## 5. 修改与完成边界

修改脚本时，从 `package.json` 的实际命令入口追踪参数解析、设备选择、副作用目标、输出限制和清理路径。新增写设备能力必须保持 Debug-only、目标可判定、失败闭合和所有权可回收；不得为采集方便扩大输出、扫描无关设备状态或读取个人 Release。

脚本语法和纯解析变化可使用对应 Node 入口做有界检查；需要 ADB、安装、重载、截图或性能采集时，先取得用户对该操作的明确指令。执行结果只作为本次工具证据，不要求生产代码迎合脚本。

**完成**：命令入口、目标设备、Debug 包、副作用所有者、输出预算与清理路径保持一致，并明确报告实际执行了什么、生成了什么以及哪些设备或产品结论仍未取得。
