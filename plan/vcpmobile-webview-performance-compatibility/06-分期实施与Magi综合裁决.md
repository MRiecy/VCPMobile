# 06. 分期实施与 Magi 综合裁决

## 1. 三方独立审查

### 1.1 Melchior：逻辑与系统

核心判断：BootScreen 自身很轻，优先证据不支持“启动动画”或“Rust AST”是主因。高价值候选依次为：

1. `READY` 前全量头像二进制 IPC、设置/助手快照和恢复历史的混合门禁；
2. 初始静态模块图中的非首屏富渲染/全局 UI 代码；
3. `READY` 后无条件实例化的异步 Overlay，以及关闭状态仍挂载的左侧栏/AgentList/Sortable；
4. 每条 MessageRenderer 的 KaTeX/Mermaid selector 与表情全树扫描；
5. HTML cache 到 500 项后整表清空的潜在抖动；
6. 壁纸 decode、eager 主题和 settings 第二次 IPC 等次级候选。

Melchior 的硬约束：保留 30 Hz 流式合帧、后台 snapshot 收敛、AST diff、最终 blocks、KaTeX/Mermaid/高亮和现有 active-content guard。优先减少无关工作，不降低刷新/内容能力。

### 1.2 Balthasar：移动直觉与视觉

核心判断：旧机/平板“UI 不完整”最像外壳兼容问题，而不是消息能力过强：

1. 根 `flex-col` 与 `>=768px` relative 双侧栏存在确定性几何矛盾；
2. `minSdk`、WebView provider/version 和 CSS build target 尚未形成兼容契约；
3. `color-mix()`、关键 `gap/inset`、safe-area 与键盘数据缺少系统化 fallback/验证；
4. 原生 Insets 事件可能早于 Vue listener，需要可查询或可重放的当前快照；
5. IME bottom 与 system bars bottom 的关系应明确，避免部分 ROM 双重计入；
6. `backdrop-blur-xl`、全屏双层 blur、可多实例 Toast blur、`.theme-switching *` 与长期 `will-change` 是低端 GPU 风险。

Balthasar 建议以 WebView 87/111/最新版作为实验锚点。综合裁决保留这些实验点，但在采到真实受影响设备版本前，不把 87 写成已批准的产品最低版本。

### 1.3 Casper：工程与交付

核心判断：当前设施能开始调查，但不能证明 Vue 首屏慢或旧机兼容达标：

1. `am start -W` 不等于 boot-ready，10 样本 p95 不足以做发布门禁；
2. Criterion 与 CI benchmark compile 不覆盖用户首屏；
3. happy-dom 不处理 CSS，当前 CI 没有生产 bundle/CSS 兼容/真机性能门禁；
4. manualChunks 只分文件，不改变静态 import 的首屏执行；
5. 应先补分段 trace，后按真实最大段逐个施工；
6. 门槛先测噪声再冻结，设备失败不能由现代机平均值抵消。

Casper 的交付约束：不先做全面虚拟列表、依赖升级、polyfill 或大型测试平台；每个候选单独提交、A/B、可完整回滚。

## 2. 综合裁决

三方没有发现需要削减消息气泡能力的证据。最终裁决如下：

| 决策 | 裁决 |
|---|---|
| “Vue 拖慢首屏” | 目前只是未分段的总体现象，不是已证根因 |
| BootScreen | 保留；它提供早期反馈且静态成本低 |
| Rust AST/30 Hz 合帧 | 保留；当前设计已有回压与终态机制 |
| 全量头像阻塞 READY | 高优先级 P1 候选；先测 count/bytes/time/RSS，并与隐藏侧栏实例化一起设计 |
| 恢复 5 条历史 | 保持数量；先分 cache hit/miss 测量，不能用空白假装 READY |
| Overlay/侧栏提前挂载 | 高优先级低风险候选，按首次打开挂载/初始化 |
| `color-mix` 与现代装饰 | 基础 fallback + `@supports` 增强，不做旧机主题 |
| 平板双栏 | 修 shell 几何；断点由主区最小宽度推导 |
| 安全区 | system bars + display cutout 四边 snapshot、净 IME + 可重放；CSS env 仅 bootstrap fallback |
| WebView 87/111 | 设备实验锚点，暂非产品最低版本承诺 |
| 全面消息虚拟化 | P3 条件候选；只有目标旧机长会话数据失败时才启动 |
| 固定毫秒 SLA | 首轮真机基线后由产品冻结，当前不臆造 |

