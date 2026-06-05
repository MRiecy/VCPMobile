---
id: VUE-CHAT-022
title: Tarven规则系统
description: VCP Mobile 前端 Tarven 注入规则的状态管理、选择器 UI 与规则编辑面板
version: 1.0.3
date: 2026-06-05
---

# 22. Tarven 规则系统

## 1. 概述

### 1.1 领域定位

Tarven 规则系统是 VCP Mobile 的**上下文注入引擎**的前端控制面，负责让用户以声明式方式定义"在何时、何地、以何种方式向 LLM 请求中插入额外内容"。其设计灵感源自 SillyTavern 的 Lorebook / World Info 系统，但针对移动端进行了大幅简化与原生交互适配。

该系统的核心职责包括：

- **规则生命周期管理**：CRUD 操作与启用状态切换
- **注入效果可视化**：通过 WYSIWYG 预览让用户在保存前即看到规则对上下文的实际影响
- **作用域隔离**：支持全局 / 单聊 / 群聊三种生效范围
- **与输入栏无缝集成**：通过长按附件按钮快速呼出规则选择器
- **排序与优先级控制**：同类型规则按 `sortOrder` 升序执行，确保注入顺序可预测

该系统**不涉及**：
- 注入逻辑本身（由 Rust 后端 `context_injection.rs` 负责实际拼装）
- 模型推理（由 `vcp_client.rs` 负责）
- 消息持久化（由 `message_service.rs` 负责）
- 话题管理（由 `topic_service.rs` 负责）

### 1.2 模块构成表

| 文件 | 行数 | 职责 |
|------|------|------|
| `src/core/stores/tarvenStore.ts` | 109 | Pinia 状态层：规则列表、CRUD、排序、预览调用 |
| `src/features/chat/components/TarvenSelector.vue` | 191 | 底部抽屉选择器：快速开关规则、跳转设置 |
| `src/features/chat/components/TarvenSettings.vue` | 747 | SlidePage 规则管理页：列表展示、表单编辑、实时预览 |
| `src/features/chat/ChatView.vue` | 316 | 挂载 TarvenSelector，作为规则 UI 的容器 |
| `src/features/chat/InputEnhancer.vue` | 657 | 输入栏增强：长按附件按钮触发选择器，绿色指示点 |
| `src-tauri/src/vcp_modules/chat/context_injection.rs` | 574 | Rust 注入引擎：规则查询、流水线注入、Tauri 命令 |

### 1.3 在整体架构中的位置

```
┌─────────────────────────────────────────────────────────────┐
│                      Vue 3 前端层                            │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │TarvenSelector│  │TarvenSettings│  │   InputEnhancer     │  │
│  │ (快速开关)   │  │ (编辑面板)   │  │  (触发入口+指示点)   │  │
│  └──────┬──────┘  └──────┬──────┘  └──────────┬──────────┘  │
│         │                │                     │             │
│         └────────────────┼─────────────────────┘             │
│                          │                                   │
│                   ┌──────┴──────┐                            │
│                   ▼             ▼                            │
│         ┌───────────────┐ ┌───────────────┐                 │
│         │  tarvenStore  │ │  overlayStore │                 │
│         │  (Pinia 状态)  │ │ (SlidePage 栈)│                 │
│         └───────┬───────┘ └───────────────┘                 │
└─────────────────┼───────────────────────────────────────────┘
                  │ IPC (Tauri Commands)
┌─────────────────▼───────────────────────────────────────────┐
│                   src-tauri (Rust 核心层)                    │
│                                                              │
│  ┌───────────────────────────────────────────────────────┐  │
│  │           chat/context_injection.rs                   │  │
│  │  ┌───────────────┐  ┌─────────────────────────────┐  │  │
│  │  │Tauri Commands │  │ apply_tarven_pipeline()     │  │  │
│  │  │ (6 个命令)    │  │ 系统注入 / 用户后缀 / 上下文 │  │  │
│  │  └───────────────┘  │ 节点插入                     │  │  │
│  │                     └─────────────────────────────┘  │  │
│  └───────────────────────────────────────────────────────┘  │
│                           │                                  │
│                           ▼                                  │
│                    ┌─────────────┐                           │
│                    │ db_manager  │                           │
│                    │ tarven_rules│                           │
│                    └─────────────┘                           │
└─────────────────────────────────────────────────────────────┘
```

### 1.4 核心设计原则

| 原则 | 说明 |
|------|------|
| **声明式注入** | 用户只需声明"注入什么、注入到哪里"，无需关心底层消息拼装细节 |
| **即时生效** | 规则启用/禁用切换后，下一条消息立即生效，无需重启应用或重新加载话题 |
| **所见即所得** | 编辑规则时实时调用后端预览接口，在模拟上下文中渲染注入效果 |
| **作用域隔离** | 同一规则不会意外在不该生效的会话类型（单聊/群聊）中触发 |
| **零历史污染** | 用户后缀（`user_suffix`）仅修改发往模型的上下文，不写入本地消息历史 |
| **类型内排序** | 同类型规则按 `sortOrder` 升序执行，不同类型间互不干扰 |

---

## 2. 规则类型系统

### 2.1 TarvenRule 接口详解

> 文件位置：`src/core/stores/tarvenStore.ts` 第 6–23 行

