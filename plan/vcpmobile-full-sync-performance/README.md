# VCPMobile 全量同步性能与内存专项

> 状态：`COMPLETED / PULL-INSTRUMENTATION-RETIRED / UPSTREAM-SPLIT-PENDING`
>
> 测试日期：2026-09-02
>
> 实现提交：`430fb25 perf(sync): instrument full pulls and raise workers`
>
> 范围：空 Mobile Debug 全量 Pull、NDJSON 数据面、Topic worker、SQLite 写队列、Final ACK 与 Mobile 进程内存

> 2026-09-03 收尾：下述 Pull 细粒度埋点及数据仍作为 2026-09-02 的历史测量证据保留在本文中；当前生产路径已移除逐 chunk、逐帧和逐 Topic 的一次性 Profile 代码。`worker=4`、Final ACK 标记、慢写/失败观测与 report-only 采样器继续保留，采样器报告格式升级为 v2。

## 1. 专项目标

本专项回答五个问题：

1. 261 个 Topic 的全量 NDJSON 是否会完整落库并经 Final ACK 收敛；
2. 32 MiB 单帧限制是否在当前真实数据上形成风险；
3. Mobile Topic worker、SQLite 或数据面谁是主要耗时来源；
4. 全量同步及后续空增量同步是否出现持续内存抬升；
5. 更换网络与将 Topic worker 从 2 提升到 4 后，为什么总耗时发生变化。

非目标：本轮没有测量远端 VChat Node/CDS RSS，没有建立 Release SLA，没有修改 Wire/Schema、NDJSON 单线顺序、DbWriteQueue 单写者、Flush 或 Final ACK 语义。

## 2. 测试环境与固定条件

| 项目       | 固定值                                                                   |
| ---------- | ------------------------------------------------------------------------ |
| Mobile 包  | `com.vcp.avatar.debug` / `1.1.6-debug`                                   |
| 设备       | OPPO PHZ110，Android 16 / API 36，arm64-v8a                              |
| WebView    | Google WebView `150.0.7871.181`                                          |
| 数据路径   | VChat Node → VCP-CDS → HTTP NDJSON → Mobile Rust → DbWriteQueue → SQLite |
| 预渲染同步 | 关闭                                                                     |
| 同步日志   | `DEBUG`                                                                  |
| Mobile RSS | `/proc/<pid>/status`，默认每 500ms                                       |
| Mobile PSS | `dumpsys meminfo --local --checkin`，默认每 2s                           |
| 原始报告   | `tests/perf/reports/*/`，本机证据，受 `.gitignore` 管理，不进入 Git      |

对比轮次均使用同一份桌面数据。两次成功全量 Pull 的结果精确一致：261 个 Topic、6753 条消息、`24,121,559` 字节（23.004 MiB）；因此性能差异不是数据量变化造成的。

## 3. 实现内容

### 3.1 数据面可观测性

- Mobile 为每个完整 Topic 帧记录消息数与 Wire 字节数，不保留消息正文；
- Mobile 为整条 `/messages/pull` 响应记录 chunk 数、总字节数、首字节与末字节时间；
- Final ACK 接受后记录明确标记，避免把“网络读完”误当成“同步已持久化”；
- 新增 `pnpm perf:sync-pipeline`，连续采集 Mobile RSS/PSS、Topic 处理、DbWriteQueue 事务、Flush 与同步终态；
- 采样器只操作已运行的 Debug 包，不安装、不启动、不清库、不修改设置；
- Topic worker 从固定 2 调整为固定 4，仍在读取下一帧前等待可用 worker 槽位。

### 3.2 使用方法

```bash
pnpm android:debug:doctor -- --json
pnpm android:debug:status -- --json
pnpm perf:sync-pipeline -- --serial <serial> --out-dir tests/perf/reports/<run-name>
```

采样器输出：

- `samples.ndjson`：按时间采集的 Mobile PSS/RSS/Swap/HWM，以及可选的同机 Node/CDS RSS；
- `sync-metrics.log`：白名单过滤后的 NDJSON、Topic、SQLite、Flush、Final ACK 与完成日志；
- `summary.json`：机器可读的分位数、计数、字节数与终态摘要。

## 4. 测试轮次

### 4.1 R0：混合部署失败基线

首次空库全量 Pull 在约 15 秒处失败：

- 收到 147/261 个 Topic；
- 收到 2514 条消息、`13,811,483` 字节；
- 最大 Topic 帧 465,960 字节，没有触及 32 MiB；
- 已收到的 147 次 DB 写入全部提交；
- 没有 Phase 3 completed、Final ACK 或同步完成；
- 结构化错误为 `TIMEOUT / desktop_cds / messages / VCP-CDS request timed out`。

