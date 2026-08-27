# VCPMobile 测试体系架构约束文档

> 版本：v1.2（价值密度校准）
> 状态：**正式约束文档** — 所有测试 Agent 必须遵循，冲突项需重新报批
> 范围：Rust 后端 / Tauri 核心 / 自定义插件 / Vue 前端 / Android arm64 E2E / 手工性能诊断
> 最近校准：2026-08-27
> 当前实现：单一 `ci.yml` + Vitest/happy-dom + Rust tests + Android 插件 JVM tests + Android Debug Agent CLI

---

## 0. 文档地位

本文档由 @测试架构师 基于五份 Specialist 诊断报告汇总，经用户全量审批后生效。它是后续所有测试 Agent 的**全局约束**：未经重新报批，任何 Agent 不得变更目录结构、引入全局依赖、修改业务源码，或偏离本文件规定的分层与隔离原则。

---

## 1. 审批记录

| 项 | 决策 | 状态 |
|----|------|------|
| 产品目标 | 仅 Android `arm64-v8a` 触控手机/平板/按窗口宽度响应的折叠屏 | ✅ 生效 |
| 目录结构 | `src-tauri/tests/`、插件 `android/src/test/`、`src/tests/`、`tests/e2e-android/` | ✅ 已落地 |
| 当前工具 | Vitest + happy-dom / cargo test + Criterion / JUnit + Robolectric + MockK / Node.js + adb | ✅ 已落地 |
| 当前 CI | 单一 `.github/workflows/ci.yml` 执行前端、Rust、生成树、Android JVM 与审计门禁 | ✅ 已落地 |
| 设备证据 | 具名 Android 设备截图、触控与 WebView 证据 | `DEVICE-EVIDENCE-PENDING` |

---

## 2. 测试分层（L1-L8）

| 层级 | 名称 | 覆盖范围 | 目标 | 触发频率 | 主要工具 |
|------|------|----------|------|----------|----------|
| L1 | 单元测试 | 纯函数、算法、DTO、内存状态机 | 核心纯逻辑与竞态边界 | 每次 PR/push | cargo test、Vitest |
| L2 | Rust 集成测试 | 文件提取与真实 fixture 边界 | 仓库级跨格式路径 | 每次 PR/push | cargo test、tempfile、固定二进制 fixture |
| L3 | Android 插件单元测试 | Kotlin 纯逻辑、Shadow 系统服务 | ForegroundGuardian、安全校验、固定事件 schema | 每次 PR/push | JUnit 4、Robolectric、MockK |
| L4 | 前端组件/Store 测试 | Vue 原子组件、纯状态 Store | L1 组件、无重副作用 Store | 每次 PR/push | Vitest、@vue/test-utils、happy-dom |
| L5 | 契约测试 | Rust/TS/Kotlin/权限/Release/Android UI 静态契约 | 命令、参数、生成物与治理一致性 | 每次 PR/push | Vitest 文本契约、Kotlin 反射/文本契约 |
| L6 | Android 集成/设备测试 | Service/Activity/权限生命周期 | 需要模拟器或真机的原生行为 | 手工按需 | AndroidX Test / adb |
| L7 | 移动端 E2E | 真机关键用户旅程与多窗口 UI | P0/P1 旅程 | release 前具名验收 | Android Debug Agent、人工触控/截图 |
| L8 | 性能与稳定性 | 启动时间、APK 体积、长稳指标 | 诊断报告，不设当前自动阈值 | 手工按需 | Criterion、`tests/perf/` ADB 脚本 |

分层用于表达证据能力，不冻结容易失真的用例数量比例。测试数量以 runner 实际列举结果为准。

### 2.1 用例准入与删除标准

测试不是代码清单，也不以数量或覆盖率占坑。新增或保留的用例必须能指出一个可信的回归，并至少保护以下一类边界：

1. 状态机终态、并发 owner/epoch、取消/重试、事务原子性或资源上限；
2. 输入校验、权限、文件路径、隐私、安全、跨进程/跨层数据契约；
3. 类型系统和普通编译无法闭合的 Rust ↔ TypeScript ↔ Kotlin ↔ 权限/发布接缝；
4. 用户可观察且曾经出错的交互结果，或关键旅程的失败/降级行为。

出现以下情况时直接删除，不为维持数量而修补：

- 仅证明 props/slot 原样渲染、Vue/JUnit/Rust 标准行为、常量等于自身或简单 getter/setter；
- 只冻结 class 名、普通文案、源码排版、文档段落或内部函数调用方式，且已有类型检查、构建或行为测试覆盖；
- 与更接近真实边界的 Store/状态机/事务/安全测试重复；
- 对已经移除的兼容路径、旧 schema、旧命令或旧 UI 结构继续做存在性断言；
- mock 完整复述实现步骤，导致生产代码改名即可失败，却无法区分用户行为是否退化。

源码文本契约只用于没有更强验证手段的安全、权限、发布和跨语言接缝；不得用它同步文档措辞、视觉细节或普通模块清单。业务行为迁移后，应从公开输入/输出和可观察状态重写；业务契约已经退出时，删除旧测试和 fixture。