```typescript
export interface TarvenRule {
  id: string;                                    // 唯一标识
  name: string;                                  // 显示名称
  ruleType: 'system_suffix' | 'user_suffix' | 'context_inject';
  isEnabled: boolean;                            // 是否激活
  content: string;                               // 注入内容
  scope: 'global' | 'agent' | 'group';           // 作用范围
  wrap: boolean;                                 // 是否 XML 包裹
  
  // context_inject 专用
  role?: 'user' | 'assistant';                   // 虚拟消息角色
  depth?: number;                                // 插入深度
  
  // system_suffix / user_suffix 专用
  position?: 'prepend' | 'append';               // 前置 / 后置
  
  sortOrder: number;                             // 同类型内排序权重
}
```

**前后端字段映射**：

Rust 后端使用 `#[serde(rename_all = "camelCase")]`，因此前端 `ruleType` 自动映射到 Rust 的 `rule_type`，`sortOrder` 映射到 `sort_order`，`isEnabled` 映射到 `is_enabled`，无需手动转换。这是 Tauri v2 与 serde 的零成本互操作特性。

### 2.2 三种规则类型对比

| 维度 | `system_suffix` | `user_suffix` | `context_inject` |
|------|-----------------|---------------|------------------|
| **注入目标** | 系统提示词（`role: system`） | 最新一轮用户消息（`role: user`） | 对话历史任意深度 |
| **生效位置** | 数组首位 | 数组中最后一个 `user` 消息 | 按 `depth` 计算插入点 |
| **专用字段** | `position` | `position` | `role`, `depth` |
| **是否写库** | 否（仅修改请求上下文） | 否（仅修改请求上下文） | 否（仅修改请求上下文） |
| **XML 包裹** | 支持（`<vcp_injection>`） | 支持 | 支持 |
| **占位符支持** | `{{AgentName}}`, `{{VCPChatAgentName}}` | 无 | 无 |
| **典型用途** | 长期记忆/角色设定 | 临时格式要求 | Few-shot/引导性指令 |

#### 2.2.1 system_suffix

在系统提示词的前端或后端追加自定义内容。适用于：
- 为特定 Agent 追加长期记忆或背景设定
- 注入当前时间、运行环境等动态信息（由后端自动追加）
- 修改 Agent 的行为边界而不改动其原始系统提示词

拼接逻辑（Rust 侧）：
```
[前置规则内容]\n\n[原始系统提示词]\n\n[后置规则内容]
```

多条 `system_suffix` 规则同时生效时，所有 `prepend` 规则按 `sortOrder` 升序拼接后置于原始提示词之前；所有 `append` 规则按同样顺序拼接后置于原始提示词之后。前后置组之间用双换行分隔。

#### 2.2.2 user_suffix

在用户发出的最新一条消息文本前后追加内容。适用于：
- 在用户输入后自动附加格式要求（如"请用 Markdown 回复"）
- 注入临时指令而不改变用户原始输入的视觉效果
- 与 `system_suffix` 配合实现"系统层设定 + 用户层微调"的双层控制

> **关键区别**：`user_suffix` 仅修改发往 LLM 的上下文载荷，本地数据库中存储的仍是用户原始输入。这保证了历史记录的可读性与可回溯性。流式输出期间，用户看到的仍是自己输入的原始文本，模型接收的则是附加后的完整版本。

#### 2.2.3 context_inject

在对话历史的指定深度插入一条虚拟消息。适用于：
- 在上下文末尾注入"总结性指令"引导模型回复风格
- 在特定位置插入参考文档或 Few-shot 示例
- 实现类似"系统消息但放在上下文末尾"的 Jailbreak 技巧

**深度语义**：
- `depth = 0`：插入到非系统消息的最末尾（紧接在最新用户消息之后）
- `depth = N`：从末尾向前数第 `N+1` 条消息之前
- 多条 `context_inject` 规则按 `depth` 从大到小排序后依次插入，避免索引错位

**示例**（4 条非系统消息，2 条注入规则）：
```
原始:    [u1, a1, u2, a2]            (u=用户, a=助手)
规则 X:  depth=0, role=assistant     → 插入到末尾之后（index=4）
规则 Y:  depth=2, role=user          → 插入到 u2 之前（index=2）

结果:    [u1, a1, Y, u2, a2, X]
```

### 2.3 Scope 与过滤逻辑

> 过滤逻辑实现位置：`src-tauri/src/vcp_modules/chat/context_injection.rs` 第 43–76 行

```sql
SELECT ... FROM tarven_rules 
WHERE is_enabled = 1 AND (scope = 'global' OR scope = ?)
ORDER BY sort_order ASC
```

| scope 值 | 生效场景 | 参数绑定 |
|----------|----------|----------|
| `global` | 所有会话类型 | 无条件匹配 |
| `agent` | 仅单聊会话 | `scope = 'agent'` |
| `group` | 仅群聊会话 | `scope = 'group'` |

**前端展示过滤**：TarvenSelector 当前展示全部规则（由后端 `get_tarven_rules` 返回），但每条规则都标注了 scope 标签（全局 / 智能体 / 群组）。实际注入时由后端根据当前会话类型进行二次过滤，前端无需额外过滤逻辑。

---

## 3. 规则状态（tarvenStore）

### 3.1 核心状态字段

> 文件位置：`src/core/stores/tarvenStore.ts` 第 25–27 行

```typescript
const rules = ref<TarvenRule[]>([]);      // 全量规则列表
const isSelectorOpen = ref(false);        // 底部选择器开关
```

