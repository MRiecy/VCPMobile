---
id: MOD-TARVEN-016
title: Tarven 上下文注入引擎
description: 结构化提示词注入规则——system_suffix、user_suffix、context_inject 的解析、排序与注入流水线
version: 1.0.3
date: 2026-06-04
---

# 16. Tarven 上下文注入引擎

## 1. 概述

### 1.1 领域定位

`context_injection.rs` 是 VCP Mobile Rust 核心层中 **Chat 领域**的上下文注入引擎，负责在请求发送到 VCP 服务器前，根据用户定义的 Tarven 规则动态修改系统提示词、用户消息或对话上下文。其设计灵感源自 SillyTavern 的 Lorebook / World Info 系统，但针对移动端进行了架构级精简：所有注入逻辑收敛在单一 Rust 模块中，前端仅承担规则的配置与预览展示。

该模块的核心职责包括：

- **规则查询与过滤**：从 SQLite 加载激活规则，按 `scope` 过滤，按 `sort_order` 排序
- **系统提示词注入**：在原始 system prompt 前/后追加 `system_suffix` 规则内容，并自动注入基础环境信息
- **用户消息后缀**：在最新一轮用户消息文本前/后追加 `user_suffix` 规则内容（仅修改上下文，不写历史）
- **上下文节点插入**：按 `depth` 计算插入点，在对话历史指定深度插入 `context_inject` 虚拟消息
- **占位符替换**：将 `{{AgentName}}` / `{{VCPChatAgentName}}` 替换为当前 Agent 名称
- **实时预览**：通过 `preview_tarven_injection` 命令，在模拟上下文中执行与真实注入完全一致的算法

该模块**不涉及**：
- 模型推理本身（由 `vcp_client.rs` 负责）
- 消息持久化的通用逻辑（由 `message_service.rs` 负责）
- 规则 UI 的渲染与交互（由前端 `TarvenSettings.vue` / `TarvenSelector.vue` 负责）
- 同步协议实现（当前版本 `tarven_rules` 未接入 Sync V2，见 §5.3）

### 1.2 模块构成表

| 文件 | 行数 | 职责 |
|------|------|------|
| `src-tauri/src/vcp_modules/chat/context_injection.rs` | 574 | 规则引擎全部逻辑：结构体定义、查询、注入流水线、6 个 Tauri Commands |

本模块为**单文件设计**，不拆分子模块。原因：
1. 逻辑高度内聚（查询 → 过滤 → 分组 → 注入），拆分反而增加导航成本
2. 代码量 574 行，未超过 God File 警戒线（500 行接近但尚未触及重组阈值）
3. 所有函数均为无状态纯函数或异步 DB 查询，无需内部状态管理

### 1.3 在整体架构中的位置

```
┌─────────────────────────────────────────────────────────────┐
│                      Vue 3 前端层                            │
│  (TarvenSettings / TarvenSelector / InputEnhancer)         │
└──────────────────────────┬──────────────────────────────────┘
                           │ IPC (Tauri Commands)
┌──────────────────────────▼──────────────────────────────────┐
│                   src-tauri (Rust 核心层)                    │
│                                                              │
│  ┌───────────────────────────────────────────────────────┐  │
│  │              chat/ 领域                                │  │
│  │  ┌─────────────┐  ┌─────────────────────────────┐    │  │
│  │  │ context_    │  │ context_injection.rs        │    │  │
│  │  │ assembler_  │──►│ (本模块)                    │    │  │
│  │  │ utils.rs    │  │ 规则查询 / 注入流水线 / 预览  │    │  │
│  │  └─────────────┘  └─────────────┬───────────────┘    │  │
│  │                                 │                     │  │
│  │  ┌──────────────────────────────┼──────────────────┐  │  │
│  │  │                              ▼                  │  │  │
│  │  │  ┌─────────────────┐  ┌─────────────────────┐  │  │  │
│  │  │  │agent_chat_app_  │  │ group_chat_app_     │  │  │  │
│  │  │  │ service.rs      │  │ service.rs          │  │  │  │
│  │  │  │ scope="agent"   │  │ scope="group"       │  │  │  │
│  │  │  └─────────────────┘  └─────────────────────┘  │  │  │
│  │  └─────────────────────────────────────────────────┘  │  │
│  └───────────────────────────────────────────────────────┘  │
│                           │                                  │
│                           ▼                                  │
│                    ┌─────────────┐                           │
│                    │ db_manager  │                           │
│                    │ tarven_rules│                           │
│                    └─────────────┘                           │
└─────────────────────────────────────────────────────────────┘
```

注入引擎被两个应用层编排器调用：
- `agent_chat_application_service.rs`：单聊场景，传入 `scope = "agent"`
- `group_chat_application_service.rs`：群聊场景，传入 `scope = "group"`

两者共享同一套 `apply_tarven_pipeline` 逻辑，仅通过 `scope` 参数区分规则生效范围。

### 1.4 核心设计原则

| 原则 | 说明 |
|------|------|
| **后端注入，前端零感知** | 所有注入发生在 Rust 层，前端发送的原始消息不经修改直接入库，保证历史记录纯净 |
| **声明式规则，命令式执行** | 用户声明"注入什么、到哪里"，引擎负责底层的数组拼接、索引计算与排序 |
| **同类型内排序，类型间隔离** | `sort_order` 仅在同一 `rule_type` 内部生效，不同类型规则互不干扰执行顺序 |
| **Scope 二次过滤** | 前端管理全部规则列表，后端在注入时根据当前会话类型执行 `scope = 'global' OR scope = ?` 过滤 |
| **预览即实际** | `preview_tarven_injection` 与真实注入共享同一套子逻辑，消除前后端行为分叉 |
| **就地修改，零拷贝** | `apply_tarven_pipeline` 接收 `&mut Vec<Value>`，直接修改消息数组，避免中间克隆（除 `context_inject` 的 drain 重组外） |

