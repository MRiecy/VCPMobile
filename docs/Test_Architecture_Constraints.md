# VCPMobile 测试体系架构约束文档

> 版本：v1.0（已审批）
> 状态：**正式约束文档** — 所有测试 Agent 必须遵循，冲突项需重新报批
> 范围：Rust 后端 / Tauri 核心 / 自定义插件 / Vue 前端 / Android E2E / 性能稳定性
> 生效日期：2026-07-01
> 阶段：阶段一（诊断与架构）已完成，进入阶段二（Rust 后端整理与补充）

---

## 0. 文档地位

本文档由 @测试架构师 基于五份 Specialist 诊断报告汇总，经用户全量审批后生效。它是后续所有测试 Agent 的**全局约束**：未经重新报批，任何 Agent 不得变更目录结构、引入全局依赖、修改业务源码，或偏离本文件规定的分层与隔离原则。

---

## 1. 审批记录

| 项 | 决策 | 状态 |
|----|------|------|
| 分层比例 | L1-L4 70% / L5-L6 20% / L7-L8 10% | ✅ 已批准 |
| 目录结构 | `src-tauri/tests/`、`plugins/vcp-mobile/android/src/test/`、`src/tests/`、`tests/e2e-android/` | ✅ 已批准 |
| 工具选型 | Vitest + happy-dom / JUnit + Robolectric + MockK / wiremock + mockall / Maestro | ✅ 已批准 |
| CI 分级 | Fast CI / Full Test / Android Device / Performance 四级流水线 | ✅ 已批准 |
| 进入阶段二 | 由 Rust 后端测试专家提交现有测试整理方案 | ✅ 已批准 |

---

## 2. 测试分层（L1-L8）

| 层级 | 名称 | 覆盖范围 | 目标 | 触发频率 | 主要工具 |
|------|------|----------|------|----------|----------|
| L1 | 单元测试 | 纯函数、算法、DTO、内存状态机 | 80%+ 纯逻辑路径 | 每次 PR/push | cargo test、Vitest |
| L2 | Rust 集成测试 | Tauri command + mock state/DB/FS/HTTP | 核心 command 路径 | 每次 PR/push | tauri::test、wiremock、tempfile |
| L3 | Android 插件单元测试 | Kotlin 纯逻辑、Shadow 系统服务 | ForegroundGuardian、安全校验、JSON 序列化 | 每次 PR/push | JUnit 4、Robolectric、MockK |
| L4 | 前端组件/Store 测试 | Vue 原子组件、纯状态 Store | L1 组件、无重副作用 Store | 每次 PR/push | Vitest、@vue/test-utils、happy-dom |
| L5 | 契约测试 | Rust command 签名 ↔ TS 调用 | 命令名/参数/返回值一致性 | 每次 PR/push | tauri-specta 或手写校验脚本 |
| L6 | Android 集成/设备测试 | Service/Activity/权限生命周期 | StreamKeepaliveService、pickFile、权限请求 | nightly/main | AndroidX Test、模拟器 |
| L7 | 移动端 E2E | 真机关键用户旅程 | P0/P1 旅程 | release 前/nightly | Maestro + adb |
| L8 | 性能与稳定性 | 启动时间、APK 体积、长稳指标 | 基线回归 | nightly/release | cargo bench、profile.perf、ADB 脚本 |

**用例数量比例**：L1-L4 ≈ 70%，L5-L6 ≈ 20%，L7-L8 ≈ 10%。

---

## 3. 目录结构

所有测试代码与业务代码**物理隔离**，不修改 `src/` 或 `src-tauri/src/` 原有结构。