`rules` 按 `sortOrder` 升序排列，由后端 `ORDER BY sort_order ASC` 保证。列表中包含所有 scope 的规则，UI 层通过标签区分展示。

### 3.2 规则 CRUD 操作

| 方法 | 签名 | 职责 | 后端命令 |
|------|------|------|----------|
| `fetchRules` | `async () => void` | 从 SQLite 加载全部规则 | `get_tarven_rules` |
| `saveRule` | `async (rule: TarvenRule) => void` | 保存规则（新建或更新），成功后刷新列表 | `save_tarven_rule` |
| `deleteRule` | `async (id: string) => void` | 删除规则，成功后刷新列表 | `delete_tarven_rule` |
| `toggleRule` | `async (id: string) => void` | 切换单条规则的启用状态，失败自动回滚 | `toggle_rule_enabled` |
| `saveOrder` | `async (orderedIds: string[]) => void` | 保存拖拽/点击重排后的顺序 | `reorder_rules` |
| `previewInjection` | `async (rules, mockMessages?) => any[]` | 调用后端预览注入效果 | `preview_tarven_injection` |

**乐观更新与回滚**（`toggleRule`）：

```
用户点击切换
    │
    ▼
┌─────────────────────┐
│ 前端立即翻转状态     │──> target.isEnabled = !target.isEnabled
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│ 调用 toggle_rule_   │──> 后端更新 SQLite
│ enabled             │
└──────────┬──────────┘
      成功 │      失败
      ┌────┘        └────┐
      ▼                  ▼
   保持新状态      回滚前端状态
                 target.isEnabled = !target.isEnabled
```

> 代码位置：`src/core/stores/tarvenStore.ts` 第 61–74 行

```typescript
const toggleRule = async (id: string) => {
  const target = rules.value.find(r => r.id === id);
  if (target) {
    try {
      const nextState = !target.isEnabled;
      target.isEnabled = nextState;                    // 乐观更新
      await invoke('toggle_rule_enabled', { id, enabled: nextState });
    } catch (e) {
      target.isEnabled = !target.isEnabled;            // 失败回滚
    }
  }
};
```

### 3.3 与 Agent/Group 的 scope 过滤

`tarvenStore` 本身不维护当前会话类型状态，而是依赖后端在注入时进行 scope 过滤。这种设计的优点是：

1. **前端简单**：选择器直接展示全部规则，用户可随时查看所有规则的状态
2. **一致性**：同一份规则列表同时服务于选择器和设置面板，避免数据分叉
3. **安全性**：即使前端存在展示漏洞，后端注入逻辑仍会严格执行 `scope = 'global' OR scope = ?` 的过滤

---

## 4. 规则选择器（TarvenSelector.vue）

### 4.1 触发方式与位置

> 文件位置：`src/features/chat/components/TarvenSelector.vue`

TarvenSelector 是一个**全局底部抽屉（BottomSheet）**，通过 `Teleport to="body"` 挂载到 body 下，独立于 ChatView 的 DOM 层级。这避免了被父容器的 `overflow: hidden` 裁剪，同时确保 z-index 在全局层级体系中正确生效（`z-sheet`，语义值 50）。

**触发路径**：

```
用户长按 InputEnhancer 的 "+" 按钮
    │
    ▼
InputEnhancer.openTarvenSelector()
    │
    ▼
tarvenStore.isSelectorOpen = true
    │
    ▼
TarvenSelector 监听到变化 → fetchRules() → 滑出抽屉
```

> 长按触发代码位置：`src/features/chat/InputEnhancer.vue` 第 532 行（`v-longpress="openTarvenSelector"`）

### 4.2 列表展示与筛选

抽屉体采用 **iOS 风格磨砂玻璃面板**（`backdrop-blur-xl` + `bg-white/95 dark:bg-zinc-900/95`），顶部有拖手线（`w-10 h-1 bg-black/10 rounded-full`）指示可下滑关闭。

头部展示两层标题：
- 上层：`Context System`（10px，uppercase，tracking-widest，灰色）
- 下层：`VCPChatTarven 规则仓`（17px，extrabold，主色）
- 右侧齿轮按钮：点击后关闭抽屉并打开 TarvenSettings SlidePage

每条规则展示以下信息：

```
┌─────────────────────────────────────────┐
│ ┌──┐  规则名称                  ┌────┐  │
│ │✨│  ┌────────┐ ┌────┐        │ ●  │  │
│ └──┘  │类型标签 │ │scope│        └────┘  │
│       └────────┘ └────┘                 │
└─────────────────────────────────────────┘
```

- **左侧图标**：`i-heroicons-sparkles`，启用时为 emerald 主题色背景（`!bg-emerald-500/10 !text-emerald-500`）
- **名称**：启用时加粗高亮（`text-emerald-600 dark:text-emerald-400`）
- **类型标签**（9px，uppercase，tracking-wider）：
  - `system_suffix` → 蓝色（`bg-blue-500/10 text-blue-500 border-blue-500/20`）
  - `user_suffix` → 翠绿色（`bg-emerald-500/10 text-emerald-500 border-emerald-500/20`）
  - `context_inject` → 橙色（`bg-orange-500/10 text-orange-500 border-orange-500/20`）
- **scope 标签**：灰色（全局 / 智能体 / 群组）
- **注入专用信息**：`context_inject` 类型额外显示 `角色 · 深度 N`
- **右侧 Switch**：iOS 经典样式，42×24px，启用时为 emerald，带动画过渡

