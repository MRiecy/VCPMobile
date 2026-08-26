---
title: 附录C - 数据库Schema对照表
scope: 双端
---

# 附录C - 数据库 Schema 对照表

## 引言

VCPMobile 使用 SQLite + WAL 持久化业务与同步状态。桌面默认由 CDS 的 `chat_data.sqlite3` 提供 Owner、Topic、Message 和 Avatar 的已提交视图；附件元数据保留在 Message JSON 中。Legacy 模式才使用插件 `sync_state_v2.db`。桌面业务正文始终位于物理配置和 `history.json`。

阅读本文档时，建议配合 `02_数据模型与类型系统.md` 理解字段默认值、DTO 映射与哈希计算规则。

---

## 表1：移动端 SQLite Schema（VCPMobile）

移动端数据库文件位于应用配置目录。全新安装由 `0100_baseline_v2.sql` 建立当前 Schema；旧数据库不兼容迁移。

### 1.1 `avatars` — 全局多态头像表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应桌面端 |
|-----|-------|-----|-----|-----|----------|
| avatars | owner_type | TEXT | NOT NULL, PK(1) | 封闭枚举：`agent`、`group`、`user` | `avatar_index.owner_type` |
| avatars | owner_id | TEXT | NOT NULL, PK(2) | Agent/Group UUID；`user` 仅允许 `user_avatar` | `avatar_index.owner_id` |
| avatars | avatar_hash | TEXT | NOT NULL | 头像二进制 SHA-256 摘要，用于 WebSocket 快速 Diff | `avatar_index.hash` |
| avatars | mime_type | TEXT | NOT NULL | 图像 MIME 类型，如 `image/webp`、`image/png` | 由文件扩展名推导 |
| avatars | image_data | BLOB | NOT NULL | 头像物理二进制数据，移动端真理之源 | `UserData/avatars/*` 或同级文件 |
| avatars | dominant_color | TEXT | — | 前端 Canvas 计算主色调（rgb/hex），后端仅存储。commit `df3f219` 将计算从后端 FFmpeg 移至前端 `extractDominantColorFromBlob` | — |
| avatars | updated_at | BIGINT | NOT NULL | 逻辑时钟，毫秒时间戳 | `avatar_index.updated_at` |
| avatars | deleted_at | BIGINT | — | Migration 0006 头像墓碑；非空时读取隐藏且不可被普通 upsert 复活 | `avatar_index.deleted_at` |

> **设计说明**：`avatars` 表采用复合主键 `(owner_type, owner_id)`，实现多态头像的统一存储。头像二进制以 BLOB 形式保存在移动端数据库内部，桌面端则分散存储为独立图像文件。

### 1.2 `agents` — 智能体配置表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应桌面端 |
|-----|-------|-----|-----|-----|----------|
| agents | owner_type | TEXT | NOT NULL, PK(1) | 固定为 `agent` | Legacy/CDS `owners.owner_type` |
| agents | agent_id | TEXT | NOT NULL, PK(2) | Agent ID | Legacy/CDS `owners.owner_id` |
| agents | name | TEXT | NOT NULL | 智能体显示名称 | `config.json` → `name` |
| agents | system_prompt | TEXT | NOT NULL DEFAULT '' | 系统提示词（System Prompt） | `config.json` → `systemPrompt` |
| agents | mobile_system_prompt | TEXT | NOT NULL DEFAULT '' | 移动端专用系统提示词（不同步，仅本机生效） | — |
| agents | model | TEXT | NOT NULL | 模型标识，如 `gemini-2.5-flash` | `config.json` → `model` |
| agents | temperature | REAL | NOT NULL DEFAULT 1 | 采样温度，范围通常为 0.0–2.0 | `config.json` → `temperature` |
| agents | context_token_limit | INTEGER | NOT NULL DEFAULT 0 | 上下文 Token 上限 | `config.json` → `contextTokenLimit` |
| agents | max_output_tokens | INTEGER | NOT NULL DEFAULT 0 | 单次输出 Token 上限 | `config.json` → `maxOutputTokens` |
| agents | stream_output | INTEGER | NOT NULL DEFAULT 1 | 是否启用流式输出（SQLite 无原生 bool，0/1） | `config.json` → `streamOutput` |
| agents | use_temperature | INTEGER | NOT NULL DEFAULT 0 | 是否发送 `temperature` 参数（0/1） | — |
| agents | config_hash | TEXT | NOT NULL DEFAULT '' | V2 配置内容指纹（SHA-256），用于 Diff 阶段 | `owners.config_hash` |
| agents | content_hash | TEXT | NOT NULL DEFAULT '' | Topic 子树聚合指纹 | Desktop 从 live `topics` 动态聚合 |
| agents | updated_at | BIGINT | NOT NULL | 更新时间戳，毫秒 | `owners.updated_at` |
| agents | deleted_at | BIGINT | — | 软删除时间戳，非空即视为已删除 | `owners.deleted_at` |

