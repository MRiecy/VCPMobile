# 06 · HTTP 连接管理重构评估（reqwest Client 治理）

> 议题（用户提出）：VCPMobile 强依赖网络连接——日记中心、VCP Log、VCPInfo、
> 同步、聊天等皆是连接使用者。当前「各模块自建 reqwest Client」的方案可能最稳，
> 但不一定最好。多连接管理已是独立的系统设计问题。
> 本篇从**工程规范 / 可读性 / 稳定性 / 性能提升**四个维度评分，裁决是否值得重构。

---

## 1. 现状盘点（2026-08 全量摸排）

`src-tauri` 内 `reqwest::Client` 构建点共 **9 处，散布 8 个模块**，
另有两条独立 WebSocket 通道（VCP Log、分布式）与同步子系统的 WS。

| # | 位置 | 配置 | 生命周期 | 评价 |
| --- | --- | --- | --- | --- |
| 1 | `infra/vcp_client.rs:465`（`perform_vcp_request`） | 仅 `tcp_keepalive(20s)` | ⚠️ **每次聊天请求新建** | 热点路径最痛点：每条消息新建 Client + 连接池 |
| 2 | `infra/vcp_client.rs:1766`（`test_vcp_connection`） | `timeout(10s)` | 一次性探测 | 合理，瞬时探测就该用完即弃 |
| 3 | `infra/vcp_client.rs:2237`（`recover_active_generation`） | 默认 | 每次恢复新建 | 低频，可接受 |
| 4 | `infra/model_manager.rs:46` | `pool_max_idle_per_host(10)` + `pool_idle_timeout(90s)` | 结构体持有 | ✅ 教科书式 |
| 5 | `chat/topic_summary_service.rs:71` | `timeout(AI_REQUEST_TIMEOUT)` | ⚠️ 每次总结新建 | 低频（话题总结），可接受但不规范 |
| 6 | `updater/update_manager.rs:283/293` | 两个专职 Client（release 检查 / APK 下载，GitHub redirect 策略） | 每次更新会话 | ✅ 场景特殊，合理 |
| 7 | `sync/sync_service.rs:941` | `timeout(2s)` 健康探测 | 瞬时 | ✅ 合理 |
| 8 | `sync/sync_service.rs:1005` | `connect 10s` + `total 120s` | 会话持有，`&Client` 传遍全部 executor | ✅ 子系统内典范 |
| 9 | `chat/message_service.rs:22` | `connect 10s` + `total 60s` | `OnceLock` 全局单例 | ✅ 教科书式 |
| 10 | `diary/diary_service.rs:76` | `connect 10s` + `redirect(none)` | `DiaryServiceState` 持有 | ✅ 良好 |
| 11 | `chat/emoticon_manager.rs:219` | `Client::new()` 裸默认 | ⚠️ 每次拉取新建 | 低频 admin_api 调用 |
| 12 | `infra/local_server.rs:279` | `Client::new()` | debug-only 开发代理 | 不参评（非产品路径） |

**结论性观察**：
- 做得好的（sync、message、model、diary）证明团队**已经懂**共享 Client 的范式，
  只是没有沉淀成公共机制，新模块靠各自觉悟；
- 真正的问题集中在 **#1（聊天流式，每请求新建）** 与两处低频裸建（#5、#11）；
- 配置策略发散且无文档：超时/重定向/池参数各写各的，无人知道全局图景；
- 即将新增的 logcenter、taskcenter 会是第 10、11 个构建点——**现在不定规矩，
  发散会继续加剧**。

## 2. 关键领域事实

1. `reqwest::Client` 内部是 `Arc`，克隆廉价，**设计意图就是全局共享**：
   共享连接池、HTTP/2 多路复用、TLS session 复用。
2. 新建 Client 的代价 = 新连接池 + TLS 配置初始化；首个请求额外支付
   TCP + TLS 握手（移动网络 1–2 个 RTT，约 100–500ms）+ 射频唤醒耗电。
3. Client 级配置（redirect 策略、池参数、tcp_keepalive）与请求级配置
   （`RequestBuilder::timeout()`、header、auth）正交——**大部分「策略差异」
   其实是请求级差异，不构成各自建 Client 的理由**。
4. 移动网络特有风险：WiFi↔蜂窝切换后池内空闲连接变「半死不活」，
   复用时可能报 `connection closed before message completed`。
   缓解：`pool_idle_timeout` 缩短（30–90s）让坏连接自然淘汰——
   model_manager 已有此实践。
5. 目的地天然隔离：VCP 服务器 / 同步服务器 / GitHub / 本地回环是不同 host，
   reqwest 池按 host 分桶，共享 Client 不会造成跨功能连接争用。

