# 10｜S1 审计：localLoopback 收敛删除映射表

> 日期：2026-08-14
> 方法：5 个并行只读审计子代理，覆盖全部 `localLoopback`/`LocalLoopback`/`local_loopback` 相关符号；未修改任何文件。
> 依据 ADR：[08-本地回环收敛与保活分层ADR](./08-本地回环收敛与保活分层ADR.md)
> 分类：`delete`=仅服务 localLoopback 的代码；`migrate`=跨路线共享但按 route 分支、收敛后简化；`keep`=共享基础设施不动。

## 1. 总览（23 个主文件 + 连带清理）

| 组 | 文件数 | 审计行数 | delete | migrate | keep |
|---|---:|---:|---:|---:|
| A 传输层 | 1（vcp_client.rs） | 3162 | ~583 | ~53 | ~2526 |
| B 本地多轮 owner | 3（turn_coordinator/meta/types） | 1847 | ~1796 | ~1 | ~26 |
| C 本地 turn 存储 + 结果 | 2（turn_ledger/result） | 1753 | ~1290 | ~125 | ~280 |
| D runtime/设置/服务/分布式工具 | 6 | 5324 | ~390 | ~110 | ~4820 |
| E 前端/测试/golden | 11 | 3370 | ~324 | ~179 | ~2867 |
| F 前端 turn-wire 消费者（补充） | 3 | 1892 | ~330 | ~50 | ~1512 |
| **合计** | **26** | **17348** | **≈4710** | **≈518** | **≈12030** |

> 行数为"≈"估算（midpoint），精确到行需在 S2 逐条落刀时二次核对；误差 ±3% 以内。
> 连带清理（不在上表，但删除后必须同步改才能编译/过 CI）：`cli/mod.rs`、`src-tauri/src/lib.rs` 命令注册、`SettingsView.vue` 路由切换宿主、`cli/manifest.ts` 的 `LOCAL_ROUTE_GUIDE_STORAGE_KEY`、**删除 SQLite 迁移 0007/0008/0009 文件**（开发期未发布，最终迁移集 = 0001-0006，无需 0010）。

## 2. 关键裁决：exact-once 续轮/幂等 → 删，由 VCPToolBox 承担

这是之前 ±500 行不确定性的核心，现已闭合：

- `turn_coordinator/turn_ledger/turn_meta/turn_types` 整体是 localLoopback 的"模型→工具→续轮"多轮 owner。收敛后多轮 loop 归 VCPToolBox，这四文件**整删**，无迁移成本。
- 手机侧最小幂等已内置于 `MobileCliRuntimeState.replay_operation`（operation_id + action_sha256 去重，JobLedger 持久化可跨重启重放），分布式适配器已用确定性 `dist:` operation_id 接入。**无需迁出续轮代码**。
- 可选迁移仅一项（不在四文件内）：`execute_with_turn_admission` 的 cancel/deadline fence 若未来要让 VCPToolBox 使用，再迁入 distributed adapter；当前不迁。

## 3. 分文件明细（符号 → 行范围 → 分类）

### A. `src-tauri/src/vcp_modules/infra/vcp_client.rs`（3162）