> **归一化说明**：`config_hash` 与 `content_hash` 的分离是 V2 协议的核心优化。修改系统提示词仅变更 `config_hash`，不会触发旗下所有 Topic 的消息重新比对。

### 1.3 `groups` — 群组配置表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应桌面端 |
|-----|-------|-----|-----|-----|----------|
| groups | owner_type | TEXT | NOT NULL, PK(1) | 固定为 `group` | Legacy/CDS `owners.owner_type` |
| groups | group_id | TEXT | NOT NULL, PK(2) | Group ID | Legacy/CDS `owners.owner_id` |
| groups | name | TEXT | NOT NULL | 群组显示名称 | `config.json` → `name` |
| groups | mode | TEXT | NOT NULL DEFAULT 'sequential' | 发言模式：`sequential`、`naturerandom`、`invite_only` | `config.json` → `mode` |
| groups | group_prompt | TEXT | — | 群组全局提示词 | `config.json` → `groupPrompt` |
| groups | invite_prompt | TEXT | — | 邀请发言提示词模板 | `config.json` → `invitePrompt` |
| groups | use_unified_model | INTEGER | NOT NULL DEFAULT 0 | 是否强制使用统一模型（0/1） | `config.json` → `useUnifiedModel` |
| groups | unified_model | TEXT | — | 统一模型名称 | `config.json` → `unifiedModel` |
| groups | tag_match_mode | TEXT | — | 标签匹配模式：`strict`、`fuzzy` | `config.json` → `tagMatchMode` |
| groups | member_tags | TEXT | NOT NULL DEFAULT '{}' | 完整成员标签 JSON，包含已移除成员的 Tag 记忆 | `config.json` → `memberTags` |
| groups | config_hash | TEXT | NOT NULL DEFAULT '' | V2 配置内容指纹 | `owners.config_hash` |
| groups | content_hash | TEXT | NOT NULL DEFAULT '' | Topic 子树聚合指纹 | Desktop 从 live `topics` 动态聚合 |
| groups | created_at | BIGINT | NOT NULL DEFAULT 0 | 创建时间戳，毫秒 | `config.json` → `createdAt` |
| groups | updated_at | BIGINT | NOT NULL | 更新时间戳，毫秒 | `owners.updated_at` |
| groups | deleted_at | BIGINT | — | 软删除时间戳 | `owners.deleted_at` |

### 1.4 `group_members` — 群组成员关联表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应桌面端 |
|-----|-------|-----|-----|-----|----------|
| group_members | group_id | TEXT | NOT NULL, PK(1) | 所属群组 ID | `config.json` → `members[]` |
| group_members | agent_id | TEXT | NOT NULL, PK(2) | 成员 Agent ID | `config.json` → `members[]` 元素 |
| group_members | sort_order | INTEGER | NOT NULL DEFAULT 0 | 成员在群组内的展示排序 | `members[]` 数组顺序 |
| group_members | updated_at | BIGINT | NOT NULL | 关联更新时间戳 | Owner DTO 变化时推进 `owners.updated_at` |