**缺省状态**：当 `tarvenStore.rules.length === 0` 时，展示空态插图（sparkles 图标 + "尚未配置任何规则"），并提供 "立即添加规则" 按钮直接跳转设置。

### 4.3 启用/禁用切换

点击整行卡片即可切换规则的启用状态：

```typescript
const toggleRuleState = (id: string) => {
  tarvenStore.toggleRule(id);
};
```

卡片边框与背景会根据启用状态实时变化：
- 启用：`border-emerald-500/40 bg-emerald-500/[0.04]`
- 禁用：`opacity-70`（整体透明度降低）

该操作调用后端的 `toggle_rule_enabled` 命令，更新 `tarven_rules.is_enabled` 字段与 `updated_at` 时间戳。

### 4.4 与输入栏的集成

> 文件位置：`src/features/chat/InputEnhancer.vue` 第 531–541 行

```vue
<button v-longpress="openTarvenSelector" @click="showAttachMenu = !showAttachMenu">
  <div class="i-heroicons-plus-circle text-2xl"></div>
  <!-- 绿色指示点：当有任何规则处于启用状态时显示 -->
  <div v-if="tarvenStore.rules.some(r => r.isEnabled)" 
    class="absolute top-1.5 right-1.5 w-2 h-2 bg-emerald-500 rounded-full 
           border-2 border-[var(--secondary-bg)] 
           shadow-[0_0_8px_rgba(16,185,129,0.5)]">
  </div>
</button>
```

- **短按**：展开附件菜单（拍摄 / 相册 / 文件）
- **长按**（`v-longpress` 指令，约 350ms）：触发 TarvenSelector 抽屉
- **指示点**：emerald 绿色小圆点 + 发光阴影，提示用户"有规则正在生效"
- **触觉反馈**：`navigator.vibrate(50)` 在长按时提供确认感

---

## 5. 规则设置面板（TarvenSettings.vue）

### 5.1 打开方式

TarvenSettings 通过 `SlidePage` 组件实现全屏页面滑入，由 `overlayStore` 统一管理页面栈。`SlidePage` 属于项目的全局页面栈系统，支持从右侧滑入、左滑返回手势、以及多层页面的 z-index 自动计算。

**打开路径**：

```
路径 A：TarvenSelector 齿轮按钮
    │
    ▼
overlayStore.openTarvenSettings()
    │
    ▼
SlidePage 滑入 (z-index = overlayStore.getPageZIndex('tarven'))

路径 B：长按 "+" 按钮 → 点击"立即添加规则"
    │
    ▼
关闭 TarvenSelector → 延迟 200ms → overlayStore.openTarvenSettings()
```

> 延迟设计意图：`setTimeout(() => { overlayStore.openTarvenSettings() }, 200)` 保证 BottomSheet 收起动画（`slide-up-leave-active: transform 0.35s`）与 SlidePage 滑入过渡不重叠，避免视觉抖动与性能峰值。

### 5.2 表单字段与验证

设置面板采用**双视图架构**：`list`（规则列表）与 `form`（新建/编辑表单）。通过 `currentView` 响应式变量控制，配合 `animate-fade-in` 动画实现平滑过渡。

#### 5.2.1 列表视图（`list`）

按 `ruleType` 分为三个可折叠分区，使用 `collapsedSections` 对象（`system_suffix` / `user_suffix` / `context_inject`）分别控制：

| 分区 | 标签色 | 头部信息 | 操作 |
|------|--------|----------|------|
| 系统提示词注入 | 蓝色（primary） | 规则数量 + 折叠箭头 | 上/下排序、编辑、删除、点击卡片切换启用 |
| 用户消息注入 | 翠绿色（emerald） | 规则数量 + 折叠箭头 | 同上 |
| 上下文消息注入 | 橙色 | 规则数量 + 折叠箭头 | 同上 |

每个分区的头部显示当前规则数量（`font-mono`，10px），点击可折叠/展开（箭头旋转 `-rotate-90`）。空分区展示虚线边框的缺省提示。

**规则卡片布局**：

```
┌─────────────────────────────────────────────────────────┐
│ 规则名称                              [▲] [▼] [✎] [🗑] │
│ ┌──────────┐ ┌──────┐ ┌──────┐                          │
│ │系统提示词 │ │全局  │ │后置  │                          │
│ └──────────┘ └──────┘ └──────┘                          │
└─────────────────────────────────────────────────────────┘
```

- 上/下箭头：调整同类型内的 `sortOrder`，边界状态自动 `disabled:opacity-10`
- 编辑按钮（✎）：打开表单视图并回填当前规则数据
- 删除按钮（🗑）：触发优雅删除确认弹窗
- 整行点击：切换启用状态（与 TarvenSelector 行为一致）

**排序逻辑**：

