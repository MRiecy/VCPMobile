---
title: 附录C - 数据库Schema对照表
scope: 双端
version: 0.9.13
last_updated: 2026-05-13
---

# 附录C - 数据库 Schema 对照表

## 引言

本文档以表格形式精确列出 VCPMobile（移动端）与 VCPMobileSync（桌面端插件）在同步场景下涉及的全部数据库表结构。移动端使用原生 SQLite（WAL 模式）持久化实体全量数据；桌面端插件使用 `better-sqlite3` 维护轻量级索引库，用于快速 Diff 与哈希比对，实体正文仍存储于桌面端原有 JSON 文件系统中。

阅读本文档时，建议配合 `02_数据模型与类型系统.md` 理解字段默认值、DTO 映射与哈希计算规则。

---

## 表1：移动端 SQLite Schema（VCPMobile）

移动端数据库文件位于应用配置目录（Android 下为 `/data/user/0/com.vcp.avatar/files/vcp_avatar.db`）。所有同步相关表均定义于 `db_manager.rs` 的 `setup_tables` 函数中。

### 1.1 `avatars` — 全局多态头像表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应桌面端 |
|-----|-------|-----|-----|-----|----------|
| avatars | owner_type | TEXT | NOT NULL, PK(1) | 实体类型：`agent`、`group`、`user`、`system` | `avatar_index.owner_type` |
| avatars | owner_id | TEXT | NOT NULL, PK(2) | 对应实体 UUID 或固定值 `user_avatar` | `avatar_index.owner_id` |
| avatars | avatar_hash | TEXT | NOT NULL | 头像二进制 SHA-256 摘要，用于 WebSocket 快速 Diff | `avatar_index.hash` |
| avatars | mime_type | TEXT | NOT NULL | 图像 MIME 类型，如 `image/webp`、`image/png` | 由文件扩展名推导 |
| avatars | image_data | BLOB | NOT NULL | 头像物理二进制数据，移动端真理之源 | `UserData/avatars/*` 或同级文件 |
| avatars | dominant_color | TEXT | — | 前端 Canvas 计算主色调（rgb/hex），后端仅存储。commit `df3f219` 将计算从后端 FFmpeg 移至前端 `extractDominantColorFromBlob` | — |
| avatars | updated_at | BIGINT | NOT NULL | 逻辑时钟，毫秒时间戳 | `avatar_index.updated_at` |

> **设计说明**：`avatars` 表采用复合主键 `(owner_type, owner_id)`，实现多态头像的统一存储。头像二进制以 BLOB 形式保存在移动端数据库内部，桌面端则分散存储为独立图像文件。

### 1.2 `agents` — 智能体配置表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应桌面端 |
|-----|-------|-----|-----|-----|----------|
| agents | agent_id | TEXT | PRIMARY KEY | Agent 唯一标识（UUID 格式） | `entity_index.id`（type=`agent`） |
| agents | name | TEXT | NOT NULL | 智能体显示名称 | `config.json` → `name` |
| agents | system_prompt | TEXT | NOT NULL DEFAULT '' | 系统提示词（System Prompt） | `config.json` → `systemPrompt` |
| agents | model | TEXT | NOT NULL | 模型标识，如 `gemini-2.5-flash` | `config.json` → `model` |
| agents | temperature | REAL | NOT NULL DEFAULT 1 | 采样温度，范围通常为 0.0–2.0 | `config.json` → `temperature` |
| agents | context_token_limit | INTEGER | NOT NULL DEFAULT 0 | 上下文 Token 上限 | `config.json` → `contextTokenLimit` |
| agents | max_output_tokens | INTEGER | NOT NULL DEFAULT 0 | 单次输出 Token 上限 | `config.json` → `maxOutputTokens` |
| agents | stream_output | INTEGER | NOT NULL DEFAULT 1 | 是否启用流式输出（SQLite 无原生 bool，0/1） | `config.json` → `streamOutput` |
| agents | config_hash | TEXT | NOT NULL DEFAULT '' | V2 配置内容指纹（SHA-256），用于 Diff 阶段 | `entity_index.hash` |
| agents | content_hash | TEXT | NOT NULL DEFAULT '' | V2 聚合指纹（Config + Topics Merkle Root） | `entity_index.aggregated_hash` |
| agents | current_topic_id | TEXT | — | 最后活跃话题 ID，纯本地 UI 状态 | — |
| agents | updated_at | BIGINT | NOT NULL | 更新时间戳，毫秒 | `entity_index.updated_at` |
| agents | deleted_at | BIGINT | — | 软删除时间戳，非空即视为已删除 | `entity_index.deleted_at` |