---

## 2. 规则类型与数据结构

### 2.1 TarvenRule Rust 结构体

> 文件位置：`src-tauri/src/vcp_modules/chat/context_injection.rs` 第 7–26 行

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TarvenRule {
    pub id: String,
    pub name: String,
    pub rule_type: String, // 'system_suffix' | 'user_suffix' | 'context_inject'
    pub is_enabled: bool,
    pub content: String,
    pub scope: String,     // 'global' | 'agent' | 'group'
    pub wrap: bool,
    
    // context_inject 专用
    pub role: Option<String>, // 'user' | 'assistant'
    pub depth: Option<i32>,
    
    // system_suffix / user_suffix 专用
    pub position: Option<String>, // 'prepend' | 'append'
    
    pub sort_order: i32,
}
```

| 字段 | 类型 | 约束 | 说明 |
|------|------|------|------|
| `id` | `String` | `PRIMARY KEY` | 规则唯一标识，前端生成格式为 `rule_{timestamp}_{random}` |
| `name` | `String` | `NOT NULL` | 显示名称，仅用于前端列表展示 |
| `rule_type` | `String` | `NOT NULL` | 规则类型，三选一：`system_suffix` / `user_suffix` / `context_inject` |
| `is_enabled` | `bool` | `DEFAULT true` | 是否激活，禁用规则在 `fetch_active_rules` 中被过滤掉 |
| `content` | `String` | `NOT NULL` | 注入的原始文本内容，支持多行 |
| `scope` | `String` | `NOT NULL` | 生效范围：`global`（全部）/ `agent`（单聊）/ `group`（群聊） |
| `wrap` | `bool` | `DEFAULT true` | 是否用 `<vcp_injection>` XML 标签包裹内容 |
| `role` | `Option<String>` | `NULL` | `context_inject` 专用：虚拟消息角色，`user` 或 `assistant` |
| `depth` | `Option<i32>` | `NULL` | `context_inject` 专用：插入深度，0 表示末尾，N 表示倒数第 N+1 条之前 |
| `position` | `Option<String>` | `NULL` | `system_suffix` / `user_suffix` 专用：拼接位置，`prepend` 或 `append` |
| `sort_order` | `i32` | `DEFAULT 0` | 同类型规则间的执行顺序，升序排列 |

**Serde 映射策略**：`#[serde(rename_all = "camelCase")]` 使 Rust 的 `rule_type` / `sort_order` / `is_enabled` 自动映射到前端的 `ruleType` / `sortOrder` / `isEnabled`，零成本互操作。

### 2.2 与前端 TarvenRule TS 接口的映射

> 前端接口位置：`src/core/stores/tarvenStore.ts` 第 6–23 行

```typescript
export interface TarvenRule {
  id: string;
  name: string;
  ruleType: 'system_suffix' | 'user_suffix' | 'context_inject';
  isEnabled: boolean;
  content: string;
  scope: 'global' | 'agent' | 'group';
  wrap: boolean;
  role?: 'user' | 'assistant';
  depth?: number;
  position?: 'prepend' | 'append';
  sortOrder: number;
}
```

| 前端字段 | Rust 字段 | 传输行为 |
|----------|-----------|----------|
| `ruleType` | `rule_type` | camelCase → snake_case 自动映射 |
| `sortOrder` | `sort_order` | 同上 |
| `isEnabled` | `is_enabled` | 同上 |
| `ruleType: 'system_suffix'` | `rule_type: String` | TypeScript 字面量类型在 Rust 侧为普通 `String`，无运行时枚举校验 |
| `depth?: number` | `depth: Option<i32>` | TS `undefined` 映射到 Rust `None` |
| `position?: 'prepend' \| 'append'` | `position: Option<String>` | 同上 |

**注意**：Rust 侧不使用枚举类型（`enum RuleType`）而使用 `String`，是为了简化前端互操作与数据库存取，避免枚举序列化兼容性风险。类型正确性由前端表单控件保证。

### 2.3 三种规则类型详解

#### 2.3.1 system_suffix

在系统提示词（`role: system`）的前端或后端追加自定义内容。适用于：
- 为特定 Agent 追加长期记忆或背景设定
- 注入动态占位符内容（`{{AgentName}}` 由后端替换）
- 修改 Agent 行为边界而不改动其原始系统提示词

**拼接逻辑**：

```
[前置规则内容 A]

[前置规则内容 B]

[基础环境注入]
[原始系统提示词]

[后置规则内容 C]

[后置规则内容 D]
```

- 所有 `position = "prepend"` 的规则按 `sort_order` 升序拼接后，置于原始提示词之前
- 所有 `position = "append"` 的规则按 `sort_order` 升序拼接后，置于原始提示词之后
- 前后置组内部以 `\n\n`（双换行）分隔
- **基础环境注入**（`inject_base_environment`）在所有 `system_suffix` 规则之前自动执行，追加当前时间、运行环境、话题创建时间（见 §3.3）

#### 2.3.2 user_suffix

在最新一轮用户消息文本的前端或后端追加内容。适用于：
- 在用户输入后自动附加格式要求（如"请用 Markdown 回复"）
- 注入临时指令而不改变用户原始输入的视觉效果
- 与 `system_suffix` 配合实现"系统层设定 + 用户层微调"的双层控制

**关键区别**：`user_suffix` 仅修改发往 LLM 的上下文载荷，本地数据库中存储的仍是用户原始输入。这保证了历史记录的可读性与可回溯性。流式输出期间，用户看到的仍是自己输入的原始文本，模型接收的则是附加后的完整版本。

**定位策略**：使用 `messages.iter().rposition(|m| m["role"].as_str() == Some("user"))` 定位数组中**最后一个** `user` 消息。若不存在用户消息（如首次对话仅含 system），则跳过注入。

#### 2.3.3 context_inject