根因不是慢网本身，而是只替换了新版 CDS EXE，VChat Node 仍加载旧版 15 秒 `requestNdjson` 超时实现。完整部署同步插件、共享 CDS client 并重启 VChat/Node 后，该限制消失。

本机报告：`tests/perf/reports/full-sync-worker-2-20260902-1740/`。

### 4.2 R1：worker=2，完整配对部署，旧网络

| 指标                    |                          结果 |
| ----------------------- | ----------------------------: |
| UI 总耗时               |                      25,850ms |
| Topic / 消息            |                    261 / 6753 |
| NDJSON                  | 24,121,559 字节（23.004 MiB） |
| 网络 chunk              |                          7817 |
| 首字节                  |                     580.272ms |
| 末字节                  |                  24,773.626ms |
| 有效数据段吞吐          |                   0.951 MiB/s |
| 最大 Topic 帧           |                  466,148 字节 |
| Topic prepare p95 / max |            2.666ms / 10.479ms |
| submit_queue p95 / max  |             0.037ms / 1.009ms |
| DB wait p95 / max       |            0.006ms / 39.506ms |
| 最终可见 Flush          |                      69.539ms |
| Finalizer               |                     284.085ms |

持久化日志确认 261/261、Phase 3 completed、一致性校验成功、同步完成与 Session ended。完成事件位于 Final ACK 接受及最终 Flush 之后，因此本轮是完整收敛，不是只有网络成功。

本机报告：`tests/perf/reports/full-sync-worker-2-redeploy-20260902-1817/`。

### 4.3 R2：不清库的收敛验证

R1 后立即进行第二次同步：

- `Phase 3 skipped: no changed topics`；
- NDJSON 0 帧，Topic 处理 0 个，DB 写入 0 次；
- Final ACK 正常；
- 6 次 Flush 最大 0.074ms；
- 同步前稳定 PSS 约 312.5 MiB，同步瞬时约 348.8 MiB，随后稳定在约 316.5–317 MiB；
- RSS 高水位没有刷新。

这证明 R1 没有漏写，并且没有观察到“每同步一次就继续抬升”的 Mobile 内存泄漏。

本机报告：`tests/perf/reports/full-sync-convergence-20260902-1822/`。

### 4.4 R3：worker=4，新 Wi-Fi

| 指标                    |                          结果 |
| ----------------------- | ----------------------------: |
| UI 总耗时               |                      16,431ms |
| Topic / 消息            |                    261 / 6753 |
| NDJSON                  | 24,121,559 字节（23.004 MiB） |
| 网络 chunk              |                          8789 |
| 首字节                  |                     148.757ms |
| 末字节                  |                  15,688.083ms |
| 有效数据段吞吐          |                   1.480 MiB/s |
| Topic prepare p95 / max |            2.086ms / 19.305ms |
| submit_queue p95 / max  |             0.030ms / 1.803ms |
| DB wait p95 / max       |            0.003ms / 20.567ms |
| Phase 3 PSS p50 / max   |         318.1 MiB / 320.8 MiB |
| Finalizer               |                     247.947ms |

持久化日志确认 261/261、0 errors、Phase 3 completed、一致性校验成功、同步完成与 Session ended。

本机报告：`tests/perf/reports/full-sync-worker-4-newwifi-20260902-1855/`。

## 5. 成对比较与结论

| 指标                      | R1：worker=2 / 旧网络 | R3：worker=4 / 新 Wi-Fi |              变化 |
| ------------------------- | --------------------: | ----------------------: | ----------------: |
| UI 总耗时                 |               25.850s |                 16.431s | -9.419s（-36.4%） |
| Desktop `/messages/pull`  |               24.184s |                 15.454s |           -8.730s |
| Mobile 末字节             |               24.774s |                 15.688s |           -9.086s |
| 有效吞吐                  |           0.951 MiB/s |             1.480 MiB/s |            +55.7% |
| 261 Topic Mobile 处理总量 |               208.9ms |                 169.1ms |           -39.8ms |

结论：

1. 本轮 9.419 秒收益几乎全部发生在 CDS→Node→网络→Mobile 的数据面；Mobile worker 处理总量只减少约 40ms，worker=4 不是主要加速来源。
2. 同一 CDS 版本可以达到 1.480 MiB/s，因此 CDS 不存在固定的 0.951 MiB/s 上限。旧网络链路是 R1 变慢的主要嫌疑，CDS/OS 热缓存仍是未隔离的次要变量。
3. 当前 worker=4 没有产生 Queue 背压或 Phase 3 内存恶化，按用户决策保留为生产候选；这不构成“4 比 2 快 9 秒”的因果声明。
4. 当前 23.004 MiB 若要在 10 秒内完成，单数据面至少需要约 2.3 MiB/s；R3 的 1.480 MiB/s 仍不足以复现历史十秒内结果。
5. 2026-08-28 的旧日志确实记录了从 Session start 到 255/255 Pull 完成约 7.43 秒，但当时没有 NDJSON 字节数、完整消息数、内存或 Final ACK 证据，不能与本轮直接成对比较。