> **归一化说明**：`config_hash` 与 `content_hash` 的分离是 V2 协议的核心优化。修改系统提示词仅变更 `config_hash`，不会触发旗下所有 Topic 的消息重新比对。

### 1.3 `groups` — 群组配置表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应桌面端 |
|-----|-------|-----|-----|-----|----------|
| groups | group_id | TEXT | PRIMARY KEY | Group 唯一标识（通常为 `____123` 格式） | `entity_index.id`（type=`group`） |
| groups | name | TEXT | NOT NULL | 群组显示名称 | `config.json` → `name` |
| groups | mode | TEXT | NOT NULL DEFAULT 'sequential' | 发言模式：`sequential`、`naturerandom`、`invite_only` | `config.json` → `mode` |
| groups | group_prompt | TEXT | — | 群组全局提示词 | `config.json` → `groupPrompt` |
| groups | invite_prompt | TEXT | — | 邀请发言提示词模板 | `config.json` → `invitePrompt` |
| groups | use_unified_model | INTEGER | NOT NULL DEFAULT 0 | 是否强制使用统一模型（0/1） | `config.json` → `useUnifiedModel` |
| groups | unified_model | TEXT | — | 统一模型名称 | `config.json` → `unifiedModel` |
| groups | tag_match_mode | TEXT | — | 标签匹配模式：`strict`、`fuzzy` | `config.json` → `tagMatchMode` |
| groups | config_hash | TEXT | NOT NULL DEFAULT '' | V2 配置内容指纹 | `entity_index.hash` |
| groups | content_hash | TEXT | NOT NULL DEFAULT '' | V2 聚合指纹（Config + Topics） | `entity_index.aggregated_hash` |
| groups | current_topic_id | TEXT | — | 最后活跃话题 ID，纯本地 UI 状态 | — |
| groups | created_at | BIGINT | NOT NULL DEFAULT 0 | 创建时间戳，毫秒 | `config.json` → `createdAt` |
| groups | updated_at | BIGINT | NOT NULL | 更新时间戳，毫秒 | `entity_index.updated_at` |
| groups | deleted_at | BIGINT | — | 软删除时间戳 | `entity_index.deleted_at` |

### 1.4 `group_members` — 群组成员关联表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应桌面端 |
|-----|-------|-----|-----|-----|----------|
| group_members | group_id | TEXT | NOT NULL, PK(1) | 所属群组 ID | `config.json` → `members[]` |
| group_members | agent_id | TEXT | NOT NULL, PK(2) | 成员 Agent ID | `config.json` → `members[]` 元素 |
| group_members | member_tag | TEXT | — | 成员标签，用于路由与过滤 | `config.json` → `memberTags[agentId]` |
| group_members | sort_order | INTEGER | NOT NULL DEFAULT 0 | 成员在群组内的展示排序 | `members[]` 数组顺序 |
| group_members | updated_at | BIGINT | NOT NULL | 关联更新时间戳 | `entity_index.updated_at`（父级 Group） |

> **归一化策略**：桌面端 `GroupConfig.members` 为扁平字符串数组；移动端拆分为独立关联表，支持 `member_tag` 与 `sort_order` 等扩展元数据。同步时由 `GroupSyncDTO.members` 双向反规范化转换。