---

## 3. 目录结构

新增仓库级测试优先与业务代码物理隔离；既有 Rust `#[cfg(test)]` 内联单测保留原位。不得为了测试方便改变生产行为或公开接口。

```
VCPMobile/
├── src-tauri/
│   ├── tests/                          # Rust 仓库级集成测试
│   │   ├── fixtures/file_extractor/    # 固定二进制样本
│   │   ├── file_extractor_integration.rs
│   │   └── unit/diary_service_tests.rs # cfg(test) 路径挂载的私有逻辑测试
│   └── src/...                         # 现有内联 #[cfg(test)] 单元测试保留原位
├── src-tauri/plugins/vcp-mobile/
│   ├── android/src/test/               # Kotlin 单元测试（Robolectric）
│   └── android/src/androidTest/        # AndroidX 仪器测试目录（按需）
├── src/
│   └── tests/                          # 前端测试
│       ├── setup.ts / mocks/ / utils/
│       └── unit/                       # Store、组件、跨层文本/治理契约
├── tests/                              # 仓库级脚本与 E2E
│   ├── e2e-android/scripts/            # Debug-only Agent CLI 与 ADB 基础层
│   └── perf/scripts/                   # 手工性能/体积/启动诊断
└── .github/workflows/
    ├── ci.yml                          # 单一 PR/push 软件门禁
    └── release.yml                     # 已发布 Release 的签名 arm64 APK 构建
```

---

## 4. 工具选型

### 4.1 Rust 后端
- `cargo test` + `tokio::test`：原生，无需额外 runner。
- `tempfile`：隔离文件系统副作用。
- `criterion`：`src-tauri/benches/ast_tail_bench.rs` 的人工 benchmark；CI 只做 `--no-run` 编译检查。
- `wiremock`、`mockall`、`serial_test` 与 Tauri test feature 当前未作为仓库既有基础设施，不得在文档中写成已落地。

### 4.2 Android 插件
- JUnit 4 + Robolectric：不启动模拟器测试 Kotlin 纯逻辑与 Shadow 系统服务。
- MockK：mock Android Context/Activity/Manager。
- AndroidX Test + Espresso/UIAutomator：仪器测试（L6），按需启用。

### 4.3 Vue 前端
- Vitest：Vite 原生集成。
- `@vue/test-utils` + happy-dom：组件挂载与 DOM 断言。
- `src/tests/mocks/`：统一 mock invoke/listen/Channel、plugin guest-js 与浏览器 API。
- 手写文本/运行时契约：覆盖 Rust command ↔ TS 调用、权限声明、Release 与 Android UI 治理；当前没有 tauri-specta。

### 4.4 移动端 Debug / E2E
- 当前只使用 `tests/e2e-android/scripts/` 中的 Node.js + adb。Agent 统一入口为 `android-debug-agent.cjs`：固定 Debug 包、USB Dev、设备发现、验证后安装、权限预授权、有限日志、状态与 opt-in 单张截图。
- 完整契约见 `docs/ANDROID_AGENT_DEBUGGING.md`。日常 Agent 调试不得调用全量性能采集脚本，也不得把 Debug snapshot 描述为自动 UI E2E。
- Maestro 尚未引入；Playwright 桌面 E2E 不在产品支持范围，也不是 Android WebView 兼容证据。未来引入任何 runner 必须先提交独立提案与真实缺口证据。

### 4.5 性能/稳定性
- `cargo bench --locked --profile perf`：Rust 热路径人工 benchmark。
- `tests/perf/scripts/`：APK 体积、冷启动、dumpsys/logcat 与 benchmark 归档。
- 这些资产当前仅供本地人工诊断，不在 CI Release 中执行，也没有冻结回归阈值。性能 Phase 1 已恢复为 report-only 的独立 A/B 轨道；不得虚构 nightly/performance pipeline，也不得把 Debug/HMR 结果升级成 Release 或能耗结论。

---

## 5. 测试数据生命周期管理

1. **Fixture 内嵌化**：所有外部文件依赖改为 `include_str!` / `include_bytes!` 或放入 `src-tauri/tests/fixtures/`，**杜绝绝对路径**。
2. **临时资源隔离**：每个测试用例独立 `tempdir`，自动清理。
3. **数据库隔离**：每个测试使用独立 SQLite 文件或内存库；关键 DB 测试串行化。
4. **单例状态清理**：Kotlin `ForegroundGuardian` 在 `@Before` 重置；Rust 测试不得跨用例复用可变全局状态。
5. **事件监听器清理**：Tauri `listen` 在测试后显式注销，避免泄漏。
6. **Android 运行时权限**：E2E 前通过 `adb shell pm grant` 预授权普通权限；特殊权限（通知监听、厂商保活）在测试机上手动预配置或跳过断言。

---

## 6. 当前 CI 与设备分工

