# VCPInfo 认知广播数据类型协议规范 (INFO_DATA_TYPES)

本文件是 **VCP Mobile (Project Avatar)** 移动端与 **VCPToolBox** 中间层在 `/vcpinfo` WebSocket 通道上交互的标准化数据格式规范文档。

所有通过 `pushVcpInfo` 发送的广播消息，均须严格对齐此规范中列出的数据类型、嵌套字段与语义定义。

---

## 📌 广播消息基础外壳 (Envelope)
所有原始数据包在进入 VCPInfo 分发管道时，都会在后端被提炼和封装为如下统一的元数据外壳（Metadata Envelope）。

### 统一 Metadata 结构 (前端 Pinia Store 接收格式)
```typescript
interface VcpInfoMetadata {
  id: string;         // 动态生成的唯一ID，格式：vcp_info_{timestamp_ms}_{counter}
  type: string;       // 广播事件子类型（如下文详述的 RAG_RETRIEVAL_DETAILS 等）
  title: string;      // UI 渲染主标题（例如：“元思考链: Nova”、“RAG知识库: 公共”）
  subtitle?: string;  // UI 渲染副标题（例如：“K: 3 | Time: 45ms”、“模式: VectorRecall”）
  summary: string;    // 折叠状态下的单行预览文本（限制长度并剔除冗余换行）
  timestamp: string;  // 发生时间的 ISO 8601 / RFC 3339 字符串
  hasDetails: boolean;// 指示该消息是否含有饱水详情（若为 true，前端允许点击展开并拉取 Zstd 内存缓存中的完整 Payload）
}
```

---

## 📂 核心子类型数据格式规范

```
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                                   VCPInfo 认知广播分类                                   │
├───────────────┬───────────────────┬───────────────┬───────────────────┬────────────────┤
│  RAG 检索详情  │     元思考链      │   Agent私聊   │     记忆检索      │   Agent梦境    │
│  (RAG_Detail) │ (META_THINKING)   │ (CHAT_PREV)   │  (MEMO_RETRIEVAL) │  (DREAM_*)     │
└───────────────┴───────────────────┴───────────────┴───────────────────┴────────────────┘
```

### 1. RAG 知识库检索结果 (RAG_RETRIEVAL_DETAILS / 默认 RAG 路由)
* **来源模块**：`Plugin/RAGDiaryPlugin/RAGDiaryPlugin.js`
* **触发时机**：AI 进行 RAG 知识库向量检索并召回匹配片段时。
* **Payload 完整结构**：
```typescript
interface RagRetrievalDetails {
  type: "RAG_RETRIEVAL_DETAILS";
  dbName: string;                 // 检索的数据库名称（如："VCP百科全书"）
  query: string;                  // 大模型或用户生成的检索提问句
  k: number;                      // 检索设定的最大召回项数 (k)
  useTime: boolean | number;      // 检索计时，布尔值或耗时毫秒数
  useGroup: boolean;              // 是否开启了知识库分群检索
  useRerank: boolean;             // 是否开启 Rerank 重排
  useRerankPlus: boolean;         // 是否开启强力 Rerank Plus
  rrfAlpha: number | null;        // RRF（倒数排名融合）混合检索参数
  useGeodesicRerank: boolean;     // 是否启用测地线空间关联重排
  geoAlpha?: number;              // 测地线重排混合权重 (可选)
  useExpand: boolean;             // 是否开启 Query 扩展
  useAssociate: boolean;          // 是否开启联想扩展
  associateCount?: number;        // 联想共现命中的结果项数 (V10 联想模式专属)
  useTagMemo: boolean;            // 是否使用 TagMemo 标签强度检索增强
  tagWeight: number | null;       // 标签增强权重的精确配比系数
  coreTags: string[];             // 从 Query 中提取出的候选核心语义标签列表
  timeRanges?: Array<{            // 检索过滤的时间窗口限制（ISO 范围，可选）
    start: string;
    end: string;
  }>;
  results: Array<{                // 检索召回的细分结果对象数组
    text: string;                 // 知识库/日记原文片段内容
    score?: number;               // 检索相似度得分
    source?: string;              // 检索来源标识 (如 "rag", "time", "associate")
    date?: string;                // 日记时间戳（桌面端额外输出）
    fullPath?: string;            // 相对路径路径（防穿越安全清洗）
    sourceFile?: string;          // 物理源文件名称
    matchedTags?: string[];       // 匹配命中的知识库标签列表
    tagMatchCount?: number;       // 匹配命中的标签总个数（仅在启用 TagMemo 时存在）
    boostFactor?: number;         // 标签相关性乘积计算出的 Boost 强化因子（仅在启用 TagMemo 时存在）
    associateCoCount?: number;    // 联想共现的发生次数（V10 联想模式专属）
    coreTagsMatched?: string[];   // 与 coreTags 匹配上的核心标签子集
  }>;
  tagStats?: {                    // 检索全局的标签匹配统计指标（仅在启用 TagMemo 时存在）
    uniqueMatchedTags: string[];  // 去重后的全局匹配标签集合
    totalTagMatches: number;      // 去重后的全局匹配标签总数（注：桌面端实现由于去重 size 导致与 uniqueMatchedTags.length 相等，非累计总次数）
    resultsWithTags: number;      // 携带有效标签的召回结果项数
    avgBoostFactor: string | number; // 平均增强因子系数
  };
}
```