### 1.5 `topics` — 话题元数据表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应桌面端 |
|-----|-------|-----|-----|-----|----------|
| topics | topic_id | TEXT | PRIMARY KEY | Topic 唯一标识 | `config.json` → `topics[].id` |
| topics | owner_type | TEXT | NOT NULL | 所有者类型：`agent` / `group` | 由父级目录类型推导 |
| topics | owner_id | TEXT | NOT NULL | 所有者 ID（Agent 或 Group） | 父级目录名 |
| topics | title | TEXT | NOT NULL | Topic 显示名称 | `config.json` → `topics[].name` |
| topics | created_at | BIGINT | NOT NULL | 创建时间戳，毫秒 | `config.json` → `topics[].createdAt` |
| topics | updated_at | BIGINT | NOT NULL | 更新时间戳，毫秒 | `entity_index.updated_at` |
| topics | locked | INTEGER | NOT NULL DEFAULT 1 | 是否锁定（Agent Topic 有效，0/1） | `config.json` → `topics[].locked` |
| topics | unread | INTEGER | NOT NULL DEFAULT 0 | 是否未读（Agent Topic 有效，0/1） | `config.json` → `topics[].unread` |
| topics | unread_count | INTEGER | NOT NULL DEFAULT 0 | 未读消息计数，纯本地统计 | — |
| topics | msg_count | INTEGER | NOT NULL DEFAULT 0 | 消息总数，纯本地统计 | — |
| topics | config_hash | TEXT | NOT NULL DEFAULT '' | 话题元数据指纹（V2） | — |
| topics | content_hash | TEXT | NOT NULL DEFAULT '' | 消息聚合指纹（V2，Messages Merkle Root） | — |
| topics | deleted_at | BIGINT | — | 软删除时间戳 | `entity_index.deleted_at` |

> **字段名差异**：移动端 `title` 对应桌面端 `config.json` 内的 `topics[].name`，对应 DTO 字段为 `name`。此差异源于历史设计：移动端数据库在 Topic 表中使用 `title`，而桌面端配置文件中 Topic 数组项使用 `name`。

### 1.6 `messages` — 消息历史表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应桌面端 |
|-----|-------|-----|-----|-----|----------|
| messages | msg_id | TEXT | PRIMARY KEY | 消息唯一标识 | `history.json` 数组项 → `id` |
| messages | topic_id | TEXT | NOT NULL | 所属 Topic ID | 父级目录 `{topicId}` |
| messages | role | TEXT | NOT NULL | 角色：`user`、`assistant`、`system` | `history.json` → `role` |
| messages | name | TEXT | — | 消息发送者显示名称 | `history.json` → `name` |
| messages | agent_id | TEXT | — | 发送者 Agent ID（Agent/Group 消息有效） | `history.json` → `agentId` |
| messages | content | TEXT | NOT NULL | 消息文本内容（Markdown 或纯文本） | `history.json` → `content` |
| messages | timestamp | BIGINT | NOT NULL | 消息时间戳，毫秒 | `history.json` → `timestamp` |
| messages | is_thinking | INTEGER | NOT NULL DEFAULT 0 | 是否为思考中状态（兼容占位，0/1） | `history.json` → `isThinking` |
| messages | is_group_message | INTEGER | NOT NULL DEFAULT 0 | 是否为群组消息（0/1） | `history.json` → `isGroupMessage` |
| messages | group_id | TEXT | — | 所属 Group ID（群组消息有效） | `history.json` → `groupId` |
| messages | finish_reason | TEXT | — | 模型结束原因，如 `stop`、`length` | `history.json` → `finishReason` |
| messages | render_content | BLOB | — | 预渲染结构化内容（如工具调用结果） | `history.json` → `renderContent` |
| messages | content_hash | TEXT | NOT NULL DEFAULT '' | 消息内容指纹（SHA-256） | `message_index.hash` |
| messages | created_at | BIGINT | NOT NULL | 创建时间戳 | `history.json` → `createdAt` |
| messages | updated_at | BIGINT | NOT NULL | 更新时间戳 | `message_index.updated_at` |
| messages | deleted_at | BIGINT | — | 软删除时间戳 | `message_index.deleted_at` |