在对话历史的指定深度插入一条虚拟消息，作为独立节点存在。适用于：
- 在上下文末尾注入"总结性指令"引导模型回复风格
- 在特定位置插入参考文档或 Few-shot 示例
- 实现类似"系统消息但放在上下文末尾"的 Jailbreak 技巧

**深度语义**：
- `depth = 0`：插入到非系统消息的最末尾（紧接在最新用户消息之后）
- `depth = N`：从末尾向前数第 `N+1` 条消息之前

**插入公式**：
```rust
let insert_index = if non_system_msgs.len() > depth {
    non_system_msgs.len() - depth
} else {
    0
};
```

**示例**（4 条非系统消息，2 条注入规则）：
```
原始:    [u1, a1, u2, a2]            (u=用户, a=助手)
规则 X:  depth=0, role=assistant     → 插入到末尾之后（index=4）
规则 Y:  depth=2, role=user          → 插入到 u2 之前（index=2）

结果:    [u1, a1, Y, u2, a2, X]
```

**注入标记**：所有 `context_inject` 插入的消息均带有 `"__tavernInjected": true` 字段，供前端预览时高亮展示，实际发往 VCP 服务器的消息也保留此字段（VCP 服务器可忽略未知字段）。

### 2.4 Scope 过滤机制

> 过滤逻辑实现位置：`src-tauri/src/vcp_modules/chat/context_injection.rs` 第 43–76 行

```sql
SELECT ... FROM tarven_rules 
WHERE is_enabled = 1 AND (scope = 'global' OR scope = ?)
ORDER BY sort_order ASC
```

| scope 值 | 生效场景 | 参数绑定 |
|----------|----------|----------|
| `global` | 所有会话类型 | 无条件匹配（`OR` 左侧恒真） |
| `agent` | 仅单聊会话 | `scope = 'agent'` |
| `group` | 仅群聊会话 | `scope = 'group'` |

**设计要点**：
1. `global` 规则与特定 scope 规则**叠加生效**，而非互斥。即单聊会话同时加载所有 `global` 规则和 `agent` 规则
2. 过滤发生在 SQLite 查询层，而非内存过滤，利用复合索引 `idx_tarven_rules_active` 加速
3. 前端展示全部规则（由 `get_tarven_rules` 返回），实际注入时由后端二次过滤，前端无需感知当前会话类型

---

## 3. 注入流水线

### 3.1 规则加载与排序

> 函数位置：`src-tauri/src/vcp_modules/chat/context_injection.rs` 第 43–76 行

```rust
pub async fn fetch_active_rules(
    pool: &Pool<Sqlite>,
    scope: &str,
) -> Result<Vec<TarvenRule>, String> { ... }
```

**查询策略**：
1. 条件：`is_enabled = 1`（仅激活规则）
2. 过滤：`scope = 'global' OR scope = ?`（叠加匹配）
3. 排序：`ORDER BY sort_order ASC`（升序，值小的先执行）
4. 绑定：参数化查询防止 SQL 注入

**SQLite → Rust 类型转换**：
- `INTEGER` 布尔字段（`is_enabled`、`wrap`）通过 `row.get::<i32, _>("...") != 0` 转为 `bool`
- `NULL` 字段（`role`、`depth`、`position`）通过 `sqlx::Row::get` 自动映射到 `Option<T>`

### 3.2 规则解析与验证

本模块采用**弱校验**策略：不拒绝前端传入的非法规则，而是在注入时安全降级。

| 字段 | 非法值示例 | 降级行为 |
|------|-----------|----------|
| `rule_type` | `"unknown_type"` | 被 `filter(|r| r.rule_type == "...")` 过滤掉，不执行任何操作 |
| `position` | `"center"` | 非 `"prepend"` 时统一视为 `"append"`（`as_deref() == Some("prepend")` 判断） |
| `role` | `"system"` | 非 `"assistant"` 时统一视为 `"user"`（`unwrap_or("user")`） |
| `depth` | 负数或超大值 | 负数：`unwrap_or(0)`；超大值：`insert_index = 0`（数组长度不足时回退到头部） |

这种设计的理由是：规则由前端表单控件生成，正常情况下不会出现非法值；后端弱校验可在极端异常下保持流水线不崩溃，而非 panic 或返回错误阻断用户聊天。

### 3.3 上下文注入时机

> 核心函数位置：`src-tauri/src/vcp_modules/chat/context_injection.rs` 第 103–273 行

`apply_tarven_pipeline` 是注入引擎的唯一收敛点，按以下 4 个阶段顺序执行：

```
输入: pool, topic_id, agent_name, scope, &mut messages
       │
       ▼
┌─────────────────────────────────────────────┐
│ 阶段 1: 加载规则                             │
│ fetch_active_rules(pool, scope)             │
│ ──> Vec<TarvenRule>（已过滤+已排序）          │
└─────────────────────┬───────────────────────┘
                      ▼
┌─────────────────────────────────────────────┐
│ 阶段 2: System Prompt 注入                   │
│ 2a. inject_base_environment()               │
│     自动追加: 当前时间 / 运行环境 / 话题创建时间 │
│ 2b. 分离 prepend / append 规则并拼接          │
│ 2c. 占位符替换: {{AgentName}} → agent_name    │
│ 2d. 回写或插入 system 消息到数组首位          │
└─────────────────────┬───────────────────────┘
                      ▼
┌─────────────────────────────────────────────┐
│ 阶段 3: User Suffix 注入                     │
│ 3a. 定位最后一个 user 消息（rposition）        │
│ 3b. 分离 prepend / append 规则并拼接          │
│ 3c. 就地修改该 user 消息的 content 字段        │
└─────────────────────┬───────────────────────┘
                      ▼
┌─────────────────────────────────────────────┐
│ 阶段 4: Context Inject 节点插入              │
│ 4a. 分离 system / non-system 消息            │
│ 4b. 按 depth 从大到小排序规则                │
│ 4c. 逐个计算 insert_index 并插入虚拟消息      │
│ 4d. 重组数组: system + non-system            │
└─────────────────────────────────────────────┘
```

