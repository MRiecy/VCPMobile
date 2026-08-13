# 04｜VCPToolBox 提示词所有权、Skills 与召回语义

## 1. 已冻结边界

本专项不设计 Mobile 侧 `CliPromptCatalog`、运行态提示词片段、Skills 轻目录注入、`mobile_system_prompt` 改写或 `context_assembler.rs` 新 seam。VCP 体系中，CLI 的主要工具提示词来源就是 manifest 的 `capabilities.invocationCommands[].description/example`；如何放进角色提示词、是否使用 DynamicTools、怎样手工微调，都由用户在 VCPToolBox 侧控制。

P2 在 typed `localLoopback` 网络请求首部加入的 `[[VCPToolUse=Forbidden]]` 不改变这条裁决：它是 VCPToolBox 在转发模型前剥离的请求级执行-owner 标记，用于关闭中央 parser/executor，既不向模型描述 CLI，也不持久化到 Agent prompt 或聊天历史。manifest 内容与用户微调仍只有 VCPToolBox 一个所有者。

Mobile 只拥有四件事：

1. 输出同一份规范 `VCPMobileCLI` manifest；
2. 本地回环和真实 VCP route 使用同一 parser/action/结果合同；
3. 按 manifest 如实实现 Shell、Job、权限和输出边界；
4. 在不可用时返回 `user_disabled`、`remote_disconnected` 等稳定执行错误，而不是另注入提示词纠正 Agent。

VCPToolBox 已提供精确占位符、`{{VCPAllTools}}`、`{{VCPDynamicTools}}` 和管理端指令描述编辑。真实 VCP 插件默认关闭只意味着“不向 Distributed registry 发布执行器”，不代表 Mobile 要删除或接管用户在 VCPToolBox 中保存的工具说明。桌面端 SkillBridge 不属于本专项的依赖、参考实现或验收范围，其实现和注入方式不用于 Mobile 方案。

默认本地 route 没有 Distributed 注册，因此 VCPToolBox 无法自动发现动态 manifest。Mobile 只提供规范 manifest 的查看/复制/导出能力；用户在 VCPToolBox 侧导入、引用和微调后，Agent 才稳定知道本地 CLI。这是用户控制提示词所带来的明确初始化步骤，不得再宣传为零配置。

## 2. 本地 CLI 目录与 Skills

应用私有 host catalog 允许保留：

```text
<catalog-root>/<id>/SKILL.md
               可选 references/
               可选 scripts/
               可选 assets/
```

它不是 Mobile 的提示词数据库，但也不再只是一个被动文件目录。Skill 的来源、hash、版本、完整性状态和本地文件权限进入 Rust 侧受控 Skill catalog；`VCPMobileCLI` manifest 将 `list_skills` 与 `read_skill` 声明为一等 action。Agent 能先列出可读项、再按稳定 ID 阅读 `SKILL.md` 或资源，不需要预先知道宿主真实路径，也不需要记住内部 Shell 命令。

Skill action 的唯一规范说明来自 `VCPMobileCLI` manifest；用户决定如何在 VCPToolBox 中放置或微调这份说明。Mobile 不承诺 OpenMinis 的 Top-20 提示词注入策略，也不采集 use count 给提示词排序，更不额外注入一份 Skill 摘要目录。

第三方 Skill 仍是潜在可执行内容：导入时保留来源与 hash，默认不因为出现 `scripts/` 就自动运行，也不因 Skill 已安装或校验通过而授予 Root、SAF 外部目录或密钥。catalog 不挂载到 PRoot；`read_skill` 只是阅读，返回 `vcp-skill://<id>` 逻辑引用，不是对文档内指令的隐式授权。执行脚本必须先经明确动作物化到 `/workspace`，再由 Agent 另发 `action=run`。VCP 自有记忆继续是长期事实与日记/知识召回的真源；本专项不创建 `/memory` 投影或平行记忆数据库。