## 3. 分期路线

### Phase 0：冻结证据与施工安全

目标：得到可复现的 clean 基线，保护当前 Diary 工作区。

前置条件：

- 当前工作区有 Diary 未提交代码，任何专项代码施工前由用户确认范围并创建 checkpoint；
- 若新增模块文件、修改 `mod.rs/lib.rs` 或调整 Android 插件命令，严格执行项目的重构前存档协议；
- 文档提交与 Diary 实现提交分离，不能用 `git add .` 无选择混合历史；
- 固定 commit、APK SHA、fixture 和实际 WebView 版本。
- 由产品负责人和技术负责人共同冻结 tracked WebView 支持基线，明确最低 major、provider 例外、必测实体设备和生效版本；实验锚点不自动进入 Release 阻断集合。

交付：

- clean production Web build manifest；
- 受影响手机/平板的 provider/version、截图与复现步骤；
- 基线报告模板和原始样本目录；
- 明确 Release 或 release-like profileable 构建方式。

停止条件：拿不到受影响设备时，仍可做静态兼容修复候选，但不得宣称真机问题已解决。

### Phase 1：建立真实测量面

目标：把总时间拆成可归因阶段。

建议落点：

- `index.html`：HTML 起点与 buffered paint 采集；
- `src/main.ts`：JS entry、mount mark；
- `src/App.vue` / `src/core/stores/appLifecycle.ts`：listener、snapshot、core、READY；
- `src/features/chat/ChatView.vue`：READY 后的 chat-shell 双 rAF 与首次输入；
- `settings.ts`、`assistant.ts`、`avatar.ts`、`chatHistoryStore.ts`：边界时间和 payload metadata；
- `MessageRenderer.vue`：first message/rich settled；
- `tests/perf/scripts/measure_startup_adb.cjs`：收集 app-level trace、build identity 和统计；
- Android 设备脚本：记录 provider/version、gfxinfo、热状态。

实现要求：marks 先内存聚合、终态或超时后一次导出；生产默认无高频日志；每次页面启动只生成一个 `bootTraceId` 用于拼接分块日志，不把它升级成生命周期状态。不要创建全局性能状态机。

探索验收：S1—S3 在受影响手机、受影响平板和现代对照机各完成 5—10 次 process-cold，分段无缺失，足以选出首个候选；候选正式接受时再按 05 文档跑每个 build/单元格总计 20—30 次的三批 A/B。

### Phase 2：兼容与平板 P0 修复

目标：先解决会让 UI 不完整的结构问题。

按独立提交施工：

1. 重建 App `WorkspaceRow`，明确 overlay/single/dual pane；
2. 让左右栏的 responsive mode 与 `layoutStore` open 状态一致；
3. 补 `color-mix()` 基础声明与少量语义化 `@supports`；
4. 对关键全屏 Overlay 检查生成 CSS；当前 Uno 已展开关键 inset utilities，只有残留的结构性 shorthand 才补 fallback，flex gap 另做几何验收；
5. 合并 `systemBars` 与 `displayCutout` 四边安全区，保留 raw IME、统一派生净 IME 高度，并提供当前快照重放；
6. 把 `backdrop-blur-xl` 和多实例/全屏 blur 改为实色表面，保留允许的小面积单例增强。

潜在文件：

- `src/App.vue`；
- `src/components/layout/AgentSidebar.vue`；
- `src/components/layout/RightSidebar.vue`；
- `src/assets/themes.css`；
- `src/core/composables/useKeyboardInsets.ts`；
- `src-tauri/plugins/vcp-mobile/android/.../KeyboardInsetsManager.kt`；
- Viewer/Overlay 的直接 safe-area 消费点；
- 对应 Vue/Kotlin/契约测试。

每一跨层提交后运行 `pnpm check`，并按测试架构补前端和 Android JVM 契约测试。最后仍需平板/旧机设备验收。

### Phase 3：首屏低风险候选

严格按 Phase 1 最大耗时排序，不要求全部实施。建议实验顺序：

1. 手机端关闭的 AgentSidebar/AgentList 延迟挂载或至少延迟 Sortable/VcpAvatar 初始化；
2. READY 再只加载当前对象和平板常驻侧栏真实可见项的头像，其余小并发暖机；若无法与上一步独立验证，两步明确作为一个耦合实验；
3. FeatureOverlays 按首次 open 锁存挂载；
4. settings recovery 查询移出关键路径；
5. UpdatePrompt/release notes、重型 viewer 和低频 Block 收紧 import graph；
6. KaTeX CSS 与 JS 同一个 loader 按需准备；
7. 当前主题最小加载、其余主题 manifest/lazy loader；
8. 若证据成立，再处理壁纸 decode。

