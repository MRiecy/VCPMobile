# Phase 0：验收记录与 Magi 交接

> 原验收日期：2026-10-08，`4c6e25bb` 加当时工作树。**Phase 0 已完成**；当时 Phase 1–6 尚未实施。第 1–3 节保留原交付/执行记录，第 4 节于 2026-10-09 校准当前交接；不将旧结果描述为本轮复验。

## 1. 本阶段交付

- Phase 0–6 均有专属目录和入口；Phase 0 展开 00–05 子规划/审计/交接文档，Phase 1–6 尚未实施。
- D1–D19、当前生产行为、minimum 工程选择和执行证据分别记录。
- 命令抽取脚本与来源快照覆盖 plugin/app/Kotlin/permissions/capability/消费者；具体限制和人工审计见 02。
- 依赖审计定位 cfg/features/C/FFI、Swift selector/队列、模板、资源和签名输入；11 个 DEP 问题已分配阶段。
- 产品边界、移动端 UI 规范及现行引用同步；保留 Android 生产契约与既有工作树修改。

## 2. 实际验证

| 检查 | 结果 | 能说明什么 |
|---|---|---|
| Node/pnpm/Rust/Cargo/Tauri CLI 版本 | 通过，见 01 | 本地输入可定位 |
| 6 条 Apple target `cargo tree --locked --offline` | 均退出 0，完整命令见 03 | 目标依赖解析与 feature 图，不是编译/链接 |
| `pnpm check` | 退出 0；vue-tsc 与 host cargo check 通过 | 当前工作树 TypeScript/Rust host 静态基线；不是 Apple 编译 |
| 命令注册与调用面盘点 | 42 个插件命令、176 个应用命令、59 个 Kotlin 命令，九组集合差为 0 | 记录当时的源码盘点结果，审计范围见 02 |
| Markdown 链接与围栏检查 | 45 份文档、145 个相对链接目标均存在，围栏闭合 | 使用现有 marked 解析，排除 inline code 示例；不包含远端链接可用性 |
| 旧规范引用搜索 | 只剩两处更名记录及两处明确历史行号引用；现行链接无旧路径 | 导航迁移完成，历史证据未伪造新行号 |
| ignore 正反样本 | 15 项通过：8 项生成物/签名样本忽略、7 项维护输入可追踪 | 无实际凭据读取/写入；配置、Swift、plist、隐私与 resolution 源未被误忽略 |
| 继承修改 SHA-256 保留检查 | 未编辑的 17 个继承路径全部匹配原始字节 | 研究 README/01/08 仅追加实施导航/记录；其余既有修改保留 |
| `git diff --check` | 通过 | 已跟踪补丁无空白错误；不作为生产行为证据 |
| Apple 构建、签名、Actions、设备操作 | 未执行 | 均不在本阶段执行范围 |

## 3. Magi 汇合

| 视角 | 本次发现 | 处理结论 |
|---|---|---|
| Melchior | app IPC 总闸不覆盖 plugin；not(android) 误纳入 iOS；Swift future 取消可能触发上游 send unwrap | Phase 1 分清能力/cfg/ACL，Phase 2 保留最终回调接收和 generation 丢弃；不新增业务 owner |
| Balthasar | 启动权限与更新的调用/文案/挂载必须一起隔离；返回和 Insets 有 Android 语义；ImageIO 还须接已有 CAS 发送 | Phase 1 封闭入口，Phase 3/4 保留同一组件/状态树并逐路径接入 |
| Casper | product/binding、SwiftPM resolution、Scene gate、device archive runtime 检查均有精确条件 | 明确 Phase 2/G1 任务；不提前扩建验证体系、不升级框架碰运气 |

三方共同结论：Phase 0 用可复查输入和具体任务支持 Phase 1；原生附件和尽力后台仍是首产物范围。争议细项见 04 §4。规范名称按研究 01 的用户既定更名决定统一，不再沿用最初审查中保留旧名称的建议。

## 4. 2026-10-09 剩余问题与下一步

| 优先级/归属 | 具体动作 | 闭环位置 |
|---|---|---|
| 已完成/已替代 | cfg/ACL、目录、lifecycle DTO 已实施；旧 typed capability 又已被 plugin-os 同步身份替代 | Phase 1 历史记录与 Phase 2 当前平台简化，勿重复构建四轴表 |
| Phase 2/G1 | Swift/Channel、单 Scene、minimum 和可重放输入已有源码；Apple 编译/链接仍待取得 | 03 更新后的 DEP-02–06/08–10；真实 G1 诊断 |
| Phase 3 | SQLite/Sync/LAN 及业务收尾；共同权限、本 App 通知、电量/空间/网络和 Distributed | 最新 Phase 3 README；字段缺失、授权偏好和实际执行结果分开 |
| Phase 4 | 原生文件/媒体、拍照/相册、双向分享、staging/像素预算、WebKit 缓存与输入 | 最新 Phase 4 README；不继续执行旧分享/相机/相册延期 |
| Phase 5/6 | SSE 后台持续接收专项、其余尽力后台、进度/终态、最终 ID/签名与 G2 产物 | 主线 04/06/07 与 Phase 5/6；候选方案不当作已有效实现 |

没有把后续施工记作 Phase 0 成果。minimum 15.0 是 Phase 0–2 的历史输入，用户已于 2026-10-09 明确改为 26.0，见 [D4](../09-Magi综合裁决与待决策清单.md)；现阶段不再以“尚无 Swift/配置”或“先建立能力表”排任务。源码工作继续按 [当前 Phase 2](../phase-2/README.md) 与 Phase 3–6 的依赖推进；G1/G2 仍未取得 Apple 结果。