```typescript
const handleMove = async (rule: TarvenRule, direction: 'up' | 'down') => {
  // 1. 取出同类型规则并按 sortOrder 排序
  const sameTypeRules = tarvenStore.rules
    .filter(r => r.ruleType === rule.ruleType)
    .sort((a, b) => a.sortOrder - b.sortOrder);
  
  // 2. 找到当前索引并计算目标索引
  const index = sameTypeRules.findIndex(r => r.id === rule.id);
  const targetIndex = direction === 'up' ? index - 1 : index + 1;
  
  // 3. 交换相邻元素的 ID
  const sameTypeIds = sameTypeRules.map(r => r.id);
  const temp = sameTypeIds[index];
  sameTypeIds[index] = sameTypeIds[targetIndex];
  sameTypeIds[targetIndex] = temp;
  
  // 4. 合并其他类型规则的 ID（保持其原有顺序不变）
  const otherTypeIds = tarvenStore.rules
    .filter(r => r.ruleType !== rule.ruleType)
    .map(r => r.id);
  
  const finalOrderedIds = [...otherTypeIds, ...sameTypeIds];
  await tarvenStore.saveOrder(finalOrderedIds);
};
```

> 关键实现：`reorder_rules` 后端命令接收**全局**规则 ID 数组，按数组索引重写所有规则的 `sort_order`。因此前端需要构造包含所有规则（不仅仅是当前类型）的完整 ID 数组。其他类型规则的相对顺序被保留，仅当前类型的内部顺序发生变化。

#### 5.2.2 表单视图（`form`）

新建规则时的默认值（`openForm` 无参数时）：

```typescript
{
  name: '',
  ruleType: 'system_suffix',
  content: '',
  isEnabled: true,
  scope: 'global',
  wrap: true,
  role: 'user',
  depth: 0,
  position: 'append',
  sortOrder: tarvenStore.rules.length
}
```

**表单字段矩阵**：

| 字段 | 控件类型 | 所有类型 | system_suffix | user_suffix | context_inject |
|------|----------|----------|---------------|-------------|----------------|
| 规则名称 | 文本输入（13px bold） | ✅ | ✅ | ✅ | ✅ |
| 注入类型 | 三选一胶囊（Segmented Capsule） | ✅ | ✅ | ✅ | ✅ |
| 作用范围 | 三选一胶囊 | ✅ | ✅ | ✅ | ✅ |
| XML 包裹 | 复选框 + 标签说明 | ✅ | ✅ | ✅ | ✅ |
| 拼接位置 | 单选（前置/后置） | ❌ | ✅ | ✅ | ❌ |
| 虚拟消息角色 | 二选一胶囊（用户/智能体） | ❌ | ❌ | ❌ | ✅ |
| 插入深度 | 数字输入（0–20，mono 字体） | ❌ | ❌ | ❌ | ✅ |
| 规则内容 | 多行文本域（12px，6 行） | ✅ | ✅ | ✅ | ✅ |

**保存按钮**：在 `name` 和 `content` 非空时启用（`:disabled="!editingRule.name || !editingRule.content"`），点击后构造完整 `TarvenRule` 对象：

```typescript
const ruleData: TarvenRule = {
  id: id || `rule_${Date.now()}_${Math.random().toString(36).substring(2, 7)}`,
  name,
  ruleType: ruleType as any,
  content,
  isEnabled: isEnabled !== false,
  scope: scope || 'global',
  wrap: wrap !== false,
  role: role || 'user',
  depth: depth ?? 0,
  position: position || 'append',
  sortOrder: sortOrder ?? 0,
};
```

ID 生成策略：`rule_` 前缀 + 毫秒时间戳 + 5 位随机字符，确保全局唯一且人类可读。

**删除确认弹窗**：独立的自定义弹窗（`z-dialog`），非系统 `confirm()`。包含 danger 色图标、说明文字、取消/确认双按钮网格布局。

### 5.3 规则预览

表单视图内置 **WYSIWYG 实时预览**，通过深度监听 `editingRule` 的所有字段变化自动触发：

```typescript
watch(
  () => [
    editingRule.value.name,
    editingRule.value.content,
    editingRule.value.ruleType,
    editingRule.value.scope,
    editingRule.value.wrap,
    editingRule.value.role,
    editingRule.value.depth,
    editingRule.value.position,
  ],
  () => { if (currentView.value === 'form') updatePreview(); },
  { deep: true }
);
```

**预览流程**：

```
用户修改任意字段
    │
    ▼
构造 draftRule（id='draft'，isEnabled=true，其余字段取当前表单值）
    │
    ▼
tarvenStore.previewInjection([draftRule])
    │
    ▼
Rust preview_tarven_injection()
    │
    ▼
使用默认模拟上下文 或 用户提供的 mockMessages
    │
    ▼
执行与真实注入完全一致的逻辑（同一份代码路径）
    │
    ▼
返回注入后的消息数组
    │
    ▼
前端渲染为带角色标签的卡片列表
    ──> 注入消息带有 __tavernInjected=true 标记，
        以虚线边框 + primary 色高亮展示
```

**默认模拟上下文**（4 条消息）：
```json
[
  { "role": "system", "content": "你是一个智能助手。" },
  { "role": "user", "content": "你好，请问你是？" },
  { "role": "assistant", "content": "我是你的 AI 助手，有什么可以帮你的吗？" },
  { "role": "user", "content": "帮我写一首关于秋天的诗。" }
]
```

预览区域使用 `font-mono` 等宽字体展示 JSON 结构化的消息列表，每条消息显示 `[index] role` 与内容文本。注入消息额外标注 "XML 格式注入" 徽章（8px，primary 背景色）。`__tavernInjected` 标记在预览中渲染为 `border-dashed border-primary/50` 的虚线边框，与正常消息区分。

---

## 6. 数据流时序

### 6.1 规则注入的完整时序