## 6. 内存结论

- 空库初始 PSS 约 257.5 MiB；R1 Phase 3 峰值约 325.3 MiB，R3 约 320.8 MiB；
- R1 完成后出现约 393.2 MiB 的短暂峰值，但峰值发生在 Phase 3 结束之后，随后回落；
- 数分钟后 R2 同步前已回落到约 312.5 MiB，R2 完成后稳定在约 316.5–317 MiB；
- 没有观察到逐次同步持续抬升，当前证据不支持 Mobile 存在持续泄漏；
- 全量导入后的稳定工作集高于空库，可能包含 SQLite 页面、导入后的数据投影和 UI 缓存；未做组件级堆归因；
- Node/CDS 位于远端，本轮没有采集其 RSS，因此本专项不能代替双端长稳测试。

## 7. 32 MiB 与流式边界

- Mobile、Node 和 CDS 的关键限制均是单条完整 NDJSON Topic 帧 32 MiB，不是整次 Pull 只能 32 MiB；
- 当前整次 Pull 为 23.004 MiB，最大单帧只有 466,148 字节；
- 当前真实数据既没有触及单帧限制，也没有验证超过 32 MiB 的 Topic 错误路径；
- Mobile 仍单线读取上游 NDJSON，只有完整 Topic 帧进入最多 4 个处理 worker；DbWriteQueue 继续单写者落库。

## 8. 如何最终拆分网络与 CDS

当前 Mobile 埋点只能测整条数据面，不能把 CDS 编码和网络背压分开。若需要关闭 `UPSTREAM-SPLIT-PENDING`，只增加诊断计时，不改变协议：

1. CDS 记录每个 Topic 的 `encode_ms`、字节数与整次累计编码时间；
2. VChat Node 分别累计“等待 CDS 下一帧”和 `response.write()`/`drain` 等待时间；
3. 使用同一份空 Mobile Debug 数据再跑一次全量 Pull；
4. CDS 编码累计接近总耗时则判定 CDS；Node downstream drain 接近总耗时则判定网络；两者都小则继续检查 Node 解码、校验和二次序列化。

纯 TCP 23 MiB 回放也可作为网络侧交叉验证，但必须由桌面端启动发送服务并允许 Tailscale 接口访问；ping 只能证明 RTT 与丢包，不能证明吞吐。

## 9. 验证记录

- `cargo fmt --all -- --check`：通过；
- PullExecutor Rust 定向测试：6/6 通过；首次沙箱内失败仅因 localhost bind 被拒绝，授权环境重跑通过；
- `pnpm check`：通过；
- Node `--check` 与 Prettier：通过；
- 低扰动采样器 smoke：3 秒 6 个 RSS 点、2 个 PSS 点、0 errors；
- worker=4 最终源码已重新编译并安装到 `com.vcp.avatar.debug`，真机完成 R3；
- Release `com.vcp.avatar` 未启动、停止、清理、覆盖或重装；
- VChat/VCP-CDS 本轮只做部署与只读诊断，没有新的仓库修改。

## 10. Magi 综合裁决

- **Melchior**：worker=4 仍是固定有界并发；NDJSON 单线、DbWriteQueue、Flush、Final ACK 与错误传播不变。真实 Phase 3 PSS 未恶化，但上游 RSS 未覆盖。
- **Balthasar**：本轮不改 UI；以 UI 总耗时辅助用户感知，但只用协议和持久化证据判定成功。
- **Casper**：保留一个 report-only 采样器和固定 worker 常量，不引入分页、动态调度器、状态机或未经证明的自动调参。

## 11. 后续复测门禁

每次要比较 worker、网络或 CDS 版本时，必须同时满足：

1. 相同桌面数据及同一组 Topic/消息/Wire 字节数；
2. 只清理 `com.vcp.avatar.debug` 的业务数据，Release 保持只读；
3. `syncLogLevel=DEBUG`、预渲染关闭；若需要比较 NDJSON 吞吐或 Topic 处理耗时，必须在隔离的 Android Debug 测量版本中临时恢复等价埋点，当前 v2 采样器不会把缺失数据报告为零；
4. 从同步前启动采样，持续到 Final ACK、最终 Flush、完成事件和 Session ended；
5. 紧接着执行一次不清库的第二轮同步，必须出现 `Phase 3 skipped: no changed topics`；
6. 报告区分源码事实、真机证据、静态推断和尚未测量项。