> **已移除字段**：历史版本中 `messages` 表曾包含 `avatar_url` 与 `avatar_color`，现已被移除。头像信息通过 `avatars` 表按 `agent_id` 动态查询，避免数据冗余。

### 1.7 `attachments` — 附件物理存储表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应桌面端 |
|-----|-------|-----|-----|-----|----------|
| attachments | hash | TEXT | PRIMARY KEY | 内容 SHA-256 摘要，全局去重键 | `attachment_index.hash` |
| attachments | mime_type | TEXT | NOT NULL | MIME 类型，如 `image/webp`、`application/pdf` | 由文件头推导 |
| attachments | size | BIGINT | NOT NULL | 文件大小（字节） | 文件系统元数据 |
| attachments | internal_path | TEXT | NOT NULL | 本地物理存储绝对路径 | `attachment_index.file_path` |
| attachments | extracted_text | TEXT | — | OCR 或文本提取结果，用于搜索 | — |
| attachments | image_frames | TEXT | — | 视频帧或 PDF 图片路径（JSON 数组序列化） | — |
| attachments | thumbnail_path | TEXT | — | 缩略图本地路径 | — |
| attachments | created_at | BIGINT | NOT NULL | 创建时间戳，毫秒 | `attachment_index.updated_at` |
| attachments | updated_at | BIGINT | NOT NULL | 更新时间戳，毫秒 | `attachment_index.updated_at` |

> **容量限制**：移动端附件上传限制 20 MB；`read_local_file_base64` 限制 50 MB，防止 OOM。

### 1.8 `message_attachments` — 消息-附件关联表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应桌面端 |
|-----|-------|-----|-----|-----|----------|
| message_attachments | msg_id | TEXT | NOT NULL, PK(1) | 消息 ID | `message_attachments.msg_id` |
| message_attachments | hash | TEXT | NOT NULL | 附件内容哈希，外键指向 `attachments.hash` | `message_attachments.hash` |
| message_attachments | attachment_order | INTEGER | NOT NULL, PK(2) | 附件在消息内的展示排序 | `message_attachments.attachment_order` |
| message_attachments | display_name | TEXT | NOT NULL | 原始文件名（保留用户上传时的名称） | `message_attachments.display_name` |
| message_attachments | src | TEXT | — | 来源 URL（网络资源时有效） | — |
| message_attachments | status | TEXT | — | 附件状态，如 `removed` | — |
| message_attachments | created_at | BIGINT | NOT NULL | 关联创建时间戳，毫秒 | `message_attachments.created_at` |

> **逻辑引用设计**：`attachments` 表存储物理文件（真理之源），`message_attachments` 表存储逻辑引用上下文。同一附件可被多条消息引用，实现去重与空间节省。

### 1.9 其他辅助表（不参与同步）

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应桌面端 |
|-----|-------|-----|-----|-----|----------|
| settings | key | TEXT | PRIMARY KEY | 配置键 | — |
| settings | value | TEXT | NOT NULL | 配置值（JSON 字符串） | — |
| settings | updated_at | BIGINT | NOT NULL | 更新时间戳 | — |
| model_favorites | model_id | TEXT | PRIMARY KEY | 收藏模型标识 | — |
| model_favorites | created_at | BIGINT | NOT NULL | 收藏时间戳 | — |
| model_usage_stats | model_id | TEXT | PRIMARY KEY | 模型标识 | — |
| model_usage_stats | usage_count | INTEGER | NOT NULL DEFAULT 0 | 使用次数 | — |
| model_usage_stats | updated_at | BIGINT | NOT NULL | 统计更新时间 | — |
| emoticon_library | id | INTEGER | PRIMARY KEY AUTOINCREMENT | 自增主键 | — |
| emoticon_library | category | TEXT | NOT NULL | 表情包分类 | — |
| emoticon_library | filename | TEXT | NOT NULL | 文件名 | — |
| emoticon_library | url | TEXT | NOT NULL, UNIQUE | 资源 URL | — |
| emoticon_library | search_key | TEXT | NOT NULL | 搜索关键词 | — |