```
VCPMobile/
├── src-tauri/
│   ├── tests/                          # Rust 仓库级集成测试（新增）
│   │   ├── fixtures/                   # include_str!/include_bytes! 内嵌数据
│   │   ├── integration/                # command 集成测试
│   │   └── utils/                      # mock_app 构建器、临时 DB 工厂
│   └── src/...                         # 现有内联 #[cfg(test)] 单元测试保留原位
├── src-tauri/plugins/vcp-mobile/
│   ├── android/src/test/               # Kotlin 单元测试（Robolectric）
│   ├── android/src/androidTest/        # 仪器测试
│   └── tests/                          # 插件 Rust 侧集成测试
├── src/
│   └── tests/                          # 前端测试
│       ├── unit/stores/
│       ├── unit/components/
│       ├── unit/composables/
│       ├── integration/                # invoke/listen/Channel mock 与契约
│       └── e2e/                        # Playwright（桌面端可选）
├── tests/                              # 仓库级脚本与 E2E
│   ├── e2e-android/                    # Maestro flows
│   ├── perf/                           # 性能/体积/启动时间脚本
│   └── scripts/
└── .github/workflows/
    ├── ci.yml                          # 快速反馈
    ├── test-rust.yml                   # Rust 全量
    ├── test-android.yml                # Kotlin 单元 + 设备测试
    ├── test-frontend.yml               # Vitest
    └── perf.yml                        # 性能/体积基线
```

---

## 4. 工具选型

### 4.1 Rust 后端
- `cargo test` + `tokio::test`：原生，无需额外 runner。
- `tauri = { features = ["test"] }`（仅 dev-dependencies）：启用 `tauri::test::mock_app()`。
- `tempfile` / `assert_fs`：隔离文件系统副作用。
- `wiremock`：模拟 VCP HTTP/SSE 后端。
- `mockall`：为 DB/HTTP/文件接口生成 mock trait。
- `serial_test`：串行访问全局状态（如 SQLite 文件）。
- `criterion`（可选）：替换 `ast_bench.rs` 的 `#[test]` 性能基准。

### 4.2 Android 插件
- JUnit 4 + Robolectric：不启动模拟器测试 Kotlin 纯逻辑与 Shadow 系统服务。
- MockK：mock Android Context/Activity/Manager。
- AndroidX Test + Espresso/UIAutomator：仪器测试（L6），按需启用。

### 4.3 Vue 前端
- Vitest：Vite 原生集成。
- `@vue/test-utils` + happy-dom：组件挂载与 DOM 断言。
- `@tauri-apps/api/mock` 或自建 wrapper：统一 mock invoke/listen/Channel。
- tauri-specta 或手写生成脚本：Rust command → TypeScript 类型生成，作为契约测试。

### 4.4 移动端 E2E
- Maestro：声明式 YAML，对 WebView 支持较好，与 adb 启动/停止天然配合。
- adb：安装、启动、权限预授权、日志收集、崩溃收集。

### 4.5 性能/稳定性
- `cargo test --profile perf`：运行 ast_bench 等 Rust 热路径基准。
- ADB 脚本：冷启动采样、内存采样、logcat 聚合。
- GitHub Actions artifacts：APK 体积、启动时间、基准结果历史化。

---

## 5. 测试数据生命周期管理

1. **Fixture 内嵌化**：所有外部文件依赖改为 `include_str!` / `include_bytes!` 或放入 `src-tauri/tests/fixtures/`，**杜绝绝对路径**。
2. **临时资源隔离**：每个测试用例独立 `tempdir`，自动清理。
3. **数据库隔离**：每个测试使用独立 SQLite 文件或内存库；关键 DB 测试串行化。
4. **单例状态清理**：Kotlin `ForegroundGuardian` 通过反射在 `@Before` 重置；Rust managed state 每次 `mock_app` 重建。
5. **事件监听器清理**：Tauri `listen` 在测试后显式注销，避免泄漏。
6. **Android 运行时权限**：E2E 前通过 `adb shell pm grant` 预授权普通权限；特殊权限（通知监听、厂商保活）在测试机上手动预配置或跳过断言。

---

## 6. CI/CD 分级执行策略