> `group_members` 只保存当前成员与顺序。完整 `memberTags` 独立保存在 `groups.member_tags`，不会因成员暂时移除而丢失。

### 1.5 `topics` — 话题元数据表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应桌面端 |
|-----|-------|-----|-----|-----|----------|
| topics | owner_type | TEXT | NOT NULL, PK(1) | 所有者类型：`agent` / `group` | 由父级配置类型确定 |
| topics | owner_id | TEXT | NOT NULL, PK(2) | 所有者 ID | 父级 ID |
| topics | topic_id | TEXT | NOT NULL, PK(3) | Topic ID | `config.json` → `topics[].id` |
| topics | title | TEXT | NOT NULL | Topic 显示名称 | `config.json` → `topics[].name` |
| topics | created_at | BIGINT | NOT NULL | 创建时间戳，毫秒 | `config.json` → `topics[].createdAt` |
| topics | updated_at | BIGINT | NOT NULL | 更新时间戳，毫秒 | `topics.updated_at` |
| topics | locked | INTEGER | NOT NULL DEFAULT 1 | 是否锁定（Agent Topic 有效，0/1） | `config.json` → `topics[].locked` |
| topics | unread | INTEGER | NOT NULL DEFAULT 0 | 是否未读（Agent Topic 有效，0/1） | `config.json` → `topics[].unread` |
| topics | unread_count | INTEGER | NOT NULL DEFAULT 0 | 未读消息计数，纯本地统计 | — |
| topics | msg_count | INTEGER | NOT NULL DEFAULT 0 | 消息总数，纯本地统计 | — |
| topics | config_hash | TEXT | NOT NULL DEFAULT '' | 话题元数据指纹（V2） | — |
| topics | content_hash | TEXT | NOT NULL DEFAULT '' | 消息聚合指纹（V2，Messages Merkle Root） | — |
| topics | deleted_at | BIGINT | — | 软删除时间戳 | `topics.deleted_at` |

> **字段名差异**：移动端 `title` 对应桌面端 `config.json` 内的 `topics[].name`，对应 DTO 字段为 `name`。此差异源于历史设计：移动端数据库在 Topic 表中使用 `title`，而桌面端配置文件中 Topic 数组项使用 `name`。

### 1.6 `messages` — 消息历史表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应桌面端 |
|-----|-------|-----|-----|-----|----------|
| messages | owner_type | TEXT | NOT NULL, PK(1) | Owner 类型 | Topic frame `ownerType` |
| messages | owner_id | TEXT | NOT NULL, PK(2) | Owner ID | Topic frame `ownerId` |
| messages | topic_id | TEXT | NOT NULL, PK(3) | Topic ID | Topic frame `topicId` |
| messages | msg_id | TEXT | NOT NULL, PK(4) | 消息 ID | `history.json` → `id` |
| messages | role | TEXT | NOT NULL | 角色：`user`、`assistant`、`system` | `history.json` → `role` |
| messages | name | TEXT | — | 消息发送者显示名称 | `history.json` → `name` |
| messages | agent_id | TEXT | — | 发送者 Agent ID（Agent/Group 消息有效） | `history.json` → `agentId` |
| messages | content | TEXT | NOT NULL | 消息文本内容（Markdown 或纯文本） | `history.json` → `content` |
| messages | timestamp | BIGINT | NOT NULL | 消息时间戳，毫秒 | `history.json` → `timestamp` |
| messages | is_group_message | INTEGER | NOT NULL DEFAULT 0 | 是否为群组消息（0/1） | `history.json` → `isGroupMessage` |
| messages | group_id | TEXT | — | 所属 Group ID（群组消息有效） | `history.json` → `groupId` |
| messages | finish_reason | TEXT | — | 模型结束原因，如 `stop`、`length` | `history.json` → `finishReason` |
| messages | content_hash | TEXT | NOT NULL DEFAULT '' | 消息内容指纹（SHA-256） | Legacy/CDS `messages.message_hash` |
| messages | created_at | BIGINT | NOT NULL | 创建时间戳 | `history.json` → `createdAt` |
| messages | updated_at | BIGINT | NOT NULL | 更新时间戳 | Legacy/CDS `messages.updated_at` |
| messages | deleted_at | BIGINT | — | 软删除时间戳 | Legacy/CDS `messages.deleted_at` |

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