### 1.10 移动端索引汇总

| 索引名 | 所在表 | 字段 | 用途 |
|-------|-------|------|------|
| `idx_topics_owner` | topics | `(owner_id, owner_type, created_at DESC)` | 按所有者快速查询 Topic 列表 |
| `idx_messages_topic_time` | messages | `(topic_id, timestamp DESC)` | 按 Topic 加载消息历史（时间倒序） |
| `idx_messages_updated_at` | messages | `(updated_at)` | 同步阶段快速筛选增量消息 |
| `idx_group_members_agent` | group_members | `(agent_id)` | 反向查询 Agent 所属群组 |
| `idx_message_attachments_hash` | message_attachments | `(hash)` | 按哈希查找关联消息 |
| `idx_emoticon_category` | emoticon_library | `(category)` | 表情包分类浏览 |

---

## 表2：桌面端 SQLite Schema（VCPMobileSync 索引库）

桌面端插件在启动时扫描桌面端原有文件系统，并构建独立的 SQLite 索引库（通常位于 `AppData/VCPMobileSync/sync_index.db`）。该库**不存储实体正文**，仅存储文件路径、内容哈希与更新时钟，用于三阶段同步协议中的 Diff 计算。

### 2.1 `entity_index` — 实体索引表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应移动端 |
|-----|-------|-----|-----|-----|----------|
| entity_index | id | TEXT | NOT NULL, PK(1) | 实体唯一标识 | `agents.agent_id` / `groups.group_id` / `topics.topic_id` |
| entity_index | type | TEXT | NOT NULL, PK(2) | 实体类型：`agent`、`group`、`agent_topic`、`group_topic` | 由 `owner_type` 推导 |
| entity_index | file_path | TEXT | NOT NULL | 实体配置文件绝对路径 | — |
| entity_index | hash | TEXT | NOT NULL | 内容指纹（DTO 稳定 JSON 的 SHA-256） | `agents.config_hash` / `groups.config_hash` / `topics.config_hash` |
| entity_index | aggregated_hash | TEXT | — | 聚合指纹（含下属 Topic/Message 的 Merkle Root） | `agents.content_hash` / `groups.content_hash` / `topics.content_hash` |
| entity_index | updated_at | INTEGER | NOT NULL | 更新时间戳，毫秒 | `agents.updated_at` / `groups.updated_at` / `topics.updated_at` |
| entity_index | deleted_at | INTEGER | DEFAULT NULL | 软删除时间戳，非空即已删除 | `agents.deleted_at` / `groups.deleted_at` / `topics.deleted_at` |

> **类型扩展**：桌面端 `entity_index` 将 `topic` 细分为 `agent_topic` 与 `group_topic`，以便在 Diff 阶段精确路由到对应的 DTO 提取器。

### 2.2 `message_index` — 消息索引表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应移动端 |
|-----|-------|-----|-----|-----|----------|
| message_index | msg_id | TEXT | NOT NULL, PRIMARY KEY | 消息唯一标识 | `messages.msg_id` |
| message_index | topic_id | TEXT | NOT NULL | 所属 Topic ID | `messages.topic_id` |
| message_index | hash | TEXT | NOT NULL | 消息内容指纹 | `messages.content_hash` |
| message_index | updated_at | INTEGER | NOT NULL | 更新时间戳，毫秒 | `messages.updated_at` |
| message_index | deleted_at | INTEGER | DEFAULT NULL | 软删除时间戳 | `messages.deleted_at` |

> **索引优化**：额外建立 `idx_msg_topic ON message_index(topic_id)`，用于快速按 Topic 聚合消息哈希以计算 Merkle Root。