---

### 2. 元思考链 (META_THINKING_CHAIN)
* **来源模块**：`Plugin/RAGDiaryPlugin/MetaThinkingManager.js`
* **触发时机**：AI 触发多阶段元思考（Meta Thinking）机制，循序渐进多步召回相关认知组时。
* **Payload 完整结构**：
```typescript
interface MetaThinkingChain {
  type: "META_THINKING_CHAIN";
  chainName: string;              // 当前执行的元思考链名称
  query: string;                  // 联合检索的多阶段合并输入句
  useGroup: boolean;              // 是否使用了分组检索
  activatedGroups: string[];      // 当前阶段被激活激活的分组名称列表
  totalStages: number;            // 整个思维链包含的总阶段数
  kSequence: number[];            // 预定义的各阶段分配检索项数 (k) 序列
  stages: Array<{                 // 多阶段召回分析详情
    stage: number;                // 当前阶段序号（1-indexed）
    clusterName: string;          // 阶段主题聚类特征名称
    resultCount: number;          // 本阶段的实际检索匹配数
    k: number;                    // 本阶段计划 k 值
    results: Array<{              // 阶段召回的具体数据片段
      score: number;              // 匹配得分
      text: string;               // 召回内容文本
    }>;
  }>;
  fromCache?: boolean;            // 是否来自缓存
}
```

---

### 3. Agent 私聊 / Agent 会话 (AGENT_PRIVATE_CHAT_PREVIEW)
* **来源模块**：`Plugin/AgentAssistant/AgentAssistant.js`
* **触发时机**：Agent 之间在后台触发自发性私聊通信，或者主对话之外的内心机制演练时。
* **Payload 完整结构**：
```typescript
interface AgentPrivateChatPreview {
  type: "AGENT_PRIVATE_CHAT_PREVIEW";
  agentName: string;              // 参与内心私聊演练的智能体名称
  sessionId: string;              // 演练会话 UUID 标识符
  query: string;                  // 触发演练的输入 Prompt / 清理后的提问
  response: string;               // 智能体返回的内心私聊剧场/回复文本
  timestamp: string;              // 产生的 ISO 时间戳
}
```

---

### 4. 记忆检索 (AI_MEMO_RETRIEVAL)
* **来源模块**：`Plugin/RAGDiaryPlugin/AIMemoHandler.js`
* **触发时机**：AI 跨多个库进行高维联合检索，并调用大模型对召回的碎片段进行“记忆提炼”与“联合回溯”时。
* **Payload 完整结构**：
```typescript
interface AiMemoRetrieval {
  type: "AI_MEMO_RETRIEVAL";
  dbNames: string[];              // 参与联合检索的数据库列表
  query: string;                  // 检索提问词
  mode: string;                   // 提炼与联合模式 (如 "aggregated_single", "aggregated_single_failed", "aggregated_batched")
  diaryCount: number;             // 检索命中的日记本数量
  fileCount: number;              // 检索扫描的历史文件总数
  batchCount?: number;            // 分批联合检索模式下的总批次数（分批模式专属）
  rawResponse?: string;           // 调用大模型对碎片段提炼前的原始完整响应 (可选)
  extractedMemories?: string;     // 大模型最终精炼和总结出的 Markdown 结构化记忆报告（注：调用失败即 error 存在时，此字段为 undefined）
  error?: string;                 // AI 调用失败或大模型超时的错误堆栈（可选）
  fromCache?: boolean;            // 是否来自缓存
  // --- AIMemo+ 专属字段 (可选) ---
  tagMemoChunkCount?: number;     // TagMemo/后缀管线初筛召回的 chunk 数量
  searchK?: number | null;        // 联想搜索的最大召回项数 (k)
  tagWeight?: number | null;      // 标签增强权重的精确配比系数
  sourceMode?: "suffix_pipeline" | "tagmemo_prerank"; // 检索数据源模式
  sourceFingerprint?: string;     // 检索来源的指纹哈希
}
```

---

### 5. 日记召回事件 (DailyNote)
* **来源模块**：`Plugin/RAGDiaryPlugin/RAGDiaryPlugin.js`
* **触发时机**：AI 直接命中并全文/定向检索召回某个日记本，进行前置上下文注入时。
* **Payload 完整结构**：
```typescript
interface DailyNoteEvent {
  type: "DailyNote";
  action: "FullTextRecall" | "DirectRecall" | "BM25BodyRecall" | "BM25TagRecall" | "RandomRecall"; // 召回类型（注：桌面端无 VectorRecall）
  dbName: string;                 // 被召回的日记本名称
  message: string;                // 执行结果描述日志
}
```

---