**阶段 2 细节——基础环境注入**：

> 函数位置：`src-tauri/src/vcp_modules/chat/context_injection.rs` 第 78–100 行

```rust
async fn inject_base_environment(pool, topic_id, system_prompt) {
    let now = Local::now().format("%Y-%m-%d %H:%M:%S %Z");
    let mut prepend = format!(
        "当前系统时间: {}\n运行环境: VCP Mobile (Android 移动端)\n", 
        now
    );
    // 若 topic 存在，追加话题创建时间
    if let Ok(Some(row)) = query("SELECT created_at FROM topics WHERE topic_id = ?")... {
        prepend.push_str(&format!("当前话题创建于: {}\n", dt));
    }
    prepend.push_str("\n---\n\n");
    system_prompt.insert_str(0, &prepend);
}
```

基础环境注入**无条件执行**，即使没有任何 `system_suffix` 规则激活。这确保了每次请求都携带最新的时间与运行环境上下文，供模型推理时参考。

**阶段 2 细节——system_suffix 拼接代码**：

> 实现位置：`src-tauri/src/vcp_modules/chat/context_injection.rs` 第 126–159 行

```rust
let system_rules: Vec<&TarvenRule> = rules
    .iter()
    .filter(|r| r.rule_type == "system_suffix")
    .collect();

let mut system_prepend_parts = Vec::new();
let mut system_append_parts = Vec::new();

for rule in system_rules {
    let rendered = render_rule_content(rule);
    if rule.position.as_deref() == Some("prepend") {
        system_prepend_parts.push(rendered);
    } else {
        system_append_parts.push(rendered);
    }
}

if !system_prepend_parts.is_empty() {
    let prepend_str = system_prepend_parts.join("\n\n");
    if !system_content.is_empty() {
        system_content = format!("{}\n\n{}", prepend_str, system_content);
    } else {
        system_content = prepend_str;
    }
}

if !system_append_parts.is_empty() {
    let append_str = system_append_parts.join("\n\n");
    if !system_content.is_empty() {
        system_content = format!("{}\n\n{}", system_content, append_str);
    } else {
        system_content = append_str;
    }
}
```

**阶段 3 细节——user_suffix 拼接代码**：

> 实现位置：`src-tauri/src/vcp_modules/chat/context_injection.rs` 第 180–221 行

```rust
let user_rules: Vec<&TarvenRule> = rules
    .iter()
    .filter(|r| r.rule_type == "user_suffix")
    .collect();

if !user_rules.is_empty() {
    if let Some(user_idx) = messages.iter().rposition(|m| m["role"].as_str() == Some("user")) {
        let mut user_content = messages[user_idx]["content"].as_str().unwrap_or("").to_string();
        
        let mut user_prepend_parts = Vec::new();
        let mut user_append_parts = Vec::new();

        for rule in user_rules {
            let rendered = render_rule_content(rule);
            if rule.position.as_deref() == Some("prepend") {
                user_prepend_parts.push(rendered);
            } else {
                user_append_parts.push(rendered);
            }
        }
        // ... 拼接逻辑与 system_suffix 相同
        messages[user_idx]["content"] = serde_json::Value::String(user_content);
    }
}
```

**阶段 4 细节——context_inject 插入代码**：

> 实现位置：`src-tauri/src/vcp_modules/chat/context_injection.rs` 第 224–270 行

```rust
let context_rules: Vec<&TarvenRule> = rules
    .iter()
    .filter(|r| r.rule_type == "context_inject")
    .collect();

if !context_rules.is_empty() {
    let mut system_msgs = Vec::new();
    let mut non_system_msgs = Vec::new();

    for msg in messages.drain(..) {
        if msg["role"].as_str() == Some("system") {
            system_msgs.push(msg);
        } else {
            non_system_msgs.push(msg);
        }
    }

    // 根据 depth 从大到小排列
    let mut sorted_context_rules = context_rules;
    sorted_context_rules.sort_by(|a, b| {
        let depth_b = b.depth.unwrap_or(0);
        let depth_a = a.depth.unwrap_or(0);
        depth_b.cmp(&depth_a)
    });

    for rule in sorted_context_rules {
        let role = rule.role.as_deref().unwrap_or("user");
        let depth = rule.depth.unwrap_or(0) as usize;
        let insert_index = if non_system_msgs.len() > depth {
            non_system_msgs.len() - depth
        } else {
            0
        };

        let virtual_msg = serde_json::json!({
            "role": role,
            "content": render_rule_content(rule),
            "__tavernInjected": true
        });

        non_system_msgs.insert(insert_index, virtual_msg);
    }

    messages.extend(system_msgs);
    messages.extend(non_system_msgs);
}
```

**阶段 4 细节——depth 从大到小排序**：

> 实现位置：`src-tauri/src/vcp_modules/chat/context_injection.rs` 第 241–247 行

```rust
sorted_context_rules.sort_by(|a, b| {
    let depth_b = b.depth.unwrap_or(0);
    let depth_a = a.depth.unwrap_or(0);
    depth_b.cmp(&depth_a)  // 降序
});
```

原因：若按升序插入，先插入的节点会使数组变长，导致后续 `depth` 计算错位。从大到小排序后，大 `depth`（靠近数组头部）先插入，小 `depth`（靠近数组尾部）后插入，后续规则的 `non_system_msgs.len() - depth` 计算不受前面插入影响。

### 3.4 注入位置决策树