```
┌──────────┐     ┌──────────────┐     ┌─────────────┐     ┌──────────────┐     ┌────────────┐
│  用户    │     │  InputEnhancer│     │  ChatView   │     │  Rust 后端   │     │ VCP 服务器 │
└────┬─────┘     └──────┬───────┘     └──────┬──────┘     └──────┬───────┘     └─────┬──────┘
     │                  │                    │                   │                   │
     │  长按 "+" 按钮   │                    │                   │                   │
     │─────────────────►│                    │                   │                   │
     │                  │                    │                   │                   │
     │                  │  tarvenStore.      │                   │                   │
     │                  │  isSelectorOpen    │                   │                   │
     │                  │  = true            │                   │                   │
     │                  ├───────────────────►│                   │                   │
     │                  │                    │                   │                   │
     │                  │                    │  TarvenSelector   │                   │
     │                  │                    │  滑出 + fetchRules│                   │
     │                  │                    │                   │                   │
     │  点击规则卡片    │                    │                   │                   │
     │  （切换启用）    │                    │                   │                   │
     │─────────────────►│                    │                   │                   │
     │                  │  toggleRule(id)    │                   │                   │
     │                  ├───────────────────►│                   │                   │
     │                  │                    │  invoke           │                   │
     │                  │                    │  toggle_rule_     │                   │
     │                  │                    │  enabled          │                   │
     │                  │                    ├──────────────────►│                   │
     │                  │                    │                   │  UPDATE SQLite    │
     │                  │                    │                   │  tarven_rules     │
     │                  │                    │  ◄───────────────┤                   │
     │                  │                    │  OK               │                   │
     │                  │  抽屉保持打开       │                   │                   │
     │◄─────────────────┤  状态已更新        │                   │                   │
     │                  │                    │                   │                   │
     │  点击"完成"关闭   │                    │                   │                   │
     │─────────────────►│                    │                   │                   │
     │                  │                    │                   │                   │
     │  输入文本并发送   │                    │                   │                   │
     │─────────────────►│                    │                   │                   │
     │                  │  emit('send')      │                   │                   │
     │                  ├───────────────────►│                   │                   │
     │                  │                    │  historyStore.    │                   │
     │                  │                    │  sendMessage()    │                   │
     │                  │                    │                   │                   │
     │                  │                    │  invoke           │                   │
     │                  │                    │  handle_agent_    │                   │
     │                  │                    │  chat_message     │                   │
     │                  │                    ├──────────────────►│                   │
     │                  │                    │                   │                   │
     │                  │                    │                   │  apply_tarven_    │
     │                  │                    │                   │  pipeline()       │
     │                  │                    │                   │  ────────────────►│
     │                  │                    │                   │                   │
     │                  │                    │                   │  1. fetch_active_ │
     │                  │                    │                   │     rules(scope)  │
     │                  │                    │                   │  2. 系统提示词注入 │
     │                  │                    │                   │  3. 用户消息后缀   │
     │                  │                    │                   │  4. 上下文节点插入 │
     │                  │                    │                   │                   │
     │                  │                    │                   │  perform_vcp_     │
     │                  │                    │                   │  request(...)     │
     │                  │                    │                   ├──────────────────►│
     │                  │                    │                   │                   │
     │                  │                    │                   │  SSE 流式响应      │
     │                  │                    │                   │◄──────────────────┤
     │                  │                    │                   │                   │
```

### 6.2 规则编辑保存时序

```
┌──────────────┐     ┌─────────────────┐     ┌─────────────┐     ┌──────────────┐
│   用户       │     │ TarvenSettings  │     │ tarvenStore │     │ Rust 后端    │
└──────┬───────┘     └────────┬────────┘     └──────┬──────┘     └──────┬───────┘
       │                      │                     │                   │
       │  在列表视图点击       │                     │                   │
       │  "创建自定义注入规则" │                     │                   │
       │─────────────────────►│                     │                   │
       │                      │                     │                   │
       │                      │  currentView='form' │                   │
       │                      │  editingRule={默认值}│                   │
       │                      │                     │                   │
       │  填写表单字段         │                     │                   │
       │─────────────────────►│                     │                   │
       │                      │                     │                   │
       │                      │  watch 触发         │                   │
       │                      │  updatePreview()    │                   │
       │                      ├────────────────────►│                   │
       │                      │                     │  previewTarven_   │
       │                      │                     │  Injection(...)   │
       │                      │                     ├──────────────────►│
       │                      │                     │                   │
       │                      │                     │◄──────────────────┤
       │                      │◄────────────────────┤  previewMessages  │
       │                      │  渲染预览卡片        │                   │
       │◄─────────────────────┤                     │                   │
       │                      │                     │                   │
       │  点击"保存规则"       │                     │                   │
       │─────────────────────►│                     │                   │
       │                      │                     │                   │
       │                      │  handleSave()       │                   │
       │                      │  构造完整 ruleData  │                   │
       │                      ├────────────────────►│                   │
       │                      │                     │  saveRule(rule)   │
       │                      │                     │                   │
       │                      │                     │  invoke           │
       │                      │                     │  save_tarven_rule │
       │                      │                     ├──────────────────►│
       │                      │                     │                   │
       │                      │                     │                   │  SQLite
       │                      │                     │                   │  UPSERT
       │                      │                     │◄──────────────────┤
       │                      │                     │  OK               │
       │                      │                     │                   │
       │                      │                     │  fetchRules()     │
       │                      │                     │  （自动刷新列表）  │
       │                      │◄────────────────────┤                   │
       │                      │  closeForm()        │                   │
       │                      │  currentView='list' │                   │
       │◄─────────────────────┤                     │                   │
       │                      │                     │                   │
```