### 2.3 `attachment_index` — 附件索引表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应移动端 |
|-----|-------|-----|-----|-----|----------|
| attachment_index | hash | TEXT | PRIMARY KEY | 内容 SHA-256 摘要 | `attachments.hash` |
| attachment_index | file_path | TEXT | NOT NULL | 附件物理文件绝对路径 | `attachments.internal_path` |
| attachment_index | updated_at | INTEGER | NOT NULL | 更新时间戳，毫秒 | `attachments.updated_at` |
| attachment_index | deleted_at | INTEGER | DEFAULT NULL | 软删除时间戳 | — |

> **路径约定**：桌面端附件存储于 `UserData/attachments/{hash}.{ext}`，索引库仅记录该路径，文件正文不在 SQLite 中。

### 2.4 `avatar_index` — 头像索引表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应移动端 |
|-----|-------|-----|-----|-----|----------|
| avatar_index | owner_id | TEXT | NOT NULL, PK(1) | 头像所有者 ID | `avatars.owner_id` |
| avatar_index | owner_type | TEXT | NOT NULL, PK(2) | 头像所有者类型 | `avatars.owner_type` |
| avatar_index | file_path | TEXT | NOT NULL | 头像文件绝对路径 | `avatars.image_data`（BLOB 转文件） |
| avatar_index | hash | TEXT | NOT NULL | 头像二进制 SHA-256 | `avatars.avatar_hash` |
| avatar_index | updated_at | INTEGER | NOT NULL | 更新时间戳，毫秒 | `avatars.updated_at` |
| avatar_index | deleted_at | INTEGER | DEFAULT NULL | 软删除时间戳 | — |

> **路径约定**：Agent 头像位于 `Agents/{id}/avatar.{ext}`；Group 头像位于 `AgentGroups/{id}/avatar.{ext}`；用户头像位于 `UserData/user_avatar.png`。

### 2.5 `message_attachments` — 消息附件关联表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应移动端 |
|-----|-------|-----|-----|-----|----------|
| message_attachments | msg_id | TEXT | NOT NULL, PK(1) | 消息 ID | `message_attachments.msg_id` |
| message_attachments | hash | TEXT | NOT NULL | 附件内容哈希 | `message_attachments.hash` |
| message_attachments | attachment_order | INTEGER | NOT NULL, PK(2) | 附件排序 | `message_attachments.attachment_order` |
| message_attachments | display_name | TEXT | NOT NULL | 原始文件名 | `message_attachments.display_name` |
| message_attachments | created_at | INTEGER | NOT NULL | 关联创建时间戳 | `message_attachments.created_at` |

> **字段差异**：桌面端 `message_attachments` 不含 `src` 与 `status` 字段，因为桌面端附件引用机制不依赖外部 URL，且删除策略通过 `attachment_index.deleted_at` 软删除实现。

---

## 表3：双端字段映射

下表按同步概念汇总双端字段的对位关系，覆盖 Diff、协商与传输三个阶段中涉及的全部关键字段。