```
规则 rule_type 是什么?
│
├─ system_suffix
│   ├─ position == "prepend"?
│   │   ├─ Yes → 拼接到原始 system prompt 之前
│   │   └─ No  → 拼接到原始 system prompt 之后
│   └─ 结果: 数组首位 system 消息被替换或新建
│
├─ user_suffix
│   ├─ 数组中存在 user 消息?
│   │   ├─ Yes → 定位最后一个 user 消息
│   │   │       ├─ position == "prepend"? → 拼接到该消息文本之前
│   │   │       └─ position == "append"?  → 拼接到该消息文本之后
│   │   └─ No  → 跳过注入（无目标消息）
│   └─ 结果: 该 user 消息的 content 字段被修改，其他消息不变
│
└─ context_inject
    ├─ 分离 system 与 non-system 消息
    ├─ 按 depth 降序遍历规则
    │   └─ insert_index = max(0, non_system_msgs.len() - depth)
    │       → 在 non_system_msgs 的指定位置插入虚拟消息
    └─ 结果: 数组重组为 [system...] + [non_system（含注入节点）...]
```

---

## 4. 与 Agent 聊天应用层的集成

### 4.1 在 10 步流水线中的注入点

> 文件位置：`src-tauri/src/vcp_modules/agent/agent_chat_application_service.rs`

`agent_chat_application_service.rs` 的 `internal_process_agent_chat_message` 函数定义了 Backend-Driven Streaming 架构的 10 步编排流水线。Tarven 注入发生在**第 5 步与第 6 步之间**：

```
步骤 4: assemble_history_for_vcp(&history)
    ──> 将本地 ChatMessage[] 转为 VCP API 兼容的 messages[]
        
步骤 5: 插入 System Prompt
    ──> effective_prompt = mobile_system_prompt ?? system_prompt
    ──> messages.insert(0, { role: "system", content: effective_prompt })
        
步骤 5.5: Tarven 上下文注入 ★
    ──> apply_tarven_pipeline(pool, topic_id, agent_name, "agent", &mut messages)
    ──> 系统后缀拼接 / 用户后缀追加 / 上下文节点插入
    ──> 占位符替换: {{AgentName}} → agent_config.name
        
步骤 6: 后端生成 Thinking ID
步骤 7: 构造 VCP 请求载荷
...
```

**为什么是第 5.5 步？**

1. **必须在 assemble_history_for_vcp 之后**：需要基于完整历史记录进行 `context_inject` 的深度计算
2. **必须在 System Prompt 插入之后**：`system_suffix` 需要操作已有的 system 消息，或在没有 system 时创建新的 system 消息
3. **必须在构造 VCP 请求载荷之前**：注入后的 messages 数组直接作为 `VcpRequestPayload.messages` 发往服务器

群聊流水线（`group_chat_application_service.rs` 第 220 行）遵循完全相同的时序，仅将 `scope` 参数改为 `"group"`：

> 代码位置：`src-tauri/src/vcp_modules/group/group_chat_application_service.rs` 第 220–227 行

```rust
crate::vcp_modules::chat::context_injection::apply_tarven_pipeline(
    &db_pool,
    &topic_id,
    &agent_name,
    "group",
    &mut messages,
)
.await?;
```

### 4.2 与 assemble_history_for_vcp 的协作

> 文件位置：`src-tauri/src/vcp_modules/chat/context_assembler.rs`

`assemble_history_for_vcp` 负责将本地 `ChatMessage` 历史转换为 VCP API 格式的 `Vec<Value>`，其输出是 Tarven 注入的**输入前提**：

| 协作点 | assemble_history_for_vcp | apply_tarven_pipeline |
|--------|-------------------------|----------------------|
| 输入 | `&[ChatMessage]` | `&mut Vec<Value>` |
| 输出 | 含 `role`/`name`/`content` 的 JSON 消息数组 | 就地修改后的数组 |
| system 处理 | 不生成 system 消息（历史记录无 system） | 创建或修改 system 消息 |
| user 处理 | 保留原始 content | 追加 suffix 到最新 user 的 content |
| 附件文本 | 将 `extracted_text` 拼接到 content | 在已拼接的 content 上继续追加 |

**数据流**：
```
数据库历史记录 (ChatMessage[])
    │
    ▼
assemble_history_for_vcp()
    │
    ▼
Vec<Value> messages （无 system，role ∈ {user, assistant}）
    │
    ▼
messages.insert(0, system_prompt)  ← 由 agent_chat_app_service 执行
    │
    ▼
apply_tarven_pipeline(&mut messages)
    │
    ▼
注入后的 messages → VcpRequestPayload → perform_vcp_request()
```

### 4.3 预览机制（preview_tarven_injection）

> 函数位置：`src-tauri/src/vcp_modules/chat/context_injection.rs` 第 411–573 行

```rust
#[tauri::command]
pub async fn preview_tarven_injection(
    rules: Vec<TarvenRule>,
    mock_messages: Option<Vec<serde_json::Value>>,
) -> Result<Vec<serde_json::Value>, String> { ... }
```

**与真实注入的核心差异**：

| 维度 | 真实注入 `apply_tarven_pipeline` | 预览 `preview_tarven_injection` |
|------|----------------------------------|--------------------------------|
| 规则来源 | `fetch_active_rules(pool, scope)`（DB 查询） | 前端传入 `rules` 参数（草稿规则） |
| 消息来源 | 实际对话历史 + System Prompt | 默认模拟上下文（4 条消息）或用户传入 `mock_messages` |
| 环境注入 | 查询 `topics.created_at` + 当前真实时间 | 使用当前时间模拟话题创建时间 |
| 占位符替换 | `{{AgentName}}` → `agent_config.name` | `{{AgentName}}` → `"秋水智能体"`（硬编码） |
| 写库操作 | 无（注入为纯内存操作） | 无 |
| 返回结果 | `Result<(), String>` | `Result<Vec<Value>, String>`（返回完整注入后数组） |