| 证据面 | 触发条件 | 当前内容 | 能证明 / 不能证明 |
|---|---|---|---|
| `.github/workflows/ci.yml` | main/master push 与 PR | vue-tsc、Vitest、生产 build、Rust fmt/test/integration/clippy、Android 生成树漂移、插件 JVM tests、pnpm 依赖审计 | 证明软件与生成物静态契约；不证明真机 CSS 几何/触控 |
| `.github/workflows/release.yml` | GitHub Release published | 校验版本源/签名并发布 arm64 APK | 证明发布 artifact 治理；不是发布前设备实验室 |
| Android Debug Agent | 具名人工执行 | Debug-only 安装/权限/USB Dev/PID 日志/状态/单张截图 | 证明指定设备启动与诊断状态；不自动遍历 UI，不触碰 Release |
| 多设备 UI 验收 | Release 前具名执行 | 手机/平板/折叠屏窗口、WebView、截图与触控矩阵 | 是 `DEVICE-VERIFIED` 的必要证据，当前 `DEVICE-EVIDENCE-PENDING` |
| 性能脚本 | 独立性能候选人工执行 | APK size、`am start -W`、dumpsys、Criterion、具名页面帧 A/B | 仅报告；不构成自动 Release SLA |

---

## 7. 代码隔离原则（铁律）

1. **新增测试归位**：前端、Android JVM 与仓库级 Rust 集成测试放在既有测试目录；仅既有 Rust `#[cfg(test)]` 内联单测留在业务模块原位。
2. **内联测试保留**：现有内联 `#[cfg(test)]` 单元测试保留在原位，可在原位重构（仍为内联）。
3. **新增集成测试归位**：新增的集成测试统一放入 `src-tauri/tests/`，不内联到业务文件。
4. **禁止改业务接口**：不得以"方便测试"为由修改业务接口；若必须引入 trait 抽象才能测试，作为**独立重构提案**单独报批，不得在测试任务中顺手改。
5. **Fixture 内嵌化**：`include_str!` / `include_bytes!` 或 `src-tauri/tests/fixtures/`，禁止绝对路径。
6. **现有文件修改原则**：对已存在文件的修改使用原子级编辑（Edit/replace），严禁全文件覆盖。

---

## 8. 当前落地状态

| 能力面 | 状态 | 当前落点 |
|---|---|---|
| 软件测试基础设施 | ✅ 已落地 | 单一 CI、Vitest、Rust tests、Kotlin JVM tests |
| 跨层与治理契约 | ✅ 已落地并持续维护 | 前端/Android/Release 文本和行为契约 |
| Android Debug Agent | ✅ 已落地 | Debug-only USB Dev、有限诊断与单张截图；不冒充完整 UI runner |
| 多设备样式验收 | `DEVICE-EVIDENCE-PENDING` | 按 `ANDROID_UI_COMPATIBILITY.md` 归档具名证据 |
| 性能 Phase 1 | `ACTIVE / REPORT-ONLY` | D0 因果定位与 D1 packaged Debug A/B 已通过；R1、BootTrace 与固定 SLA 仍 pending |

---

## 9. 风险与前置假设

1. **业务源码隔离**：测试代码不修改业务接口；若某些 command 必须抽象才能测试（如引入 trait），作为独立重构提案单独报批。
2. **Android 真机限制**：部分权限和厂商保活设置无法 adb 自动授权，E2E 需依赖预配置测试机或 root 设备。
3. **Host scaffold 边界**：host 编译和非 Android fallback 只用于开发/测试；通过不代表 Windows/macOS/Linux 产品支持。
4. **Mock 边界**：PluginHandle、Raw JNI、AppHandle.path() 等若确实需要抽象，必须作为独立生产重构评审，不能为追求覆盖率顺手增加 trait 层。
5. **前端动态渲染**：聊天消息块、KaTeX、Mermaid 等不做白盒 DOM 断言，改为"渲染不崩溃 + 关键容器存在 + E2E 截图兜底"。

---

## 10. Agent 角色与协作规则

- **@测试架构师**：总体策略与分层设计，发布并维护本约束文档。
- **@Rust后端测试专家**：Rust 测试验证与现有测试整理；先整理后补充。
- **@Tauri与插件测试专家**：Tauri 层与 Android Kotlin 插件测试。
- **@前端测试专家**：Vue 前端与渲染层测试。
- **@移动端E2E测试专家**：Android 真机端到端验证。
- **@性能与稳定性测试专家**：在独立性能任务中维护人工诊断资产、证据等级与同身份 A/B。

**协作铁律**：
1. 先诊断后开方——基于实际代码事实，不假设。
2. 架构师先行——本文件为全局约束，其他 Agent 不得擅自变更。
3. 当前优先级——兼容硬契约不可为性能让路；性能只接纳已有可复现证据且不降级强渲染的独立候选。
4. 审批节点——新增 runner、架构方案、工具选型与目录结构、E2E 流程、CI 配置必须暂停报批。
5. 代码隔离——新增测试归入既有测试目录，不以测试名义改变生产接口。
6. 因地制宜——具体措施由各 Agent 根据实际代码分析后提出，禁止照搬外部模板。

---

*最后更新：2026-08-13 | VCPMobile 测试体系 v1.1。平台与设备证据以 `ANDROID_UI_COMPATIBILITY.md` 为准。*