### 1.8 `message_attachments` — 消息-附件关联表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应桌面端 |
|-----|-------|-----|-----|-----|----------|
| message_attachments | owner_type | TEXT | NOT NULL, PK(1) | Owner 类型 | Topic frame `ownerType` |
| message_attachments | owner_id | TEXT | NOT NULL, PK(2) | Owner ID | Topic frame `ownerId` |
| message_attachments | topic_id | TEXT | NOT NULL, PK(3) | Topic ID | Topic frame `topicId` |
| message_attachments | msg_id | TEXT | NOT NULL, PK(4) | 消息 ID | 消息 `id` |
| message_attachments | hash | TEXT | NOT NULL | 附件内容哈希，外键指向 `attachments.hash` | `message_attachments.hash` |
| message_attachments | attachment_order | INTEGER | NOT NULL, PK(5) | 附件在消息内的展示排序 | 附件数组位置 |
| message_attachments | display_name | TEXT | NOT NULL | 原始文件名（保留用户上传时的名称） | `message_attachments.display_name` |
| message_attachments | src | TEXT | — | 来源 URL（网络资源时有效） | — |
| message_attachments | status | TEXT | — | `ready` 或 `desktop_only` | — |
| message_attachments | created_at | BIGINT | NOT NULL | 关联创建时间戳，毫秒 | `message_attachments.created_at` |

> **逻辑引用设计**：`attachments` 表存储物理文件（真理之源），`message_attachments` 表存储逻辑引用上下文。同一附件可被多条消息引用，实现去重与空间节省。

### 1.9 `active_generations` — 活跃生成注册表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应桌面端 |
|-----|-------|-----|-----|-----|----------|
| active_generations | owner_type | TEXT | NOT NULL, PK(1) | `'agent'` / `'group'` | — |
| active_generations | owner_id | TEXT | NOT NULL, PK(2) | Owner ID | — |
| active_generations | topic_id | TEXT | NOT NULL, PK(3) | Topic ID | — |
| active_generations | msg_id | TEXT | NOT NULL, PK(4) | 正在生成的助手消息 ID | — |
| active_generations | created_at | BIGINT | NOT NULL | 注册时间戳，毫秒 | — |

> **设计说明**：作为本地 SSE 代理断点续传的运行时事务日志。生成开始时写入，正常结束/错误/中止时删除。详见 `docs/modules/09_VCP请求客户端.md` §8。

### 1.10 `messages_fts` — 全文搜索虚拟表

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应桌面端 |
|-----|-------|-----|-----|-----|----------|
| messages_fts | msg_id | TEXT | UNINDEXED | 消息 ID（不建立 FTS 索引） | — |
| messages_fts | topic_id | TEXT | UNINDEXED | 话题 ID（不建立 FTS 索引） | — |
| messages_fts | content | TEXT | — | 消息原文 | — |
| messages_fts | owner_type | TEXT | UNINDEXED | Owner 类型过滤列 | — |
| messages_fts | owner_id | TEXT | UNINDEXED | Owner ID 过滤列 | — |

> **设计说明**：FTS5 使用 `tokenize = 'trigram'`，删除触发器按完整消息身份清理索引；写入由消息仓储显式维护。

### 1.11 `tarven_rules` — VCPChatTarven 规则库