---

## 7. 与 Rust 后端的 IPC 交互

### 7.1 Tauri Commands 调用表

> 命令定义位置：`src-tauri/src/vcp_modules/chat/context_injection.rs` 第 279–409 行  
> 命令注册位置：`src-tauri/src/lib.rs` 第 188–195 行

| 命令名 | 签名（Rust） | 输入 | 输出 | 前端调用方 |
|--------|-------------|------|------|----------|
| `get_tarven_rules` | `async fn(db_state) -> Result<Vec<TarvenRule>, String>` | `DbState` | 全部规则列表（按 `sort_order` 升序） | `tarvenStore.fetchRules()` |
| `save_tarven_rule` | `async fn(db_state, rule) -> Result<(), String>` | `DbState`, `TarvenRule` | `()` | `tarvenStore.saveRule()` |
| `delete_tarven_rule` | `async fn(db_state, id) -> Result<(), String>` | `DbState`, `String` | `()` | `tarvenStore.deleteRule()` |
| `toggle_rule_enabled` | `async fn(db_state, id, enabled) -> Result<(), String>` | `DbState`, `String`, `bool` | `()` | `tarvenStore.toggleRule()` |
| `reorder_rules` | `async fn(db_state, rule_ids) -> Result<(), String>` | `DbState`, `Vec<String>` | `()` | `tarvenStore.saveOrder()` |
| `preview_tarven_injection` | `async fn(rules, mock_messages) -> Result<Vec<Value>, String>` | `Vec<TarvenRule>`, `Option<Vec<Value>>` | 注入后的消息数组 | `tarvenStore.previewInjection()` |

**`save_tarven_rule` 的 UPSERT 语义**：

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

- `id` 已存在 → 更新全部字段（除 `created_at` 外）
- `id` 不存在 → 新建记录，`created_at` 与 `updated_at` 均为当前时间戳
- `updated_at` 使用毫秒级 Unix 时间戳

**`reorder_rules` 的事务保证**：

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

全有或全无（All-or-Nothing）：任一规则更新失败则整个排序操作回滚，保证 `sort_order` 的连续性。

### 7.2 事件监听表

Tarven 系统**不使用** Tauri 事件通道（`listen` / `emit`）进行通信。全部交互均通过同步式的 `invoke` 调用完成，原因：

1. 规则数据量小（通常 < 50 条），无需流式推送
2. 实时预览通过即时 `invoke` 调用即可满足响应需求
3. 启用状态切换需要事务保证，异步事件难以处理失败回滚
4. 规则属于用户主动触发的配置操作，而非被动接收的后台通知

前端唯一的事件类交互是 `useModalHistory` 对系统返回键的处理：

| 事件源 | 监听者 | 行为 |
|--------|--------|------|
| 系统返回键 / 手势 | `TarvenSelector` 的 `registerModal` | 关闭 BottomSheet |
| 系统返回键 / 手势 | `TarvenSettings` 的 `registerModal` | Form 视图时返回列表，列表时关闭 SlidePage |

---

## 8. 与 Agent/Group 设置的集成

Tarven 规则系统与 Agent/Group 设置存在**间接耦合**，而非直接依赖：

| 集成点 | 方向 | 说明 |
|--------|------|------|
| **Scope 过滤** | 后端驱动 | `apply_tarven_pipeline` 根据传入的 `scope` 参数（`'agent'` 或 `'group'`）自动过滤规则，前端无需感知当前会话类型 |
| **AgentName 占位符** | 后端替换 | `system_suffix` 规则内容中的 `{{AgentName}}` 和 `{{VCPChatAgentName}}` 由后端在注入时替换为当前 Agent 的 `name` 字段 |
| **基础环境注入** | 后端自动 | `inject_base_environment()` 在所有 `system_suffix` 规则之前自动插入当前时间、运行环境（`VCP Mobile (Android 移动端)`）、话题创建时间等信息 |
| **设置入口** | 前端独立 | TarvenSettings 通过 `overlayStore` 独立打开，不嵌入 AgentSettings 或 GroupSettings 内部 |

**占位符替换时机**：在 `system_suffix` 规则内容拼接完成后、回写 `messages` 数组之前，由 Rust 侧执行：

```rust
system_content = system_content
    .replace("{{AgentName}}", agent_name)
    .replace("{{VCPChatAgentName}}", agent_name);
```

**未来扩展预留**：
- `scope = 'agent'` 的规则可进一步细化为"仅对特定 Agent ID 生效"，当前版本未实现 per-agent 绑定
- `scope = 'group'` 同理可扩展为 per-group 绑定
- 可引入规则"条件触发"机制（如关键词匹配），当前版本为无条件注入

---

## 9. 设计决策与注意事项

### 9.1 为何 `user_suffix` 不写历史记录？

`user_suffix` 仅修改发往 LLM 的上下文载荷，用户本地数据库中的消息保持原始内容。这一决策基于三个考量：