**共享子逻辑**：预览函数与真实函数共享 `render_rule_content` 以及三段拼接/插入算法。这保证了"预览即实际"的一致性承诺。代码层面，两段逻辑的重复性较高（约 160 行相似代码），这是刻意为之：将公共逻辑提取为辅助函数会增加抽象层，而预览与实际注入的上下文差异（如 `inject_base_environment` 的异步 DB 查询 vs 同步模拟）使完全统一变得复杂。当前 574 行的模块规模下，代码重复是可接受的维护成本。

**默认模拟上下文**（4 条消息）：
```json
[
  { "role": "system", "content": "你是一个智能助手。" },
  { "role": "user", "content": "你好，请问你是？" },
  { "role": "assistant", "content": "我是你的 AI 助手，有什么可以帮你的吗？" },
  { "role": "user", "content": "帮我写一首关于秋天的诗。" }
]
```

---

## 5. 数据库与持久化

### 5.1 tarven_rules 表结构

> 定义位置：`src-tauri/src/vcp_modules/persistence/db_manager.rs` 第 309–329 行

```sql
CREATE TABLE IF NOT EXISTS tarven_rules (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    rule_type TEXT NOT NULL,
    is_enabled INTEGER NOT NULL DEFAULT 1,
    content TEXT NOT NULL,
    scope TEXT NOT NULL,
    wrap INTEGER NOT NULL DEFAULT 1,
    role TEXT,
    depth INTEGER,
    position TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
)
```

| 字段 | 类型 | 约束 | 说明 |
|------|------|------|------|
| `id` | `TEXT` | `PRIMARY KEY` | 规则唯一标识 |
| `name` | `TEXT` | `NOT NULL` | 显示名称 |
| `rule_type` | `TEXT` | `NOT NULL` | `system_suffix` / `user_suffix` / `context_inject` |
| `is_enabled` | `INTEGER` | `DEFAULT 1` | 布尔值：0=禁用，1=启用 |
| `content` | `TEXT` | `NOT NULL` | 注入内容 |
| `scope` | `TEXT` | `NOT NULL` | `global` / `agent` / `group` |
| `wrap` | `INTEGER` | `DEFAULT 1` | 布尔值：是否 XML 包裹 |
| `role` | `TEXT` | `NULL` | `context_inject` 专用：`user` 或 `assistant` |
| `depth` | `INTEGER` | `NULL` | `context_inject` 专用：插入深度 |
| `position` | `TEXT` | `NULL` | `system_suffix` / `user_suffix` 专用：`prepend` / `append` |
| `sort_order` | `INTEGER` | `DEFAULT 0` | 同类型排序权重 |
| `created_at` | `BIGINT` | `NOT NULL` | 创建时间（毫秒级 Unix 时间戳） |
| `updated_at` | `BIGINT` | `NOT NULL` | 更新时间（毫秒级 Unix 时间戳） |

**索引**：
> 定义位置：`src-tauri/src/vcp_modules/persistence/db_manager.rs` 第 344 行

```sql
CREATE INDEX IF NOT EXISTS idx_tarven_rules_active 
ON tarven_rules(rule_type, is_enabled, sort_order ASC);
```

该复合索引支撑 `fetch_active_rules` 的核心过滤场景：`rule_type` 用于潜在分组 → `is_enabled = 1` 筛选 → `sort_order` 排序。尽管当前查询使用 `scope` 过滤而非 `rule_type`，但索引的前缀 `rule_type` 仍可为 `get_tarven_rules`（全量查询）提供顺序保证。

### 5.2 UPSERT 语义

> 实现位置：`src-tauri/src/vcp_modules/chat/context_injection.rs` 第 312–355 行

`save_tarven_rule` 使用 SQLite `INSERT ... ON CONFLICT(id) DO UPDATE SET ...` 实现原子化 UPSERT：

```sql
INSERT INTO tarven_rules (id, name, ..., created_at, updated_at) 
VALUES (?, ?, ..., ?, ?)
ON CONFLICT(id) DO UPDATE SET 
    name = excluded.name,
    rule_type = excluded.rule_type,
    is_enabled = excluded.is_enabled,
    content = excluded.content,
    scope = excluded.scope,
    wrap = excluded.wrap,
    role = excluded.role,
    depth = excluded.depth,
    position = excluded.position,
    sort_order = excluded.sort_order,
    updated_at = excluded.updated_at
```

- `id` 已存在 → 更新全部字段（除 `created_at` 外），`updated_at` 刷新为当前时间
- `id` 不存在 → 新建记录，`created_at` 与 `updated_at` 均为当前时间
- 时间戳使用毫秒级 Unix 时间戳（`Local::now().timestamp_millis()`）

**`reorder_rules` 的事务保证**：
> 实现位置：`src-tauri/src/vcp_modules/chat/context_injection.rs` 第 389–409 行

```rust
let mut tx = db_state.pool.begin().await?;
for (index, id) in rule_ids.iter().enumerate() {
    sqlx::query("UPDATE tarven_rules SET sort_order = ?, updated_at = ? WHERE id = ?")
        .bind(index as i32)
        .bind(now)
        .bind(id)
        .execute(&mut *tx).await?;
}
tx.commit().await?;
```

全有或全无（All-or-Nothing）：任一规则更新失败则整个排序操作回滚，保证 `sort_order` 的连续性与一致性。

### 5.3 同步联动（Sync V2）

**当前状态**：`tarven_rules` 表**未接入** Sync V2 同步协议。表结构中虽包含 `created_at` / `updated_at` 时间戳，但模块内无 `sync_service` 调用、无同步 DTO、无哈希变更检测。

**原因与展望**：
1. Tarven 规则目前被定位为"本地个性化配置"，不同设备的规则集可能差异较大（如手机端与平板端使用不同规则）
2. 规则内容通常与特定 Agent/Group 的本地实验相关，同步价值相对较低
3. 未来若需同步，可参照 `agent_service.rs` 的 `AgentSyncDTO` + `HashAggregator` 模式，为 `tarven_rules` 增加 `config_hash` 字段与同步通知逻辑