| Pipeline | 触发条件 | 内容 | 目标耗时 |
|----------|----------|------|----------|
| Fast CI | 每次 PR / push | vue-tsc + cargo fmt --check + cargo clippy -D warnings + cargo test --lib（纯单元）+ Vitest 单元 | < 5 min |
| Full Test | PR ready / merge queue | Fast CI + Rust 集成测试 + Robolectric + 前端组件测试 | < 15 min |
| Android Device | nightly / main | 模拟器仪器测试 + Maestro E2E（如有真机设备池） | < 30 min |
| Performance | nightly / release | ast_bench + APK 体积追踪 + 真机启动时间采样 | 报告型 |

---

## 7. 代码隔离原则（铁律）

1. **物理隔离**：所有测试代码与业务代码物理隔离，不修改 `src/` 或 `src-tauri/src/` 原有结构。
2. **内联测试保留**：现有内联 `#[cfg(test)]` 单元测试保留在原位，可在原位重构（仍为内联）。
3. **新增集成测试归位**：新增的集成测试统一放入 `src-tauri/tests/`，不内联到业务文件。
4. **禁止改业务接口**：不得以"方便测试"为由修改业务接口；若必须引入 trait 抽象才能测试，作为**独立重构提案**单独报批，不得在测试任务中顺手改。
5. **Fixture 内嵌化**：`include_str!` / `include_bytes!` 或 `src-tauri/tests/fixtures/`，禁止绝对路径。
6. **现有文件修改原则**：对已存在文件的修改使用原子级编辑（Edit/replace），严禁全文件覆盖。

---

## 8. 阶段化实施计划

| 阶段 | 内容 | 当前状态 | 审批节点 |
|------|------|----------|----------|
| 阶段一 | 诊断与架构 | ✅ 已完成 | 本文档 |
| 阶段二 | Rust 后端：整理现有测试 + 补充缺失 | 进行中 | 现有测试整理方案 + 新增覆盖方案 |
| 阶段三 | Tauri 插件 Kotlin 测试 + Vue 前端测试 | 待审批后实施 | Kotlin 可测试性方案 + 组件测试粒度方案 |
| 阶段四 | Android E2E + 性能稳定性 | 待审批后实施 | adb 真机流程方案 + 基准测试方案 |
| 阶段五 | CI/CD 整合 | 待审批后实施 | 流水线配置方案 |

---

## 9. 风险与前置假设

1. **业务源码隔离**：测试代码不修改业务接口；若某些 command 必须抽象才能测试（如引入 trait），作为独立重构提案单独报批。
2. **Android 真机限制**：部分权限和厂商保活设置无法 adb 自动授权，E2E 需依赖预配置测试机或 root 设备。
3. **Tauri test feature**：启用后仅在测试编译生效，不影响 release 构建。
4. **Mock 成本**：PluginHandle、Raw JNI、AppHandle.path() 等需要适度抽象，预计增加少量 trait 层但不改变业务行为。
5. **前端动态渲染**：聊天消息块、KaTeX、Mermaid 等不做白盒 DOM 断言，改为"渲染不崩溃 + 关键容器存在 + E2E 截图兜底"。

---

## 10. Agent 角色与协作规则

- **@测试架构师**：总体策略与分层设计，发布并维护本约束文档。
- **@Rust后端测试专家**：Rust 测试验证与现有测试整理；先整理后补充。
- **@Tauri与插件测试专家**：Tauri 层与 Android Kotlin 插件测试。
- **@前端测试专家**：Vue 前端与渲染层测试。
- **@移动端E2E测试专家**：Android 真机端到端验证。
- **@性能与稳定性测试专家**：非功能性需求验证。

**协作铁律**：
1. 先诊断后开方——基于实际代码事实，不假设。
2. 架构师先行——本文件为全局约束，其他 Agent 不得擅自变更。
3. 后端整理优先——现有测试规范化是首要交付物。
4. 审批节点——架构方案、工具选型与目录结构、E2E 流程、CI 配置必须暂停报批。
5. 代码隔离——测试代码与业务代码物理隔离。
6. 因地制宜——具体措施由各 Agent 根据实际代码分析后提出，禁止照搬外部模板。

---

*最后更新：2026-07-01 | VCPMobile 测试体系 v1.0。架构变更需更新本文件并重新报批。*