| 符号 | 行 | 分类 |
|---|---|---|
| `VCP_TOOL_USE_FORBIDDEN_SENTINEL` | 96 | delete |
| `inject_local_loopback_transport_guard` / `should_inject_...` | 102-149 / 151-156 | delete |
| `LocalCliAuroraProjection` | 158-195 | delete |
| `StreamTurnMetadata` + `project_aurora_finish_reason` + `apply_stream_turn_metadata` + `project_model_step_event` + `stream_turn_metadata()` | 246-315 | delete |
| `VcpRequestPayload.turn_attempt/step_index/projection_reset` + `local_cli_projection_prefix` | 80-91 | delete |
| `StreamEvent` 的 turn 字段 + `with_turn_metadata/with_turn_projection` | 349-381 | delete |
| typed 预算 `TYPED_ASSISTANT_BUDGET_ERROR`/`is_typed_...`/`enforce_...`/`TYPED_NON_STREAM_...`/`read_response_body_bounded` | 94-244 | delete |
| 两个 handler 内 local 片段（stream 1260-1752、non-stream 1890-2029 分散） | 多段 | delete |
| `recover_active_generation` 内 `recover_local_cli_turn` 分支 | 2374-2386 | delete |
| 10 个 transport guard/typed 测试 | 2767-3161 | delete |
| `resolve_vcp_endpoint` / `resolve_request_route`（去 local 分支） | 318-338 | migrate |
| `mobile_cli_agent_route` 字段（wire 兼容） | 86-88 | migrate |
| `perform_vcp_request_registered` 路由/guard 片段 | 683-709 | migrate |
| `load_app_settings` / `resume_claimed_generation` 默认元数据 | 2047-2050 / 2642-2643 | migrate |
| helper SSE 代理、流式/非流式状态机主体、多模态预处理、ActiveRequests/Lease、恢复基础设施非 local 部分 | 其余 | keep |

### B. 本地多轮 owner（3 文件，1847 行）

| 文件 | 结论 |
|---|---|
| `cli/turn_coordinator.rs`（1292） | **整删**（`run_local_cli_turn` 34-56、`recover_local_cli_turn` 58-193、主循环 `run_record` 195-560、`finalize_turn` 863-934、批次幂等 707-801、单测 1065-1292）；仅 `now_ms`(4 行) keep |
| `cli/turn_meta.rs`（296） | 删 ~270（`plan_local_policy`、`LocalContinuationPolicy`、`local_optional_context_notices`、mark_history 块 93-194）；keep ~22（`redact_river_text` 64-80、`meta_fields_digest`） |
| `cli/turn_types.rs`（259） | 删 ~239（`LocalCliTurnRoute` 27-41 及本地类型）；**migrate 1 行 = `MAX_ASSISTANT_STEP_BYTES`(10)** —— 必须先迁到共享常量（infra/vcp_client.rs 210/1949/1969/2864 在用）再删 |

### C. 本地 turn 存储 + 结果（2 文件，1753 行）

| 文件 | 结论 |
|---|---|
| `cli/turn_ledger.rs`（1237） | **整删**（SQLite 本地 turn/step 表、`continuation_pending` 状态机全本地专属）；5 个纯 helper（`validate_digest`/`bounded_json`/3 个 int 转换）标 migrate，无调用者则一并删 |
| `cli/result.rs`（516） | keep ~280（`VcpCliResultEnvelope`/artifact/job/skill/错误码 + **`project_vcp_plugin_outcome`(338)**，vcpPlugin 真实结果路径）；delete ~85（7 处 `local_loopback` 打标 123-156/494/513 + marker + `prepend_optional_context_notices` + `serialize_local_model_payload`）；migrate ~120（死 wire 簇 `to_distributed`/`VcpDistributed*`/`serialize_distributed_tool_result` 237-308，**已核实无调用方**，删） |

### D. runtime / 设置 / 服务 / 分布式工具（6 文件，5324 行）

| 文件 | 要点 |
|---|---|
| `cli/runtime.rs`（2995） | 删 ~135（`MobileCliAdmissionFence` 133-137、`MobileCliAdmissionError` 139-164、`execute_with_turn_admission` 299-315、fence 参数/检查、fence 测试 2564-2631）；migrate 5 处 `VcpCliRuntimeInfo::local_loopback()` 打标（1512/1550/1596/1684/1902）；`execute`(289-297) 成为唯一入口；job/技能/输出基础设施全 keep |
| `infra/settings_manager.rs`（617） | 删 ~105（route 变更分类 121/139-140、turn 快照 211-215、重注册分支 392-411、测试 472-495/518-552）；migrate ~22（`MobileCliAgentRoute` 枚举/默认值/`freeze_mobile_cli_agent_route` 恒 VcpPlugin） |
| `agent/agent_chat_application_service.rs`（401） | 删 ~42（imports + `frozen_route` 124 + local 分支 149-186）；migrate 1（199 的 route 字段） |
| `group/group_chat_application_service.rs`（468） | 删 ~70（imports + 139 + local 分支 261-286 + outcome 处理 311-350）；migrate ~17（233/287-299/352-354 去 Option 包装） |
| `infra/model_manager.rs`（579） | migrate ~55（route 作为 endpoint 参数贯穿，删 local 分支恒 `/v1/chatvcp`）；delete 0 |
| `distributed/tools/vcp_mobile_cli.rs`（264） | 删 ~39（`current_mobile_cli_route` 129-133、`require_vcp_plugin_route` 135-142、`publication_route_enabled` 144-146、测试 219-233）；migrate ~11（`is_publishable` 去路由判定、`execute_with_context` 94 行删 `require_vcp_plugin_route`）；`execute_with_context`(82-126) 与分布式身份 keep |