---

## 6. 函数接口表

### 6.1 Tauri Commands（暴露给前端）

> 命令注册位置：`src-tauri/src/lib.rs` 第 16–18 行

```rust
use vcp_modules::context_injection::{
    get_tarven_rules, save_tarven_rule, delete_tarven_rule,
    toggle_rule_enabled, reorder_rules, preview_tarven_injection,
};
```

| 命令名 | 签名（Rust） | 输入 | 输出 | 前端调用方 |
|--------|-------------|------|------|----------|
| `get_tarven_rules` | `async fn(db_state) -> Result<Vec<TarvenRule>, String>` | `DbState` | 全部规则列表（按 `sort_order` 升序） | `tarvenStore.fetchRules()` |
| `save_tarven_rule` | `async fn(db_state, rule) -> Result<(), String>` | `DbState`, `TarvenRule` | `()` | `tarvenStore.saveRule()` |
| `delete_tarven_rule` | `async fn(db_state, id) -> Result<(), String>` | `DbState`, `String` | `()` | `tarvenStore.deleteRule()` |
| `toggle_rule_enabled` | `async fn(db_state, id, enabled) -> Result<(), String>` | `DbState`, `String`, `bool` | `()` | `tarvenStore.toggleRule()` |
| `reorder_rules` | `async fn(db_state, rule_ids) -> Result<(), String>` | `DbState`, `Vec<String>` | `()` | `tarvenStore.saveOrder()` |
| `preview_tarven_injection` | `async fn(rules, mock_messages) -> Result<Vec<Value>, String>` | `Vec<TarvenRule>`, `Option<Vec<Value>>` | 注入后的消息数组 | `tarvenStore.previewInjection()` |

### 6.2 内部函数

| 函数签名 | 可见性 | 调用方 | 说明 |
|----------|--------|--------|------|
| `fetch_active_rules(pool, scope) -> Result<Vec<TarvenRule>, String>` | `pub` | `apply_tarven_pipeline` | 按 scope 过滤并排序的激活规则查询 |
| `inject_base_environment(pool, topic_id, system_prompt)` | private | `apply_tarven_pipeline` | 自动追加时间、环境、话题创建时间 |
| `apply_tarven_pipeline(pool, topic_id, agent_name, scope, messages)` | `pub` | `agent_chat_application_service`, `group_chat_application_service` | 核心注入流水线，就地修改消息数组 |
| `render_rule_content(rule) -> String` | private | `apply_tarven_pipeline`, `preview_tarven_injection` | XML 包裹处理与原始内容渲染 |

---

## 7. 模块依赖关系

```
                        ┌─────────────────┐
                        │  tarven_rules   │
                        │   (SQLite 表)    │
                        └────────┬────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────┐
│              chat/context_injection.rs                       │
│  ┌───────────────┐  ┌─────────────────────────────────────┐ │
│  │Tauri Commands │  │ apply_tarven_pipeline()             │ │
│  │ (6 个命令)    │  │ fetch_active_rules()                │ │
│  └───────────────┘  │ inject_base_environment()           │ │
│                     │ render_rule_content()               │ │
│                     └─────────────────────────────────────┘ │
└────────────────────────┬────────────────────────────────────┘
                         │
           ┌─────────────┼─────────────┐
           ▼             ▼             ▼
┌─────────────────┐ ┌─────────┐ ┌─────────────────┐
│ agent_chat_app_ │ │ db_manager│ │ group_chat_app_ │
│ service.rs      │ │ (DbState) │ │ service.rs      │
│ (scope="agent") │ │         │ │ (scope="group") │
└─────────────────┘ └─────────┘ └─────────────────┘
```

**外部依赖表**：

| 被依赖模块 | 用途 |
|-----------|------|
| `db_manager::DbState` | SQLite 连接池，用于规则查询与持久化 |
| `sqlx` | 异步 SQL 查询与事务 |
| `chrono` | 本地时间格式化（`Local::now()`） |
| `serde_json::Value` | 消息数组的通用 JSON 表示 |

**无循环依赖**：`context_injection.rs` 仅依赖基础设施层（`db_manager`）与第三方 crate，不依赖 `agent/`、`group/` 等上层领域模块。注入引擎被应用层调用，但自身不反向引用应用层，保持了清晰的依赖方向。

---

## 8. 设计决策与注意事项

### 8.1 为何注入在 Rust 层而非前端？

1. **历史纯净性**：前端注入会污染本地消息数据库。`user_suffix` 若在前端执行，用户原始输入与后缀指令会被合并后入库，导致历史记录不可回溯
2. **安全性**：后端注入确保规则生效不受前端代码篡改影响。即使前端被绕过，后端仍执行 `scope` 过滤与内容拼接
3. **一致性**：所有注入逻辑收敛在单一 Rust 模块，避免 Android WebView 与桌面端（未来可能）的行为分叉
4. **占位符上下文**：`{{AgentName}}` 需要访问 `agent_config.name`，该信息在 Rust 层已加载，无需额外 IPC 传输

### 8.2 为何用 sort_order 而非优先级数字？

- **`sort_order` 是位置索引**：值直接决定规则在列表中的物理顺序（0, 1, 2, 3...），语义为"排在第几位"
- **优先级数字具有歧义**：高数值代表高优先还是低优先？不同开发者理解相反
- **拖拽排序直观**：前端拖拽重排后，只需将新顺序的 ID 数组传给 `reorder_rules`，后端按数组索引重写 `sort_order`，无需计算优先级差值
- **同类型隔离**：`sort_order` 仅在同一 `rule_type` 内比较，不同类型规则即使数值交叉也不互相干扰

### 8.3 context_inject 的 depth 从大到小排序的原因

当多条 `context_inject` 规则同时生效时，按 `depth` 降序排序后依次插入，可避免因前面插入导致数组长度变化、进而使后续 `depth` 计算错位的问题。