每项都必须有独立 commit、A/B 报告、强渲染回归和回滚点。若改善小于噪声或首次功能打开明显退化，整项回退。

### Phase 4：消息运行时与长会话

只有强渲染/100+ 消息 trace 仍失败时启动：

1. 先从现有 blocks/节点遍历派生 feature hints，跳过确定无关的 DOM 扫描；只有本地判定不可靠时才扩张 Rust DTO；
2. blocks watcher 与流结束 watcher 按 revision 合并一次 rich render；
3. HTML cache 由整表清空改为增量 LRU；
4. `.theme-switching *` 收口到固定 shell；
5. Tool/Thought/streaming 装饰只在可见且活跃时持有 `will-change`；
6. 旧 WebView 长会话仍不达标时，才原型化可恢复的消息 DOM 窗口。

这一阶段不得降低 30 Hz 上限、关闭 AST diff 或删除任何强渲染 Block。

### Phase 5：门禁制度化

- PR：production Web build、资源 manifest、生成 CSS 扫描、强渲染语义与响应式几何；
- Nightly/具名设备：核心 S1—S3；S4/S5 在相关候选、周期长稳或 Release 前扩展，并归档截图、代表性 Perfetto/gfxinfo/RSS；
- Release：先增加 pre-publish RC 入口，对签名 RC 的固定 commit/APK SHA、支持基线设备和具名验收放行，再发布并上传同一份已验收 artifact；当前仅在 `release.published` 后构建的 workflow 不能充当此前置门禁；
- 先 report-only，取得至少三批稳定噪声后冻结绝对 SLA 与资源预算；
- 每次依赖/Vite/UnoCSS 升级重新生成兼容报告，不默认沿用旧结论。

## 4. WebView 版本采集的最小实现选择

按复杂度由低到高：

1. **实验室先行：** 性能脚本采集 `dumpsys webviewupdate`，无需修改产品代码；
2. **永久诊断：** 使用已有 AndroidX WebKit 新增 `get_webview_info` 插件命令或并入现有诊断快照；允许 provider API 返回 `null`，以 `unavailable + reason` 表达；
3. **现场问题：** 设置页“诊断信息”展示 provider/version 和 CSS feature probes，用户可复制但不上传敏感数据。

如果新增插件命令，必须四重注册：Rust `invoke_handler`、`build.rs COMMANDS`、`permissions/default.toml + all.toml`、`guest-js/index.ts`，并运行 `pnpm check` 生成权限产物。若只是测试脚本即可满足 Phase 1，则暂不扩张生产 IPC。

## 5. 提交与回滚策略

建议提交序列：

```text
docs: add WebView performance and compatibility research
test(perf): add boot trace schema and collection
fix(layout): establish tablet workspace shell
fix(android): unify replayable window insets
fix(css): add old WebView structural fallbacks
perf(startup): defer non-visible avatar materialization
perf(startup): mount feature overlays on first open
perf(render): coalesce rich-content postprocessing
```

实际提交只包含已实施阶段，不预建空模块。每个提交都必须：

- 修改一个可解释机制；
- 附对应基线/候选报告；
- 通过静态与相关单测；
- 可用单一 revert 完整撤销；
- 不夹带 Diary 或其他工作区修改；
- 不 push，除非用户明确要求。

## 6. 最终完成定义

专项完成不是“文档写完”或“数字变小”，而是：

- 实际受影响旧手机和平板都记录了 WebView provider/version；
- 平板 portrait/landscape、分屏、键盘和安全区无裁切/失联；
- S1—S3 的 `T_chat_shell` 与 `T_rich_settled` 达到冻结 SLA；
- S4 长稳的帧时间、RSS 与 layer count 稳定；
- 强渲染契约 L0—L2 和所有 fixture 通过；
- 优化收益超过噪声，失败实验已回滚；
- 支持基线、CSS 规范和 Release 设备证据进入长期文档/门禁；
- 当前 Diary 与其他无关工作没有被污染。

## 7. 当前交付状态

本轮完成的是研究、实证构建和施工方案，没有修改生产代码、没有构建 APK、没有执行真机测量。当前结论可直接指导下一轮 Phase 0/1，但不得把 `DEVICE-EVIDENCE-PENDING` 标记成已验收。