### 6. Agent 梦境子系统 (AGENT_DREAM_*)
* **来源模块**：`Plugin/AgentDream/AgentDream.js`
* **描述**：Agent 梦境在后台经历了从“采样种子”到“共鸣联想”再到“梦境叙事”的渐进流程。

#### A. 入梦开始 (AGENT_DREAM_START)
* **时机**：触发入梦瞬间。
```typescript
interface AgentDreamStart {
  type: "AGENT_DREAM_START";
  agentName: string;              // 做梦智能体名称
  dreamId: string;                // 做梦会话唯一梦境 ID
  message: string;                // "Nova 正在进入梦境..."
  timestamp: string;              // ISO时间戳
}
```

#### B. 梦境共鸣/联想完成 (AGENT_DREAM_ASSOCIATIONS)
* **时机**：种子采样和跨库 Resonance 联想关联装载完毕时。
```typescript
interface AgentDreamAssociations {
  type: "AGENT_DREAM_ASSOCIATIONS";
  agentName: string;              // 做梦智能体名称
  dreamId: string;                // 唯一梦境 ID
  seedCount: number;              // 种子日记的采样篇数
  associationCount: number;       // resonance 联想召回的日记篇数
  recentSeedsCount: number;       // 近期日记种子篇数
  midSeedsCount: number;          // 中期日记种子篇数
  deepRecallsCount: number;       // 深度记忆回溯篇数
  seeds: Array<{                  // 种子日记详情列表
    file: string;                 // 文件名
    snippet: string;              // 采样内容预览片段
  }>;
  associations: Array<{           // 联想日记详情列表
    file: string;                 // 文件名
    score: string | number;       // 联想程度相关度评分
  }>;
  timestamp: string;              // ISO时间戳
}
```

#### C. 梦叙述产出 (AGENT_DREAM_NARRATIVE)
* **时机**：梦对话结束，大模型产出梦叙事正文时。
```typescript
interface AgentDreamNarrative {
  type: "AGENT_DREAM_NARRATIVE";
  agentName: string;              // 做梦智能体名称
  dreamId: string;                // 唯一梦境 ID
  message: string;                // 梦境叙述 Markdown 文本
  narrative: string;              // 梦境叙述 Markdown 文本（支持前端 marked 渲染）
  timestamp: string;              // ISO时间戳
}
```

#### D. 梦操作处理记录 (AGENT_DREAM_OPERATIONS)
* **时机**：Agent 在梦中自动整理、提炼或清理冗余记忆而产生的意图记录。
```typescript
interface AgentDreamOperations {
  type: "AGENT_DREAM_OPERATIONS";
  agentName: string;              // 做梦智能体名称
  dreamId: string;                // 唯一梦境 ID
  operationCount?: number;        // 操作数量（桌面端额外输出）
  logFile?: string;               // 梦操作日志文件名（桌面端额外输出）
  operations: Array<{             // 梦境自发性整理操作数组
    type: "merge" | "delete" | "insight" | "unknown"; // 操作类型：合并/删除/感悟/未知
    status: "pending_review" | "approved" | "rejected" | "error"; // 审批流状态
    operationId?: string;         // 操作唯一ID（桌面端额外输出，子元素字段）
  }>;
  timestamp: string;              // ISO时间戳
}
```

#### E. 梦境流程结束 (AGENT_DREAM_END)
* **时机**：梦境流程全部流转完毕，或者期间抛出严重异常时。
```typescript
interface AgentDreamEnd {
  type: "AGENT_DREAM_END";
  agentName: string;              // 做梦智能体名称
  dreamId: string;                // 唯一梦境 ID
  status: "success" | "error";    // 终态状态（注：桌面端目前仅在入梦失败时才会发送此广播，成功时不发送）
  error?: string;                 // 若失败，包含异常描述；成功时为空
  timestamp: string;              // ISO时间戳
}
```

// 自动做梦调度开始事件（桌面端存在但原本未列入规范的广播类型）
```typescript
interface AgentDreamSchedule {
  type: "AGENT_DREAM_SCHEDULE";
  agentName: "system";            // 固定为 system
  dreamId: "scheduler";           // 固定为 scheduler
  message: string;                // 调度通知信息
  agents: string[];               // 准备入梦的智能体列表
  currentHour: number;            // 当前触发的小时
  timestamp: string;              // ISO时间戳
}
```

---

## 🛠️ 前后端对接指引
1. **消息路由及静默分发**：所有列写在规范中的认知广播事件，在 Tauri 后台的专属 `"vcp-info-event"` 通道发出。前端使用专用监听器接收，**严禁分发至全局 Toast 通知通道**，不得干扰用户的正常会话流。
2. **按需 Zstd 内存解压**：前端通过元数据数组轻量级渲染骨架。用户点击卡片触发 `toggleCard` 展开时，向后端发起 Tauri command 调用，Rust 实时在内存 Zstd 映射表里提取压缩流，就地解压生成对应结构的饱水 Payload JSON，保证移动端无感超高响应速度。