```
规则 A: depth=5, 规则 B: depth=0
排序后: A(5) → B(0)

插入前: [系统, u1, a1, u2, a2, u3]  (非系统消息长度 6)
A 插入到 index=1 (6-5=1)
插入后: [系统, A, u1, a1, u2, a2, u3]  (非系统消息长度 7)
B 插入到 index=7 (7-0=7，即末尾)
最终: [系统, A, u1, a1, u2, a2, u3, B]
```

若从小到大排序，先插入 B(depth=0) 到末尾（index=6），再插入 A 时数组已变长为 7，A 的 `depth=5` 将插入到 index=2（7-5=2），而非预期的 index=1，导致错位。

### 8.4 为何 user_suffix 不写历史记录？

`user_suffix` 仅修改发往 LLM 的上下文载荷，用户本地数据库中的消息保持原始内容。这一决策基于三个考量：

1. **可回溯性**：用户日后回看聊天记录时，不应看到被自动附加的指令文本（如"请用 Markdown 回复"）
2. **可编辑性**：如果用户编辑历史消息重新发送，不应重复附加后缀，否则会导致指令堆叠
3. **隐私性**：某些后缀可能包含临时指令或敏感提示，不应永久留存于本地数据库

### 8.5 wrap（XML 包裹）的用途

当 `wrap = true` 时，注入内容被包裹在自定义 XML 标签中：

```xml
<vcp_injection description="由 VCPMobile 注入">
{规则内容}
</vcp_injection>
```

这有助于：
1. 让模型明确识别"这是外部注入的元指令，而非用户原始输入"
2. 与一些遵循 XML 标签语义的模型（如 Claude 系列）更好地协作
3. 在预览中高亮显示注入边界（通过 `__tavernInjected` 标记）
4. 便于未来在消息渲染器中识别并特殊展示注入内容

### 8.6 单文件设计的边界

`context_injection.rs` 当前 574 行，包含结构体定义、6 个 Tauri Commands、3 个内部函数。虽未触及 500 行警戒线过多，但已接近。未来若增加以下功能，应考虑拆分为 `tarven_engine.rs` + `tarven_commands.rs`：
- 条件触发机制（关键词匹配、正则过滤）
- 规则模板系统（预设规则库）
- 同步协议接入（Sync V2 DTO 与哈希计算）

### 8.7 为何 rule_type 使用 String 而非枚举？

Rust 侧 `TarvenRule.rule_type` 使用 `String` 而非 `enum RuleType`，原因如下：
1. **序列化简化**：`String` 可直接存入 SQLite 且无需自定义 `sqlx::Type` 实现；枚举需要额外编码（如整数映射或字符串转换层）
2. **前端互操作**：Tauri v2 的 IPC 传输中，TS 联合类型 `'system_suffix' | 'user_suffix' | 'context_inject'` 与 Rust `String` 天然对齐，无需枚举的 serde 属性调优
3. **扩展性**：未来新增规则类型时，无需修改 Rust 枚举定义并重新编译，只需前端表单增加选项即可（后端弱校验会自动忽略未知类型）
4. **代价**：损失编译期类型检查，但模块内通过 `filter(|r| r.rule_type == "...")` 的显式字符串比较弥补，且该模块为内部实现，不对外暴露类型契约

---

## 9. 术语速查表

| 术语 | 英文/缩写 | 定义 | 相关文件 |
|------|----------|------|----------|
| Tarven | — | VCP Mobile 的上下文注入规则系统，名称致敬 SillyTavern | 全部 |
| TarvenRule | — | 单条注入规则的数据结构，定义注入内容、类型、范围与参数 | `context_injection.rs`, `tarvenStore.ts` |
| system_suffix | 系统提示词注入 | 在系统提示词前/后追加内容的规则类型 | `context_injection.rs` |
| user_suffix | 用户消息注入 | 在最新用户消息前/后追加内容的规则类型 | `context_injection.rs` |
| context_inject | 上下文消息注入 | 在对话历史指定深度插入虚拟消息的规则类型 | `context_injection.rs` |
| scope | 作用范围 | 规则生效的会话类型：`global` / `agent` / `group` | `context_injection.rs`, `tarvenStore.ts` |
| wrap | XML 包裹 | 是否用 `<vcp_injection>` 标签包裹注入内容 | `context_injection.rs` |
| depth | 插入深度 | `context_inject` 专用：0=末尾，N=倒数第 N+1 条之前 | `context_injection.rs` |
| position | 拼接位置 | `system_suffix` / `user_suffix` 专用：`prepend` 前置 / `append` 后置 | `context_injection.rs` |
| sort_order | 排序权重 | 同类型规则间的执行顺序，升序排列 | `context_injection.rs` |
| 基础环境注入 | Base Env Injection | 后端自动在系统提示词前追加当前时间、运行环境等信息 | `context_injection.rs` |
| `__tavernInjected` | 注入标记 | 预览中标记哪些消息是由规则注入产生的元数据字段 | `context_injection.rs` |
| UPSERT | — | SQLite 的 `INSERT ... ON CONFLICT DO UPDATE` 原子操作 | `context_injection.rs` |
| 预览即实际 | Preview-as-Actual | 预览命令执行与真实注入完全一致的算法，消除行为分叉 | `context_injection.rs` |
| Backend-Driven | 后端驱动 | 流式聊天的消息生命周期由后端 SSE 事件驱动，前端不预创建占位消息 | `agent_chat_application_service.rs` |
| 弱校验 | Weak Validation | 不拒绝非法输入，而是安全降级（过滤/默认值），保证流水线不崩溃 | `context_injection.rs` |

---

*交叉引用：详见 [Tarven 规则系统（前端视角）](../vue_docs/features/chat/22_Tarven规则系统.md)*

---
*最后更新：2026-06-04 | VCP Mobile v1.0.3*