| 表名 | 字段名 | 类型 | 约束 | 说明 | 对应桌面端 |
|-----|-------|-----|-----|-----|----------|
| tarven_rules | id | TEXT | PRIMARY KEY | 规则唯一标识 | — |
| tarven_rules | name | TEXT | NOT NULL | 规则名称 | — |
| tarven_rules | rule_type | TEXT | NOT NULL | 规则类型 | — |
| tarven_rules | is_enabled | INTEGER | NOT NULL DEFAULT 1 | 是否启用 | — |
| tarven_rules | content | TEXT | NOT NULL | 规则内容 | — |
| tarven_rules | scope | TEXT | NOT NULL | 作用范围 | — |
| tarven_rules | wrap | INTEGER | NOT NULL DEFAULT 1 | 包装方式 | — |
| tarven_rules | role | TEXT | — | 角色 | — |
| tarven_rules | depth | INTEGER | — | 深度 | — |
| tarven_rules | position | TEXT | — | 位置 | — |
| tarven_rules | sort_order | INTEGER | NOT NULL DEFAULT 0 | 排序 | — |
| tarven_rules | created_at | BIGINT | NOT NULL | 创建时间戳 | — |
| tarven_rules | updated_at | BIGINT | NOT NULL | 更新时间戳 | — |

### 1.12 其他辅助表（不参与同步）

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
| `idx_topics_owner` | topics | `(owner_type, owner_id, created_at DESC)` | 按所有者快速查询 Topic 列表 |
| `idx_messages_topic_time` | messages | `(owner_type, owner_id, topic_id, timestamp DESC, msg_id DESC)` | 按完整 Topic 身份加载稳定消息时间线 |
| `idx_group_members_agent` | group_members | `(agent_id)` | 反向查询 Agent 所属群组 |
| `idx_message_attachments_hash` | message_attachments | `(hash)` | 按哈希查找关联消息 |
| `idx_emoticon_category` | emoticon_library | `(category)` | 表情包分类浏览 |
| `idx_tarven_rules_active` | tarven_rules | `(rule_type, is_enabled, sort_order ASC)` | 按类型与启用状态加载 Tarven 规则 |
| `idx_messages_agent_id` | messages | `(agent_id)` | 全局搜索按具体发言 Agent 过滤 |
| `idx_messages_role` | messages | `(role)` | 全局搜索按消息协议类型过滤 |

---

## 表2：桌面端 Legacy/兼容索引

Legacy 模式使用插件目录下的 `sync_state_v2.db`。中央模式不持久化该文件，只在内存中建立配置/附件兼容视图；正式状态由 CDS `chat_data.sqlite3` 提供。

| 表 | 主键 | 主要字段与职责 |
|---|---|---|
| `owners` | `(owner_type, owner_id)` | `config_path, config_hash, updated_at, deleted_at` |
| `topics` | `(owner_type, owner_id, topic_id)` | `config_hash, content_hash, updated_at, deleted_at`；外键指向 Owner |
| `messages` | `(owner_type, owner_id, topic_id, msg_id)` | `message_hash, updated_at, deleted_at`；外键指向 Topic |
| `history_source_state` | 完整 TopicKey | `file_path, file_size, mtime_ms, index_version`；仅用于物理来源快路径 |
| `avatar_index` | `(owner_id, owner_type)` | `file_path, hash, updated_at, deleted_at`；中央模式只存在于内存 |
| `attachment_index` | `hash` | Desktop 本机附件路径；不代表跨端附件正文 |

CDS 使用对应的 `owners/topics/messages/avatars` 提交表，并额外保存消息元数据、附件关系和 history source 健康状态。两种 Desktop 后端输出相同 Wire 合同，不要求物理列完全相同。

---

## 表3：双端字段映射

下表按同步概念汇总双端字段的对位关系，覆盖 Diff、协商与传输三个阶段中涉及的全部关键字段。