1. **可回溯性**：用户日后回看聊天记录时，不应看到被自动附加的指令文本（如"请用 Markdown 回复"）
2. **可编辑性**：如果用户编辑历史消息重新发送，不应重复附加后缀，否则会导致指令堆叠
3. **隐私性**：某些后缀可能包含临时指令或敏感提示，不应永久留存于本地数据库

### 9.2 为何预览需要调用后端而非前端模拟？

`preview_tarven_injection` 命令执行与真实注入**完全一致的算法**（同一套 `render_rule_content` + 拼接逻辑），而非前端自行模拟。这保证了"预览即实际"，消除前后端逻辑分叉导致的"预览与实际不符"Bug。

Rust 侧预览函数与真实注入函数共享以下子逻辑：
- `render_rule_content(rule)`：XML 包裹处理
- 系统提示词前置/后置拼接算法
- 用户消息后缀拼接算法
- `context_inject` 的 depth 排序与插入算法

唯一的区别是预览使用模拟上下文（或用户传入的 mock），而真实注入使用实际对话历史。

### 9.3 为何 `context_inject` 的 depth 从大到小排序？

当多条 `context_inject` 规则同时生效时，按 `depth` 从大到小排序后依次插入，可避免因前面插入操作导致数组长度变化、进而使后续 `depth` 计算错位的问题。

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

### 9.4 移动端交互的特殊处理

- **长按触发**：TarvenSelector 通过 `v-longpress` 绑定在 "+" 按钮上，与短按展开附件菜单不冲突。这是移动端屏幕空间有限时的典型设计模式——同一按钮承载两个频度不同的操作
- **振动反馈**：`navigator.vibrate(50)` 在长按时提供触觉确认，`vibrate(35)` 在切换语音模式时提供轻反馈
- **安全区域适配**：BottomSheet 底部 padding 使用 `calc(env(safe-area-inset-bottom, 20px) + 12px)` 适配刘海屏与底部手势条
- **手势防冲突**：遮罩层 `@touchmove.prevent` 阻止底层页面跟随滑动；抽屉内容区 `scrollbar-none` 隐藏滚动条保持原生感
- **返回键管理**：通过 `useModalHistory` 注册模态，确保 Android 系统返回键先关闭抽屉/页面，而非直接退出应用

### 9.5 wrap（XML 包裹）的用途

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

### 9.6 数据库表结构

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

索引：
```sql
CREATE INDEX IF NOT EXISTS idx_tarven_rules_active 
ON tarven_rules(rule_type, is_enabled, sort_order ASC);
```

该复合索引支撑 `fetch_active_rules` 的核心过滤场景：`rule_type` 分组 → `is_enabled = 1` 筛选 → `sort_order` 排序。

---

## 10. 术语速查表

| 术语 | 英文/缩写 | 定义 | 相关文件 |
|------|----------|------|----------|
| Tarven | — | VCP Mobile 的上下文注入规则系统，名称致敬 SillyTavern | 全部 |
| TarvenRule | — | 单条注入规则的数据结构，定义注入内容、类型、范围与参数 | `tarvenStore.ts`, `context_injection.rs` |
| system_suffix | 系统提示词注入 | 在系统提示词前/后追加内容的规则类型 | `context_injection.rs` |
| user_suffix | 用户消息注入 | 在最新用户消息前/后追加内容的规则类型 | `context_injection.rs` |
| context_inject | 上下文消息注入 | 在对话历史指定深度插入虚拟消息的规则类型 | `context_injection.rs` |
| scope | 作用范围 | 规则生效的会话类型：`global` / `agent` / `group` | `tarvenStore.ts` |
| wrap | XML 包裹 | 是否用 `<vcp_injection>` 标签包裹注入内容 | `context_injection.rs` |
| depth | 插入深度 | `context_inject` 专用：0=末尾，N=倒数第 N+1 条之前 | `TarvenSettings.vue` |
| position | 拼接位置 | `system_suffix` / `user_suffix` 专用：`prepend` 前置 / `append` 后置 | `TarvenSettings.vue` |
| sortOrder | 排序权重 | 同类型规则间的执行顺序，升序排列 | `tarvenStore.ts` |
| BottomSheet | 底部抽屉 | TarvenSelector 的 UI 模式，从屏幕底部滑出 | `TarvenSelector.vue` |
| SlidePage | 滑页 | TarvenSettings 的 UI 容器，从右侧滑入覆盖当前页面 | `TarvenSettings.vue` |
| WYSIWYG 预览 | 所见即所得 | 编辑规则时实时渲染注入效果，使用模拟上下文 | `TarvenSettings.vue` |
| 乐观更新 | Optimistic Update | toggleRule 先改前端状态再调后端，失败自动回滚 | `tarvenStore.ts` |
| UPSERT | — | SQLite 的 `INSERT ... ON CONFLICT DO UPDATE` 原子操作 | `context_injection.rs` |
| 基础环境注入 | Base Env Injection | 后端自动在系统提示词前追加当前时间、运行环境等信息 | `context_injection.rs` |
| `__tavernInjected` | 注入标记 | 预览中标记哪些消息是由规则注入产生的元数据字段 | `context_injection.rs` |
| Segmented Capsule | 分段胶囊 | 表单中使用的三选一/二选一按钮组 UI 模式 | `TarvenSettings.vue` |
| 长按触发 | Long-press Trigger | 按住按钮 350ms 以上触发 TarvenSelector 的交互方式 | `InputEnhancer.vue` |

---
*最后更新：2026-06-05 | VCP Mobile v1.0.3*