| 概念 | 移动端表/字段 | 桌面端表/字段 | 说明 |
|-----|-------------|-------------|-----|
| Agent ID | `agents.agent_id` | `entity_index.id`（type=`agent`） | 桌面端 Agent ID 由目录名推导，不在 `config.json` 内存储 |
| Agent 名称 | `agents.name` | `config.json` → `name` | 直接映射，白名单字段 |
| Agent 系统提示词 | `agents.system_prompt` | `config.json` → `systemPrompt` | 移动端 `snake_case`，桌面端 `camelCase` |
| Agent 模型 | `agents.model` | `config.json` → `model` | 直接映射 |
| Agent 温度 | `agents.temperature` | `config.json` → `temperature` | 桌面端 `parseFloat` 归一化 |
| Agent Token 上限 | `agents.context_token_limit` | `config.json` → `contextTokenLimit` | 桌面端 `parseInt` 归一化 |
| Agent 输出上限 | `agents.max_output_tokens` | `config.json` → `maxOutputTokens` | 桌面端 `parseInt` 归一化 |
| Agent 流式开关 | `agents.stream_output` | `config.json` → `streamOutput` | SQLite 以 0/1 存储，Rust DTO 以 bool 序列化 |
| Agent 配置指纹 | `agents.config_hash` | `entity_index.hash` | V2 协议核心，Diff 阶段直接比对 |
| Agent 聚合指纹 | `agents.content_hash` | `entity_index.aggregated_hash` | 含下属 Topic 的 Merkle Root |
| Agent 更新时间 | `agents.updated_at` | `entity_index.updated_at` | LWW 裁决标准 |
| Agent 软删除 | `agents.deleted_at` | `entity_index.deleted_at` | 非空即视为已删除，同步时双向传播 |
| Group ID | `groups.group_id` | `entity_index.id`（type=`group`） | 桌面端 `config.json` 内显式存储 `id` |
| Group 名称 | `groups.name` | `config.json` → `name` | 直接映射 |
| Group 成员列表 | `group_members.agent_id` | `config.json` → `members[]` | 移动端反规范化存储，同步时数组↔关联表转换 |
| Group 成员标签 | `group_members.member_tag` | `config.json` → `memberTags[agentId]` | JSON 对象映射到关联表行 |
| Group 发言模式 | `groups.mode` | `config.json` → `mode` | 直接映射 |
| Group 统一模型开关 | `groups.use_unified_model` | `config.json` → `useUnifiedModel` | 直接映射 |
| Topic ID | `topics.topic_id` | `config.json` → `topics[].id` | 主键 |
| Topic 名称 | `topics.title` | `config.json` → `topics[].name` | **字段名差异**：`title` ↔ `name` |
| Topic 所有者类型 | `topics.owner_type` | 由父级目录推导 | 移动端显式存储，桌面端隐式推导 |
| Topic 所有者 ID | `topics.owner_id` | 父级目录名 | 桌面端 Topic 项不存储 `ownerId`，同步时注入 |
| Topic 锁定状态 | `topics.locked` | `config.json` → `topics[].locked` | 仅 Agent Topic 有效 |
| Topic 未读状态 | `topics.unread` | `config.json` → `topics[].unread` | 仅 Agent Topic 有效 |
| Topic 配置指纹 | `topics.config_hash` | — | 移动端本地计算，Diff 阶段用于 Topic 级增量 |
| Topic 聚合指纹 | `topics.content_hash` | — | 消息 Merkle Root，用于避免全量消息比对 |
| 消息 ID | `messages.msg_id` | `history.json` → `id` | 主键 |
| 消息所属 Topic | `messages.topic_id` | 父级目录 `{topicId}` | 桌面端按目录隔离消息历史 |
| 消息角色 | `messages.role` | `history.json` → `role` | `user` / `assistant` / `system` |
| 消息发送者 | `messages.agent_id` | `history.json` → `agentId` | Agent/Group 消息必填 |
| 消息内容 | `messages.content` | `history.json` → `content` | Markdown 或纯文本 |
| 消息时间戳 | `messages.timestamp` | `history.json` → `timestamp` | 毫秒级绝对时间 |
| 消息指纹 | `messages.content_hash` | `message_index.hash` | 稳定 JSON 的 SHA-256 |
| 消息软删除 | `messages.deleted_at` | `message_index.deleted_at` | 30 天后清理 |
| 附件内容哈希 | `attachments.hash` | `attachment_index.hash` | 全局去重键 |
| 附件 MIME 类型 | `attachments.mime_type` | 由文件扩展名推导 | 桌面端不单独存储 |
| 附件物理路径 | `attachments.internal_path` | `attachment_index.file_path` | 移动端在 `app_config_dir` 内；桌面端在 `UserData/attachments/` |
| 附件大小 | `attachments.size` | 文件系统元数据 | 桌面端不索引 |
| 头像所有者类型 | `avatars.owner_type` | `avatar_index.owner_type` | 直接映射 |
| 头像所有者 ID | `avatars.owner_id` | `avatar_index.owner_id` | 直接映射 |
| 头像哈希 | `avatars.avatar_hash` | `avatar_index.hash` | WebSocket Diff 快速比对 |
| 头像二进制 | `avatars.image_data` | `Agents/{id}/avatar.{ext}` 等 | 移动端 BLOB；桌面端独立文件 |
| 头像更新时间 | `avatars.updated_at` | `avatar_index.updated_at` | 直接映射 |
| 消息附件关联 | `message_attachments.(msg_id, hash, order)` | `message_attachments.(msg_id, hash, order)` | 双端表结构基本一致，仅桌面端缺 `src`、`status` |
| 消息附件文件名 | `message_attachments.display_name` | `message_attachments.display_name` | 直接映射 |