| 概念 | 移动端表/字段 | 桌面端表/字段 | 说明 |
|-----|-------------|-------------|-----|
| Agent ID | `agents.agent_id` | `owners.owner_id`（`owner_type=agent`） | 桌面端 Agent ID 由目录名推导，不在 `config.json` 内存储 |
| Agent 名称 | `agents.name` | `config.json` → `name` | 直接映射，白名单字段 |
| Agent 系统提示词 | `agents.system_prompt` | `config.json` → `systemPrompt` | 移动端 `snake_case`，桌面端 `camelCase` |
| Agent 模型 | `agents.model` | `config.json` → `model` | 直接映射 |
| Agent 温度 | `agents.temperature` | `config.json` → `temperature` | 桌面端 `parseFloat` 归一化 |
| Agent Token 上限 | `agents.context_token_limit` | `config.json` → `contextTokenLimit` | 桌面端 `parseInt` 归一化 |
| Agent 输出上限 | `agents.max_output_tokens` | `config.json` → `maxOutputTokens` | 桌面端 `parseInt` 归一化 |
| Agent 流式开关 | `agents.stream_output` | `config.json` → `streamOutput` | SQLite 以 0/1 存储，Rust DTO 以 bool 序列化 |
| Agent 配置指纹 | `agents.config_hash` | `owners.config_hash` | Diff 阶段直接比对 |
| Agent 聚合指纹 | `agents.content_hash` | CDS `owners.content_hash`；Legacy 从 live `topics` 聚合 | 含下属 Topic 的 keyed Merkle Root |
| Agent 更新时间 | `agents.updated_at` | `owners.updated_at` | LWW 裁决标准 |
| Agent 软删除 | `agents.deleted_at` | `owners.deleted_at` | 非空即视为已删除，同步时双向传播 |
| Group ID | `groups.group_id` | `owners.owner_id`（`owner_type=group`） | 桌面端 `config.json` 内显式存储 `id` |
| Group 名称 | `groups.name` | `config.json` → `name` | 直接映射 |
| Group 成员列表 | `group_members.agent_id` | `config.json` → `members[]` | 移动端反规范化存储，同步时数组↔关联表转换 |
| Group 成员标签 | `groups.member_tags` | `config.json` → `memberTags` | 完整 JSON 对象直接存储 |
| Group 发言模式 | `groups.mode` | `config.json` → `mode` | 直接映射 |
| Group 统一模型开关 | `groups.use_unified_model` | `config.json` → `useUnifiedModel` | 直接映射 |
| Topic ID | `topics.topic_id` | `config.json` → `topics[].id` | 主键 |
| Topic 名称 | `topics.title` | `config.json` → `topics[].name` | **字段名差异**：`title` ↔ `name` |
| Topic 所有者类型 | `topics.owner_type` | 由父级目录推导 | 移动端显式存储，桌面端隐式推导 |
| Topic 所有者 ID | `topics.owner_id` | 父级目录名 | 桌面端 Topic 项不存储 `ownerId`，同步时注入 |
| Topic 锁定状态 | `topics.locked` | `config.json` → `topics[].locked` | 仅 Agent Topic 有效 |
| Topic 未读状态 | `topics.unread` | `config.json` → `topics[].unread` | 仅 Agent Topic 有效 |
| Topic 配置指纹 | `topics.config_hash` | `topics.config_hash` | DTO Hash，Diff 阶段用于 Topic 级增量 |
| Topic 聚合指纹 | `topics.content_hash` | `topics.content_hash` | keyed Message Merkle Root |
| 消息 ID | `messages.msg_id` | `history.json` → `id` | 主键 |
| 消息所属 Topic | `messages.topic_id` | 父级目录 `{topicId}` | 桌面端按目录隔离消息历史 |
| 消息角色 | `messages.role` | `history.json` → `role` | `user` / `assistant` / `system` |
| 消息发送者 | `messages.agent_id` | `history.json` → `agentId` | Agent 回复与 Group 成员回复携带；用户消息可为空 |
| 消息内容 | `messages.content` | `history.json` → `content` | Markdown 或纯文本 |
| 消息时间戳 | `messages.timestamp` | `history.json` → `timestamp` | 毫秒级绝对时间 |
| 消息指纹 | `messages.content_hash` | `messages.message_hash` | 规范消息字段的 SHA-256 |
| 消息软删除 | `messages.deleted_at` | `messages.deleted_at` | 墓碑身份长期保留；Mobile 仅在 30 天后清空正文与渲染缓存 |
| 附件内容哈希 | `attachments.hash` | `attachment_index.hash` | 全局去重键 |
| 附件 MIME 类型 | `attachments.mime_type` | 由文件扩展名推导 | 桌面端不单独存储 |
| 附件物理路径 | `attachments.internal_path` | `attachment_index.file_path` | 移动端在 `app_config_dir` 内；桌面端在 `UserData/attachments/` |
| 附件大小 | `attachments.size` | 文件系统元数据 | 桌面端不索引 |
| 头像所有者类型 | `avatars.owner_type` | `avatar_index.owner_type` | 直接映射 |
| 头像所有者 ID | `avatars.owner_id` | `avatar_index.owner_id` | 直接映射 |
| 头像哈希 | `avatars.avatar_hash` | `avatar_index.hash` | WebSocket Diff 快速比对 |
| 头像二进制 | `avatars.image_data` | `Agents/{id}/avatar.{ext}` 等 | 移动端 BLOB；桌面端独立文件 |
| 头像更新时间 | `avatars.updated_at` | `avatar_index.updated_at` | 直接映射 |
| 消息附件关联 | 完整 Message 键 + `attachment_order` | CDS/Legacy `history.json.attachments[]` | 附件关系随获胜消息 DTO 整体替换 |
| 消息附件文件名 | `message_attachments.display_name` | `history.json.attachments[].name` | 随规范消息元数据传输 |