### F. 前端 turn-wire 消费者（补充，审计子代理覆盖外，本轮 grep 坐实）

| 文件 | 行数 | 分类 | 要点 |
|---|---:|---|---|
| `src/core/stores/chatStreamStore.ts` | 1119 | migrate ~45 / keep ~1074 | `turnAttempt/stepIndex/projectionReset` 类型 26-28、turn 帧追踪 456-494、`continuation_pending` 处理 1022——收敛后删 turn 元数据、保留流式主体 |
| `src/tests/unit/chat/StreamStepProjection.test.ts` | 279 | delete ~279 | 整文件测 `project_model_step_event`/`with_turn_projection`（已被删除的 local 投影） |
| `src/tests/unit/chat/ChatConcurrencyGuards.test.ts` | 494 | migrate ~6 | 仅 `continuation_pending` 断言（约 422 行）受影响，其余并发守卫 keep |

### E. 前端 / 测试 / golden（11 文件，3370 行）

| 文件 | delete | migrate | 要点 |
|---|---:|---:|---|
| `settings.ts` | ~1 | ~17 | `DEFAULT_MOBILE_CLI_AGENT_ROUTE` 删；类型/字段/`normalize` 收敛为单值 |
| `AiLogicSettingsSection.vue` | ~28 | ~90 | 删 localLoopback 单选块 145-165、`selectRoute`、`routeChange`；vcpPlugin 预检块保留 |
| `vcpCliStore.ts` | 0 | 0 | 全 keep（run/poll/cancel/jobs/skills 与 route 无关） |
| `VcpCliManifestPanel.vue` | ~107 | 0 | 删本地路由 guide 块 229-311（含 Forbidden L281）、route 行 213-215、guide 状态 26-70 |
| `MobileCliAgentRoute.test.ts` | ~159 | ~18 | 删 3 个 localLoopback 专属 it；fixture 值更新 |
| `VcpCliGovernance.test.ts` | 0 | ~49 | **P2 契约测试 149-180 必须改写**（明文断言默认 localLoopback/类型含 vcpPlugin/`@route-change`） |
| `DistributedViewAuthorization.test.ts` | 0 | ~2 | fixture 值更新 |
| `VcpCliManifestView.test.ts` | ~26 | 0 | 删 guide 只读测试 142-165 + `LOCAL_ROUTE_GUIDE_STORAGE_KEY` import |
| `SettingsPatch.test.ts` | 0 | ~1 | fixture 值更新 |
| `vcp_cli_result.golden.json` | ~3 | 0 | 3 处 `"source":"local_loopback"` |
| `vcp_mobile_cli_manifest.golden.json` | 0 | ~2 | description 内双路线文本改写 |

## 4. 硬性前置（删除前必须先做，否则编译/测试红）