## 3. `ink`、mark、river、vref 的精确语义

用户提到的 mark/ink 等“召回模式”在当前上游并非同一层。必须按源码命名，避免设计出不存在的 `mark` 字段。

### 3.1 `ink: mark_history`

精确 wire：

```text
ink:「始」mark_history「末」
```

parser 将其变成 `markHistory=true`。当全局 `ShowVCP=false` 时，它仍强制把这一调用的 VCPInfo/结果正文写入客户端历史。它：

- 不改变工具是否执行；
- 不决定 Job 是否持久化；
- 不控制结果是否回灌 Agent；
- 不是向量召回；
- 当前没有其他已实现 `ink` 值。

因此文档中的 “mark” 只作为 `mark_history` 的口语简称，不新增一个独立协议。

- [`toolCallParser.js@311dc42`](https://github.com/lioensky/VCPToolBox/blob/311dc42e8374afd1867bd1b5c06217baf8b0f463/modules/vcpLoop/toolCallParser.js#L120-L165)
- [`streamHandler.js@311dc42`](https://github.com/lioensky/VCPToolBox/blob/311dc42e8374afd1867bd1b5c06217baf8b0f463/modules/handlers/streamHandler.js#L481-L484)

Mobile 本地回环映射：该字段在 P2 就实现。即使用户关闭普通工具详情，带 `ink=mark_history` 的调用仍把一个有界 VCPInfo/结果摘要保存到聊天历史；完整 stdout 仍留在 artifact，不因 mark 无界注入。这不需要语义索引或 VCP WebSocket。

### 3.2 `river`

`river` 控制“把哪段当前对话上下文交给工具”，不是选择工具结果显示方式：

| 值 | 上游语义 | Mobile 本地回环裁决 |
|---|---|---|
| `full` | 原始多模态上下文 | P4.2 已实现；只暴露 Mobile 已拥有且通过附件/秘密/总大小策略的内容 |
| `text` | 纯文本上下文 | P2 首发；保留 role/content，总输出有界 |
| `last:N` | 最近 N 条纯文本消息 | P2 首发；N 限制 `1..50`，再受总字节预算夹紧 |
| `semantic:N` | 用工具参数作 query 选 N 条，失败回退 `last:N` | P4.3 已为 `localLoopback` 实现；固定离线模型失败时显式回退 |

CLI 普通命令默认不需要把整段聊天写进 argv。`LocalVcpMetaProcessor` 在命令启动前选取上下文，将有界 JSON 写入 attempt-private、source-unreachable、non-writeback 副本，并仅向该 Job 注入 `VCP_RIVER_CONTEXT_FILE`。guest 可以改写自己的副本，但 canonical source 不挂载、host 在启动后不再读取该副本；Job 终态后按策略清理。不把多消息 JSON 塞进 command/argv/env 值。VCPToolBox 的 `args.river_context` 仅作为上游语义参考；Mobile 当前 `vcpPlugin` route 在 Runtime 前拒绝 river，不物化远端 river。

`semantic:N` 的本地实现固定上游选取方式：用普通工具 args 中的非空字符串拼接查询（最多 2000
UTF-8 bytes），对纯文本长度大于 10 的消息计算相似度，取 Top-N 后恢复原会话顺序；向量、缓存或
Android 资产不可用时回退 `last:N`。整个路径不请求远端 embedding；`vcpPlugin` route 仍在 Runtime 前
返回 `unsupported_mode`，不能把本地资产能力冒充为远端物化能力。

### 3.3 `vref:N`

`vref` 从记忆/知识库语义召回文件并写入 `args.vref_files`。上游固定快照会用最后一条真实 user 消息和其前最近 assistant 构建加权上下文向量（默认权重 0.7/0.3），跨知识库分区搜索，按文件去重后取全局 Top-N。本地实现若宣称兼容，需用 fixture 保持这些选取规则。

当前 VCPMobile 的日记语义搜索会调用 VCPToolBox `LightMemo`，它不是本机离线向量索引；因此不能用它宣称 local loop 已离线实现 `vref`。另一个跨机问题是：上游当前生成的 `vref_files` 是 VCPToolBox 主机上的 `file://` URL，分布式 Mobile 节点不能直接读取该路径。

分两种显式能力：

- `vcp_plugin`：VCPToolBox 可继续负责语义选择。P3 已冻结为“不物化远程引用”：对 river/vref 与 VCPToolBox 主机 `file://` URL 在 Runtime 前返回 `unsupported_mode`/引用不可达，不得把它当成 guest 文件。带 hash/size/MIME 的受权 artifact 物化协议属于 P4。
- `local_loopback`：P2 返回 `unsupported_mode`；P4 增加真实本机向量索引和 knowledge grant 后，将 Top-N 文件复制为 attempt-private、source-unreachable、non-writeback 副本，通过 `VCP_VREF_DIR` 暴露。guest 可改写副本但不得回写 canonical source；不得返回 host `file://` 路径，也不得用关键字搜索冒充语义召回。

P4.4 的最小实现已冻结为单一全局 `local_vref` catalog：授权只能由用户在 CLI 页通过 native picker、
inspect 与二次确认建立；确认即 grant，不给 Agent 增加知识管理 action。知识使用独立 App 私有 CAS，首批
只收有界 UTF-8 文本/Markdown/代码；最后真实 user 与其前最近 assistant 按 0.7/0.3 合成 query，chunk
命中按文件去重后取 `1..50`。完整配额、撤权、恢复、`river + vref` 合并预算与 Android 目录 bind 合同见
[07-P4.4本机知识授权与vref合同.md](./07-P4.4本机知识授权与vref合同.md)。在 Runtime/Kotlin/设备门完成前，
local capability 仍保持关闭。

`river/vref` 上游证据：[`toolExecutor.js@311dc42`](https://github.com/lioensky/VCPToolBox/blob/311dc42e8374afd1867bd1b5c06217baf8b0f463/modules/vcpLoop/toolExecutor.js#L192-L320)。

### 3.4 `archery`

| 值 | 上游语义 | Mobile 映射 |
|---|---|---|
| `true` | 与普通调用分离并行执行，成功正文不并入普通 tool payload，错误可进入上下文 | 启动独立异步 Job；不为成功回执单独消耗模型 step |
| `no_reply` | 对异步工具允许在 grace 后静默确认，不把成功结果回灌 Agent、不触发下一轮 | 成功不续轮；已接收、失败、超时仍写 Job ledger 并向用户可见 |

并行不等于无界：Mobile 受全局 Job 并发、CPU、内存和电量预算限制。每个 `run` 是独立 Bash Job；并发任务仍可能争用 workspace 文件，不能把进程隔离误当成文件事务隔离。若 Job 在 turn finalizer 之后才失败，不能篡改已提交的模型历史；必须保留持久错误事件和 UI/通知入口，不冒充“已在同一轮回灌”。

### 3.5 DynamicTools 与 `vcp_fold`

它们解决的是“提示词里展开哪些工具/说明块”：

```text
[===vcp_fold: 0.65 ::desc: 文件与代码操作===]
完整说明...
```

`Lite` 取最低门槛摘要，`Full` 展开全部，`Auto` 做语义选择。它与历史结果、Job 输出、VCP memory 不是一个召回层。上游格式：[`foldProtocol.js@311dc42`](https://github.com/lioensky/VCPToolBox/blob/311dc42e8374afd1867bd1b5c06217baf8b0f463/modules/foldProtocol.js#L1-L69)。

### 3.6 本地语义引擎冻结合同

P4 不复用 FTS5、LightMemo 或 RAG observer 冒充向量召回，也不在 Alpine/PRoot 内安装 Python、ONNX
或 embedding 服务。首个产品候选固定为：

| 项 | 冻结值 |
|---|---|
| 模型 | `Nourh7/granite-embedding-97m-multilingual-r2` |
| revision | `b77044bfd84eef0b552c5346eeacc851264592b3` |
| license | MIT；随产品资产保留模型卡与许可归属 |
| embedding | 64 维、float16 token vectors、float64 token weights、加权 mean 后 L2 normalize |
| `model.safetensors` | 24,471,328 bytes；SHA-256 `3a416974fe644efa62c0d33970a6403b2a00e0943d376e06dfc1dae85456b10b` |
| 上游 `tokenizer.json` | 10,614,496 bytes；SHA-256 `ef17d7b13181cf34abaeebaf1c1586eb26aba7affd175d00b1dab9113aad0bdc` |
| `config.json` | 341 bytes；SHA-256 `f30b0d724a845bf677901851617e816f36686d8719260f2a238fc7ef0e9ae420` |

生产运行时不装载通用 tokenizer JSON。构建期把精确 vocab/merge 合同编译为有版本、hash 和边界校验的
紧凑只读 pack；运行时 mmap pack 与 safetensors，通过二分查找执行同一 ByteLevel BPE。逐 token fixture
必须与冻结 tokenizer 一致。上游文件配置了 `BatchLongest` padding，而 Model2Vec 推理库会把 padding ID
混入均值，使同一句话受同批最长句影响；Mobile 明确排除 padding，并以单文本、批次重排、重启三组测试
固定确定性。

构建脚本会先逐字校验上表三个上游输入，再校验 tokenizer 的 Split + ByteLevel pre-tokenizer、
`<|endoftext|>` AddedToken、padding/decoder 与 BPE flags；任何字段漂移都拒绝重建。Rust 侧已覆盖
AddedToken 的 `single_word/lstrip/rstrip` 边界、中英/emoji/空白长输入与冻结 token-ID digest；embedding
使用 float64 权重和累加，归一化后才落为 64 维 f32 派生缓存。

2026-08-14 在 OPPO PHZ110、Android 16/API 36、arm64 上的独立进程 spike：通用 `tokenizers`
实现首次/热运行峰值均约 237 MiB、总时长约 0.87–0.89 秒，否决；紧凑 pack + mmap 首次峰值
18,748 KiB、总时长 51.8 ms，热运行峰值 16,376–16,536 KiB、总时长 16.7–27.1 ms，六条中英 fixture
的非 padding token ID 逐项一致。该数据是平台 feasibility，不替代真实消息库召回准确率、温升、App PSS
或 API 26/代表 OEM 验收。

首期消息量有界，向量搜索采用确定性的 brute-force cosine；不先引入 HNSW、sqlite-vec 或平行 daemon。
向量缓存是可丢弃的派生数据，schema migration 不同步回填；首次需要时分批构建，失败时
`river=semantic:N` 按协议回退 `last:N`。`vref` 只有在显式 knowledge grant/catalog 建立后才可打开，
不能把附件数据库中的 `internal_path` 本身当作授权。

P4.3 的产品 owner 是单个 `LocalEmbeddingOwner`：所有 selection 共用一个 permit，候选集最多 512 条/
4 MiB，每次 selection 最多工作 60 秒，并检查 turn cancellation 与绝对 deadline。SQLite 只按本次候选
hash 分块读取，最多保留 20,000 行；旧 model ID、LRU 超限以及非单位/零/NaN 缓存向量都会被清理或重算。
选取完成或 fallback 后，canonical River JSON 会先与 operation ID 一起持久化，再允许 Runtime 启动命令；
恢复只重放这些字节，不因模型可用性变化重新选取。

## 4. 本地回环元协议 owner

VCP 通用字段不能由 Bash parser 零散处理。它们在 `VcpCliProtocol` 解析后交给一个 `LocalVcpMetaProcessor`：

```text
parsed tool call + bounded turn context
            ↓
LocalVcpMetaProcessor
  ├─ ink      → history visibility policy
  ├─ river    → source-unreachable、non-writeback attempt snapshot
  ├─ vref     → capability check / knowledge projection
  └─ archery  → scheduling + continuation policy
            ↓
MobileCliRuntime(run/list_skills/read_skill/poll/cancel/list)
```

MetaProcessor 拥有 N/字节上限、附件与秘密过滤、临时文件生命周期和 `unsupported_mode` 决策；Runtime 只接收已验证的 projection handle，不自行读聊天数据库。这是本地回环复现 VCP 功能的必要层，不是新的提示词系统。

## 5. 分期兼容矩阵

| 能力 | P0 parser | P2 本地 loop | P4 完整语义 |
|---|---|---|---|
| VCP marker/escape/think 排除 | fixture 冻结 | 必须 | 回归 |
| `ink=mark_history` | 识别并保留 | 有界历史摘要 | 与全局详情设置联测 |
| `river=text/last:N` | 识别与值校验 | attempt-private、source-unreachable、non-writeback JSON 副本 | 语义与预算回归 |
| `river=full` | 识别 | P4.2 已开放：有界 descriptor + attachment attempt copy | API 36 guest 读取/写攻击回归 |
| `river=semantic:N` | 识别 | P2 基线 unsupported；P4.3 已仅为 local 打开 | 固定离线模型 + 派生缓存；失败显式回退 `last:N`，远端仍 unsupported |
| `vref:N` | 识别；P4 收紧为 `1..50` | local 明确 unsupported；VCP route 仅主机 `file://` 时也 unsupported | 本机知识索引 + attempt copy grant；远端需引用物化 |
| `archery=true` | 识别 | 映射异步 Job，成功不阻塞本轮 | 并发与通知完整化 |
| `archery=no_reply` | 识别 | 成功不回灌/不续轮；失败仍可见 | VCPLog/通知联测 |
| DynamicTools | 记录上游 fixture | 由 VCPToolBox/用户配置 | 不在 Mobile 实现 |
| `vcp_fold` | parser fixture | 由 VCPToolBox 处理 | 不在 Mobile 实现 |

## 6. 验收

1. VCPToolBox 从 golden manifest 生成的 `{{VCPVCPMobileCLI}}` 完整包含 Shell、action、字段、限制以及普通执行、`list_skills/read_skill`、后台 Job、查询和取消示例。
2. Mobile 导出的 manifest 与 `vcp_plugin` 注册的 manifest 逐字一致；local route 不需要 WS，但首次 Agent 使用有明确的 VCPToolBox 配置指引。
3. Mobile 的 `context_assembler.rs`、Agent 配置和 Group 配置没有新增 CLI/Skill 提示词片段。
4. local/VCP route 使用完全相同的 manifest 内容、工具名、action 和 validator；用户在 VCPToolBox 的手工微调不产生第二份 Mobile schema。
5. 插件关闭或远端断开时不发布真实 VCP manifest；若用户保留工具说明，调用会收到明确执行错误，不声称成功。
6. `action=list_skills|read_skill` 可在飞行模式下列出/阅读已安装且校验通过的 Skill，不接受路径穿越，不自动执行脚本，也不因目录变更改写 Agent 提示词或 VCP 记忆。
7. `ink/river/vref/archery` 每种 fixture 都证明保留字段不会进入 Bash command/argv；只有受控的 projection file 路径可以通过专用 env 名进入目标 Job。
8. `river=text/last:N` 的 local fixture 与上游选取结果一致；`river=full` 只复制 hash/size 校验通过的附件，省略项带稳定原因且 canonical CAS 无 write-back；`river=semantic:N` 在 local 模型/缓存不可用时把 durable `fallback_last` 与稳定原因写入 projection/ToolResult 后继续命令，取消或 turn deadline 后不得启动命令；`vref:N` 以及远端 river/vref 仍稳定返回标注 route/capability 的 `unsupported_mode`。