---

## 附录：同步无关表清单

以下表存在于移动端数据库，但**不参与三阶段同步协议**，仅在本地使用：

| 表名 | 用途 | 说明 |
|-----|------|------|
| `settings` | 全局键值对配置 | 如主题、API 地址等本地偏好 |
| `model_favorites` | 收藏模型列表 | 用户本地标记的常用模型 |
| `model_usage_stats` | 模型使用统计 | 调用次数与最近使用时间 |
| `emoticon_library` | 表情包修复库 | 远程表情包资源的本地缓存索引 |
| `tarven_rules` | VCPChatTarven 规则库 | 本地规则缓存 |
| `active_generations` | 活跃生成注册表 | 断点续传事务日志，不参与标准三阶段同步 |
| `messages_fts` | FTS5 全文搜索虚拟表 | 本地搜索索引，不参与同步 |

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

桌面端同步插件不执行定时墓碑硬删除。移动端生命周期任务每日调用 `DeleteExecutor::cleanup_old_deleted_records`：超过 30 天的已删消息只删除 `render_cache` 并将正文替换为 `[已清空]`，完整消息键和 `deleted_at` 继续保留。Agent/Group/Topic 删除时对 `active_generations` 的清理属于领域级联，不改变墓碑保留规则。

### 布尔值的 SQLite 表达

移动端 Schema 中所有布尔语义字段均使用 `INTEGER` 类型，以 `0`（假）和 `1`（真）表示：
- `agents.stream_output`
- `groups.use_unified_model`
- `topics.locked`
- `topics.unread`
- `render_cache` 仅在 `render_bytes` 非空时写入。
- `messages.is_group_message`

Rust 端通过 `serde` 的自定义序列化将这些字段映射为 `bool`，但在 SQL 层面保持数值型以确保 SQLite 兼容性。

### 主键策略差异

| 端 | 策略 | 示例 |
|---|------|------|
| 移动端 Owner | `(owner_type, owner_id)` | `agents/groups/avatars` |
| 移动端 Topic | `(owner_type, owner_id, topic_id)` | `topics` |
| 移动端 Message | `(owner_type, owner_id, topic_id, msg_id)` | `messages/render_cache/active_generations` |
| 移动端附件关系 | 完整消息键 + `attachment_order` | `message_attachments` |
| Legacy Owner | `(owner_type, owner_id)` | `owners` |
| Legacy Topic | `(owner_type, owner_id, topic_id)` | `topics` |
| Legacy 消息 | `(owner_type, owner_id, topic_id, msg_id)` | `messages` |

---

*最后更新：2026-08-25 | VCP Mobile v1.1.5*