---

## 附录：同步无关表清单

以下表存在于移动端数据库，但**不参与三阶段同步协议**，仅在本地使用：

| 表名 | 用途 | 说明 |
|-----|------|------|
| `settings` | 全局键值对配置 | 如主题、API 地址等本地偏好 |
| `model_favorites` | 收藏模型列表 | 用户本地标记的常用模型 |
| `model_usage_stats` | 模型使用统计 | 调用次数与最近使用时间 |
| `emoticon_library` | 表情包修复库 | 远程表情包资源的本地缓存索引 |

桌面端插件索引库**不包含**桌面端主程序的 `forum.config.json`、`emoticon_library.json`、`settings.json` 等系统文件，这些文件由桌面端原有逻辑独立维护。

---

*本文档由 `db_manager.rs`、`VCPMobileSync/core/db.js` 及 `02_数据模型与类型系统.md` 同源生成。如修改 Schema，请同步更新本文件。*


## 补充说明

### WAL 模式与并发控制

移动端数据库启用 WAL（Write-Ahead Logging）模式，配合 30 秒 `busy_timeout`，在高并发场景（如消息流式写入与同步任务并行）下显著降低锁竞争。桌面端插件使用 `better-sqlite3` 的同步 API，但由于其运行在独立的 Node.js 进程（插件宿主）中，与桌面端主程序的文件系统访问天然隔离，因此无需 WAL 即可保证一致性。

### 软删除与垃圾回收

双端均采用**软删除**策略：
- 移动端：`deleted_at` 字段由 `BIGINT` 标记，非空即视为已删除。
- 桌面端：`deleted_at` 字段由 `INTEGER DEFAULT NULL` 标记。

桌面端插件提供 `cleanupOldDeletedRecords()` 函数，自动清理 `deleted_at` 超过 30 天的记录。移动端目前依赖数据库清理策略，由 `db_write_queue.rs` 批量写入时自动过滤已删除实体。

### 布尔值的 SQLite 表达

移动端 Schema 中所有布尔语义字段均使用 `INTEGER` 类型，以 `0`（假）和 `1`（真）表示：
- `agents.stream_output`
- `groups.use_unified_model`
- `topics.locked`
- `topics.unread`
- `messages.is_thinking`
- `messages.is_group_message`

Rust 端通过 `serde` 的自定义序列化将这些字段映射为 `bool`，但在 SQL 层面保持数值型以确保 SQLite 兼容性。

### 主键策略差异

| 端 | 策略 | 示例 |
|---|------|------|
| 移动端 | 单字段 TEXT 主键（UUID） | `agents.agent_id` |
| 移动端 | 复合主键（TEXT + TEXT） | `avatars.(owner_type, owner_id)` |
| 移动端 | 复合主键（TEXT + INTEGER） | `message_attachments.(msg_id, attachment_order)` |
| 桌面端 | 复合主键（TEXT + TEXT） | `entity_index.(id, type)` |
| 桌面端 | 单字段 TEXT 主键 | `message_index.msg_id` |

移动端倾向于使用自然语义复合主键（如多态头像的所有者类型+ID），桌面端插件索引库在 `entity_index` 中采用 `(id, type)` 复合主键以区分同名 ID 的不同实体类型（尽管实际场景中 UUID 已足够唯一）。