## 3. 候选方案

### 方案 A：维持现状
各模块自建。零改动成本，发散继续。

### 方案 B：命名画像注册表（推荐）
新增 `vcp_modules/infra/http_clients.rs`，提供少数几个**命名画像**的共享 Client：

```rust
pub enum HttpProfile { ChatStream, AdminApi, Probe, Download }
pub fn client(profile: HttpProfile) -> &'static Client  // OnceLock 懒初始化
```

- `ChatStream`：`tcp_keepalive(20s)` + `pool_idle_timeout(60s)`，无总超时（流式）；
- `AdminApi`：`connect 10s` + `redirect(none)`，供 diary/emoticon/logcenter/taskcenter；
- `Probe`：`timeout(2–10s)`，健康探测/测试连接（也可选择不共享，见 §5）；
- `Download`：连接超时 + 无总超时（停滞判死编排已有）。
- **请求级差异（超时、auth）一律留在调用点**，注册表不管；
- sync 子系统、updater **不迁移**（它们已是典范/特例，动它们纯风险）；
- WebSocket 通道**不入册**（连接语义完全不同，强扭是过度抽象）。

### 方案 C：完整连接管理器系统
统一注册 + 动态重配置 + 健康检查 + 熔断 + 指标上报。
对本项目体量是过度设计：引入状态机与后台任务的新故障面，
收益（可观测性）可用更轻的日志替代。**否决**。

## 4. 四维评分（10 分制）

| 维度 | A 现状 | B 注册表 | C 管理器 | 裁决说明 |
| --- | --- | --- | --- | --- |
| **工程规范** | 4 | 8.5 | 7 | 现状 9 个构建点、策略无文档、违反 reqwest 共享惯例；B 把策略集中成可审查的清单；C 规范分反降——框架自身的复杂度成为新的不规范源 |
| **可读性** | 5 | 9 | 6 | B 的 `client(HttpProfile::ChatStream)` 是自文档；新人不再需要「各自觉悟」；C 要读懂状态机才能发一个请求 |
| **稳定性** | 6 | 7.5 | 6.5 | 现状的每请求新建反而有「自愈」假象（无共享状态可腐坏），这是它唯一的长处；B 的共享池引入半死连接风险，用 `pool_idle_timeout` + 目的地分桶对冲后净收益为正；C 的熔断/健康检查对单一后端场景几乎无用武之地 |
| **性能提升** | 5 | 8 | 8 | 真实收益集中在聊天热路径：每条消息省 1 次 TCP+TLS 握手（移动端首 token 提前 100–500ms）+ 减少射频唤醒省电；B 与 C 在此维度收益相同——**性能不需要 C** |
| **加权总评** | 5.0 | **8.3** | 6.6 | B 明显胜出 |

## 5. 风险与缓解

| 风险 | 等级 | 缓解 |
| --- | --- | --- |
| 共享池半死连接（网络切换） | 中 | 每个画像显式 `pool_idle_timeout(≤90s)`；流式请求失败重试语义不变 |
| 迁移 vcp_client 聊天路径引入回归 | 高 | 独立 commit + 全量回归（聊天单测 + 真机流式冒烟）；失败即 revert 单点 |
| 画像数量膨胀（每新功能加一个） | 低 | 注册表加注释门槛：新画像必须在文档说明「为什么现有画像不适用」 |
| 探测类请求共享后相互排队 | 低 | `Probe` 画像可保持「每次新建」的例外并注释理由——规范允许有据例外 |
| sync/updater 被顺手重构 | — | **明确禁止**，本篇范围外 |

## 6. 结论

**值得重构，但以方案 B 的收敛形态做**，定位为一个小而硬的独立迭代（建议编为 **S0**，
先于或与 S1 日志中心并行）：

1. 新建 `http_clients.rs`（约 100 行，含画像文档注释）；
2. 迁移顺序：`emoticon_manager`（最低风险验证）→ `topic_summary_service` →
   `vcp_client.rs:465` 聊天热路径（单独 commit + 回归）→ `vcp_client.rs:2237`；
3. logcenter / taskcenter 从第一天起消费 `AdminApi` 画像，
   原 04 篇的 `admin_api_client.rs` 提案**并入本方案**（URL 拼接 + basic_auth
   的便捷函数可以挂在同一模块，但 Client 本体走画像注册表）；
4. sync、updater、local_server、两条 WebSocket **不动**；
5. 完成后在 `docs/modules/` 补一份《HTTP 连接画像注册表》模块文档，
   把「为什么这样分画像」写成规矩。

**不做的事**：不做动态重配置、不做健康检查/熔断、不做指标系统、
不统一 WebSocket 管理、不动 sync/updater。