1. **`MAX_ASSISTANT_STEP_BYTES` 先迁共享常量**：`turn_types.rs:10` 被 `infra/vcp_client.rs`（210/1949/1969/2864）引用，删 turn_types 前必须先提升到共享位置。
2. **result.rs 需新增 `VcpCliRuntimeInfo::vcp_plugin()` 构造器**：runtime.rs 的 5 处 `local_loopback()` 打标与 golden 的 `"source"` 字段同步改，否则 golden 双向锁（result.rs 测试逐字段 assert）红。
3. **后端/前端路由默认值必须同一次提交改**：settings_manager 测试 473-495 断言默认 localLoopback；前端 `VcpCliGovernance.test.ts` 149-180 断言同样默认值。单边改必然红。
4. **SQLite 迁移：删除 0007/0008/0009 三条 CLI 迁移**（开发期，未发布；最终迁移集 = 0001-0006，与 main 基线一致）。`0007` 建 turn ledger（+2 索引 +2 触发器）、`0008` 建 semantic cache、`0009` 又 drop semantic cache——三者都是未发布分支的 CLI 特性迁移，收敛后全部冗余，整删。**血训纠正（S1 原判断有误）**：`sqlx::migrate!().run()` 不仅向前套用，还会校验「已应用迁移仍在解析集合中」——已应用但文件被删会直接报 `migration N was previously applied but is missing`；这与 `validate()` 的 checksum 校验是两码事。因此删迁移文件时，必须对**已应用过它的库**做精确清理：`DELETE FROM _sqlx_migrations WHERE version IN (7,8,9)` + `DROP TABLE` 相应孤儿表（`local_cli_turn_ledger`、旧遗留 `schema_migrations`），而**不是 `pm clear`**（会连珍贵数据一起清）。开发真机已按此精确清理并复验通过。
5. **模块连线**：`cli/mod.rs` 删 turn_coordinator/turn_ledger/turn_meta/turn_types 声明；`lib.rs` 若注册了 turn 相关命令一并删；`SettingsView.vue` 的 `onMobileCliRouteChange`/`mobileCliRouteSaving`/`mobileCliRouteError` 一并清。

## 5. 风险与联动

- **`replay_operation` 幂等需在 distributed adapter 层补回放测试**：删了本地 coordinator 后，vcpPlugin 的"同一操作不重跑"改由 `dist:` operation_id + `replay_operation` 保证，S2 需补一条 L1 用例。
- **删 `with_turn_projection`/`StreamTurnMetadata` 会波及前端**：消费者已定位为 `chatStreamStore.ts`（26-28/456-494/1022）与其测试（StreamStepProjection.test.ts、ChatConcurrencyGuards.test.ts:422）；`vcp_client.rs` 的 `turn_attempt/step_index/projection_reset` wire 字段删除前 grep 确认无残留引用。
- **测试数量下降不触发门禁**，但 P2 契约测试（VcpCliGovernance 149-180）会拦截——必须同步改写，不能只删测试。
- **`turn_ledger` 的 `LocalCliTurnRoute::VcpPlugin` 变体当前是死代码**（ledger 只写 LocalLoopback），删整文件时无额外处理。

## 6. 最终净改动量

| 指标 | 数值 |
|---|---|
| 净删除（delete） | **≈ 4710 行** |
| 改造/迁移（migrate） | **≈ 518 行**（其中 ~180 行是"死代码删"性质） |
| 保留不动（keep） | **≈ 12030 行** |
| 主文件 | **26 个** |
| 连带文件（mod.rs/lib.rs/SettingsView.vue/manifest.ts/删除迁移 0007/0008/0009） | **7 个** |

> 对比 08 ADR 的初步预估（3500-4500 行）：落在上沿，原因是被审计坐实——`turn_coordinator/meta/types/ledger` 四个本地多轮 owner 文件合计 ~3084 行近乎整删（而不是"部分迁移续轮语义"），续轮幂等无需迁移（已在 `replay_operation`）。

## 7. 一句话

> 收敛的本质是**整删 ~4700 行本地多轮 owner + 传输层 local 投影 + Forbidden + 前端 turn-wire 消费者**，保留 ~12000 行共享基础设施（runtime/job/output/skill/manifest/PTY/distributed adapter + 流式主体）；exact-once 幂等由 `replay_operation` 承载、多轮 loop 交还 VCPToolBox，零迁移成本。
