---
id: VUE-AGEN-012
title: Agent侧边栏与列表交互
description: VCP Mobile 前端 AgentSidebar 布局、AgentList 拖拽排序、Swipe 手势与搜索过滤的交互设计
version: 0.9.14
date: 2026-05-27
---

# 12. Agent侧边栏与列表交互

## 1. 概述

### 1.1 领域定位

`Agent侧边栏与列表交互`是 VCP Mobile 前端 **Agent 领域** 的入口级交互系统，负责智能体（Agent）与群组（Group）的列表展示、筛选、排序、快速操作，以及作为全局导航枢纽的侧边栏容器管理。它是用户与 AI 助手建立会话前的第一道交互界面，直接影响核心聊天功能的可达性。

功能边界严格限定为：

- **侧边栏容器**：打开/关闭动画、手势响应、与页面主内容的层级关系
- **列表展示**：Agent 与 Group 的分层渲染、头像、模型标识、未读角标
- **搜索过滤**：按名称实时过滤，无后端查询开销
- **排序交互**：拖拽重排，本地顺序持久化到设置存储
- **卡片级手势**：水平滑动展开编辑入口，与列表滚动和侧边栏滑出进行冲突仲裁
- **新建引导**：Agent / Group 的快速创建流程入口

该功能**不**涉及：
- Agent 详细配置的表单编辑（由 `AgentSettingsView.vue` / `GroupSettingsView.vue` 负责）
- 话题（Topic）的列表管理（由 `TopicList.vue` 负责，挂载在同一侧边栏的 Tab 下）
- 消息历史加载（由 `chatHistoryStore` 与 `chatSessionStore` 负责）

### 1.2 模块构成表

| 文件 | 行数 | 职责 |
|------|------|------|
| `src/components/layout/AgentSidebar.vue` | 157 | 侧边栏容器：布局骨架、抽屉状态绑定、Tabs/Search/List/Creator 的组装站 |
| `src/features/agent/AgentList.vue` | 439 | 列表核心：Group + Agent 双层列表、SortableJS 拖拽排序、Swipe 手势实现 |
| `src/features/agent/SidebarTabs.vue` | 28 | 标签切换器：Agent/Topic 两态切换按钮组 |
| `src/features/agent/SidebarSearch.vue` | 29 | 搜索输入框：v-model 双向绑定、动态占位符 |
| `src/features/agent/AgentsCreator.vue` | 107 | 底部创建区：Agent / Group 新建按钮与 Prompt 弹窗流程 |
| `src/core/composables/useSidebarSwipe.ts` | 73 | 全局滑动手势：侧边栏从屏幕边缘滑出/滑入的触控识别 |

### 1.3 在整体架构中的位置

```
┌─────────────────────────────────────────────────────────────┐
│                    App.vue (根布局)                          │
│  ┌───────────────────────────────────────────────────────┐  │
│  │         AgentSidebar.vue (z-drawer, z-local@md)       │  │
│  │  ┌─────────────────────────────────────────────────┐  │  │
│  │  │ 顶部区: SidebarTabs + SidebarSearch              │  │  │
│  │  ├─────────────────────────────────────────────────┤  │  │
│  │  │ 内容区: AgentList (v-if="activeTab === 'agents'")│  │  │
│  │  │         ├─ Group 列表 (Sortable + Swipe)         │  │  │
│  │  │         └─ Agent 列表 (Sortable + Swipe)         │  │  │
│  │  ├─────────────────────────────────────────────────┤  │  │
│  │  │ 底部区: AgentsCreator / TopicCreator             │  │  │
│  │  │         + 全局设置入口                            │  │  │
│  │  └─────────────────────────────────────────────────┘  │  │
│  └───────────────────────────────────────────────────────┘  │
│                            │                                │
│         ┌──────────────────┼──────────────────┐            │
│         ▼                  ▼                  ▼            │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────┐    │
│  │ layoutStore │    │assistantStore│    │ settingsStore│   │
│  │(leftDrawer) │    │(agents/groups│    │(agentOrder/ │   │
│  │             │    │/unreadCounts)│    │ groupOrder) │   │
│  └─────────────┘    └─────────────┘    └─────────────┘    │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
              ┌─────────────────────┐
              │  useSidebarSwipe    │
              │  (global / left)    │
              └─────────────────────┘
```

侧边栏作为 `App.vue` 的直接子组件，与 `ChatView` 等主内容区并列。其打开/关闭状态由 `layoutStore.leftDrawerOpen` 单一源驱动，并通过 `useSidebarSwipe` 与全局触控层衔接。

### 1.4 核心设计原则

| 原则 | 说明 |
|------|------|
| **单层状态源** | `layoutStore.leftDrawerOpen` 是侧边栏可见性的唯一真相源，任何组件（包括手势、遮罩点击、物理返回键）均通过 `setLeftDrawer(bool)` 修改，禁止直接赋值 |
| **触控优先** | 全部手势为移动端设计：Swipe 展开编辑、拖拽排序柄（`.drag-handle`）、边缘滑动打开/关闭侧边栏；桌面端（`md` 断点以上）侧边栏常驻，动画与手势禁用 |
| **本地过滤** | 搜索过滤纯前端进行，不调用后端 Command，保证 0 网络延迟；过滤字段仅限 `name`，保证性能 |
| **乐观更新** | 拖拽排序后先更新本地 `settingsStore.settings.agentOrder`，再异步持久化到后端；若后端失败，下次启动时自动回读到旧顺序 |
| **手势仲裁** | Swipe 与垂直滚动、侧边栏关闭、排序拖拽共享同一触控面，通过方向锁定（`hasDeterminedDirection`）+ 排序状态互斥 + `stopPropagation` 分层解决冲突 |

---

## 2. 侧边栏布局（AgentSidebar.vue）

### 2.1 组件结构

`AgentSidebar.vue` 是侧边栏的**布局骨架**，不直接处理列表业务逻辑，而是作为子组件的组装站。整体采用 Flexbox 纵向三栏布局：

```
┌────────────────────────────┐  ← pt-safe (安全区适配)
│  VCP MOBILE 标题            │
│  SidebarTabs               │
│  SidebarSearch             │
├────────────────────────────┤  ← border-b
│                            │
│  AgentList / TopicList     │  ← flex-1 overflow-hidden
│  (滚动内容区)               │
│                            │
├────────────────────────────┤  ← border-t, glass-panel
│  AgentsCreator             │
│  (或 TopicCreator)         │
│  全局设置按钮               │
└────────────────────────────┘  ← pb-safe-bottom
```

> 源码位置：`src/components/layout/AgentSidebar.vue` 第 51–106 行

关键布局类：
- `.vcp-drawer-left`：绝对定位，`width: 82vw / max-width: 340px`
- `.pt-safe`：刘海屏顶部安全区内边距
- `pb-[calc(var(--vcp-safe-bottom,16px)+8px)]`：底部安全区 + 额外间距
- `.vcp-scrollable.no-rubber-band`：内容区滚动，禁用橡皮筋回弹（防止与侧边栏手势冲突）

### 2.2 与 layoutStore 的绑定

侧边栏可见性通过 `layoutStore.leftDrawerOpen` 进行**单向数据流 + CSS class 响应**：

```vue
<aside ref="sidebarRef" class="vcp-drawer vcp-drawer-left flex flex-col" 
       :class="{ 'is-open': layoutStore.leftDrawerOpen }">
```

> 源码位置：`src/components/layout/AgentSidebar.vue` 第 52 行

`layoutStore` 提供的 API：

| API | 类型 | 说明 |
|-----|------|------|
| `leftDrawerOpen` | `Ref<boolean>` | 当前打开状态，驱动 `:class` 绑定 |
| `setLeftDrawer(open)` | `(boolean) => void` | 设置状态；打开左抽屉时自动关闭右抽屉，并注册模态历史 |
| `toggleLeftDrawer()` | `() => void` | 取反切换 |

`setLeftDrawer` 的副作用：
1. **互斥锁**：打开左侧边栏时自动调用 `setRightDrawer(false)`，防止左右抽屉同时出现
2. **模态历史**：通过 `useModalHistory` 注册 `LeftDrawer` 项，使系统返回键（Android 物理返回）优先关闭侧边栏而非退出应用

> 源码位置：`src/core/stores/layout.ts` 第 14–26 行

### 2.3 打开/关闭动画

侧边栏使用纯 CSS `transform` 实现硬件加速动画：

```css
.vcp-drawer {
  transition: transform 0.4s cubic-bezier(0.16, 1, 0.3, 1);
  z-index: var(--layer-drawer);   /* 20 */
}

.vcp-drawer-left {
  transform: translateX(-100%);
}

.vcp-drawer-left.is-open {
  transform: translateX(0);
}
```

> 源码位置：`src/components/layout/AgentSidebar.vue` 第 109–129 行

动画参数：
- **时长**：400ms
- **缓动**：`cubic-bezier(0.16, 1, 0.3, 1)` — 带轻微过冲的减速曲线，使滑入更具物理感
- **属性**：仅 `transform`（GPU 合成层，避免重排）
- **层级**：语义化 `z-drawer`（`var(--layer-drawer)` = 20），位于 `content`（0）之上、`overlay`（30）之下

**桌面端适配**（`@media (min-width: 768px)`）：
- `position: relative` — 不再覆盖主内容
- `transform: translateX(0) !important` — 强制常驻，无视 `is-open`
- `transition: none` — 禁用动画
- `z-index: var(--layer-local)` — 降级为局部层级（10）

### 2.4 手势滑动打开（useSidebarSwipe）

`AgentSidebar.vue` 内部通过 `useSidebarSwipe` 监听侧边栏容器上的**左滑关闭**与**右滑切 Tab** 手势：

```ts
useSidebarSwipe(sidebarRef, {
  type: 'left',
  onTabSwitch: () => {
    if (activeTab.value === 'topics') {
      activeTab.value = 'agents';
    }
  }
});
```

> 源码位置：`src/components/layout/AgentSidebar.vue` 第 21–31 行

`useSidebarSwipe` 同时被 `App.vue` 以 `type: 'global'` 调用，实现**从屏幕左边缘向右滑动打开侧边栏**的全局能力。两种实例共存，通过 `layoutStore.leftDrawerOpen` 状态互斥避免重复响应。

`useSidebarSwipe` 核心参数（`src/core/composables/useSidebarSwipe.ts`）：

| 参数 | 全局模式 (`global`) | 侧边栏内模式 (`left`) |
|------|---------------------|----------------------|
| `threshold` | 30 | 15 |
| 触发位移 | `absX > 60` | `absX > 50` |
| 方向判定 | `absY / absX < 0.577`（30°以内） | 同上 |
| 左边缘过滤 | 避开 `.vcp-scrollable` | 无需过滤 |
| 左滑行为 | 打开右侧边栏 | 关闭左侧边栏 |
| 右滑行为 | 打开左侧边栏 | 执行 `onTabSwitch` |

---

## 3. 标签切换（SidebarTabs.vue）

### 3.1 Agent / Group 标签切换

`SidebarTabs.vue` 是一个极简的受控组件，仅渲染两个按钮，通过 `v-model:activeTab` 与父组件双向绑定。

```vue
<div class="flex p-1 bg-black/5 dark:bg-white/5 rounded-xl mb-4">
  <button @click="emit('update:activeTab', 'agents')">助手</button>
  <button @click="emit('update:activeTab', 'topics')">话题</button>
</div>
```

> 源码位置：`src/features/agent/SidebarTabs.vue` 第 11–27 行

视觉反馈：
- **选中态**：`bg-white text-gray-800 shadow-sm`（亮色） / `bg-white/20 dark:text-white`（暗色）
- **未选中态**：`text-secondary-text hover:text-primary-text`
- **点击反馈**：`active:scale-[0.97]` — 微缩放，符合 VCP Mobile 内敛交互宪法

### 3.2 与 assistantStore 的联动

标签切换本身**不直接操作** `assistantStore`。`AgentSidebar.vue` 仅将 `activeTab` 传递给子组件控制条件渲染：

```vue
<template v-if="activeTab === 'agents'">
  <AgentList :searchQuery="searchQuery" ... />
</template>
<template v-if="activeTab === 'topics'">
  <TopicList ... />
</template>
```

> 源码位置：`src/components/layout/AgentSidebar.vue` 第 65–73 行

`AgentList` 和 `TopicList` 各自从 `assistantStore` 获取数据，标签切换仅控制 DOM 的挂载/卸载。

### 3.3 当前选中状态

`activeTab` 的状态托管在 `AgentSidebar.vue` 内部（`ref<'agents' | 'topics'>('agents')`），未持久化到 Store。这意味着：
- 每次打开侧边栏，默认展示 **Agent 列表**
- 若用户在话题 Tab 关闭侧边栏，下次打开时仍回到 Agent Tab
- 这是一个有意的设计：Agent 列表是更高频的导航入口

---

## 4. 搜索过滤（SidebarSearch.vue）

### 4.1 搜索输入与实现

`SidebarSearch.vue` 采用**极简的 v-model 双向绑定**，未引入 `@vueuse/useDebounce` 等外部防抖逻辑。过滤的实时响应由 Vue 的响应式系统天然保证。

```vue
<input :value="modelValue" 
       @input="emit('update:modelValue', ($event.target as HTMLInputElement).value)"
       :placeholder="placeholderText" />
```

> 源码位置：`src/features/agent/SidebarSearch.vue` 第 25–27 行

搜索框视觉细节：
- 左侧搜索图标（SVG），`group-focus-within:opacity-100` — 聚焦时图标高亮
- `shadow-inner` 内阴影，营造"凹陷输入"的物理感
- `focus:border-blue-500/50` 聚焦边框色
- 动态占位符：`activeTab === 'agents' ? '搜索助手...' : '搜索话题...'`

**为何不使用防抖？**

前端过滤的运算成本极低（`Array.filter` + `String.includes`），且列表长度通常在数十量级，防抖反而会让 UI 响应显得粘滞。VCP Mobile 在此选择**即时过滤**策略。

### 4.2 过滤逻辑（名称匹配）

搜索关键词通过 `AgentSidebar.vue` 的 `searchQuery` 传递给 `AgentList.vue` 的 `searchQuery` prop。`AgentList.vue` 执行三层过滤：

**第一层：`filteredCombinedItems`（combined 级过滤）**

```ts
const filteredCombinedItems = computed(() => {
  const query = props.searchQuery.toLowerCase().trim();
  if (!query) return assistantStore.combinedItems;
  return assistantStore.combinedItems.filter((item) =>
    item.name.toLowerCase().includes(query),
  );
});
```

> 源码位置：`src/features/agent/AgentList.vue` 第 289–295 行

**第二层/第三层：Group 列表与 Agent 列表分别再过滤**

```vue
<div v-for="group in orderedGroups.filter(
  (group) =>
    !searchQuery.trim() ||
    group.name.toLowerCase().includes(searchQuery.toLowerCase().trim()),
)">
```

> 源码位置：`src/features/agent/AgentList.vue` 第 316–322 行（Group 列表）；第 373–378 行（Agent 列表）

过滤规则汇总：

| 维度 | 匹配字段 | 匹配方式 | 大小写敏感 |
|------|---------|---------|-----------|
| Agent | `agent.name` | `includes` | 不敏感（均转小写） |
| Group | `group.name` | `includes` | 不敏感（均转小写） |
| 描述 | — | 不参与过滤 | — |
| 标签 | — | 不参与过滤 | — |
| 模型 | — | 不参与过滤 | — |

> **注意**：当前实现仅匹配 `name` 字段。描述、标签、模型标识不参与过滤，这与传统搜索的全文匹配不同，属于有意的高性能取舍。

### 4.3 空态处理

当过滤后列表为空时，展示极简提示：

```vue
<div v-else-if="filteredCombinedItems.length === 0" class="text-center p-8 opacity-30 text-sm">
  未找到助手或群组
</div>
```

> 源码位置：`src/features/agent/AgentList.vue` 第 307–309 行

空态特征：
- 居中对齐，`opacity-30` 弱化视觉权重
- 无图标、无操作按钮 — 保持信息密度宪法
- 若正在加载（`assistantStore.loading`），优先展示 Loading 动画而非空态

---

## 5. Agent 列表（AgentList.vue）

### 5.1 列表数据源

`AgentList.vue` 的数据来自三个 Store 的聚合：

| Store | 使用的状态 | 用途 |
|-------|-----------|------|
| `assistantStore` | `agents`, `groups`, `combinedItems`, `unreadCounts`, `loading` | 原始数据与加载态 |
| `settingsStore` | `settings.agentOrder`, `settings.groupOrder` | 自定义排序序列 |
| `sessionStore` | `currentSelectedItem` | 当前选中高亮 |

列表并非直接使用 `assistantStore.agents`，而是通过 `computed` 进行**订单化重排**：

```ts
const orderedAgents = computed(() => {
  const agents = assistantStore.agents;
  const order = settingsStore.settings?.agentOrder || [];
  if (order.length === 0) return agents;

  const sorted = [...agents].sort((a, b) => {
    const indexA = order.indexOf(a.id);
    const indexB = order.indexOf(b.id);
    if (indexA === -1 && indexB === -1) return 0;
    if (indexA === -1) return 1;   // 未排序项置后
    if (indexB === -1) return -1;
    return indexA - indexB;
  });
  return sorted;
});
```

> 源码位置：`src/features/agent/AgentList.vue` 第 46–60 行

`orderedGroups` 使用相同算法（第 30–44 行）。排序逻辑特点：
- `order` 数组存储的是 ID 序列，而非完整对象 — 紧凑、可序列化
- 新 Agent（ID 不在 `order` 中）默认排在末尾（`return 1`）
- 排序是**纯函数**，不修改 `assistantStore.agents` 原数组

### 5.2 单条 Agent 卡片结构

Agent 卡片与 Group 卡片结构高度对称，采用**玻璃拟态（Glassmorphism）**面板：

```
┌─────────────────────────────────────────┐
│ ┌─────┐  ┌────────────────────────────┐ │
│ │Avatar│  │ Name (font-bold text-sm)   │ │
│ │ w-10 │  │ Model (text-[10px])        │ │
│ │ h-10 │  └────────────────────────────┘ │
│ └─────┘                                 │
│ ● (未读角标, absolute -top-1 -right-1)  │
└─────────────────────────────────────────┘
```

卡片 DOM 结构要点：

```vue
<div class="relative p-3 glass-panel rounded-xl flex items-center gap-3 
            border shadow-sm cursor-pointer z-10 w-full">
  <!-- 未读角标 -->
  <div v-if="assistantStore.unreadCounts[agent.id] === -1 || 
              assistantStore.unreadCounts[agent.id] > 0"
       class="absolute -top-1 -right-1 w-3 h-3 rounded-full 
              border-2 border-white dark:border-gray-900 z-10 ..."
       style="background: #ff6b6b">
  </div>

  <VcpAvatar owner-type="agent" :owner-id="agent.id" 
             :fallback-name="agent.name" size="w-10 h-10" 
             rounded="rounded-full" class="pointer-events-none" />

  <div class="flex flex-col overflow-hidden flex-1 pointer-events-none">
    <span class="font-bold text-sm truncate text-primary-text">{{ agent.name }}</span>
    <span class="text-[10px] text-secondary-text opacity-80 truncate">{{ agent.model }}</span>
  </div>
</div>
```

> 源码位置：`src/features/agent/AgentList.vue` 第 400–434 行

**设计细节**：
- `pointer-events-none` 在内部子元素上 — 确保触摸事件完整落到卡片根节点，进入 Swipe 手势处理流程
- 角标颜色固定 `#ff6b6b`（珊瑚红），不依赖主题变量 — 保证在任何背景下都具有足够的视觉醒目度
- 角标尺寸 `w-3 h-3`（12px），带 2px 边框与背景色融合，避免遮挡头像边缘

Group 卡片差异点：
- `owner-type="group"`
- 副标题展示成员数与模式：`{{ group.members.length }} Members • {{ group.mode }}`
- 副标题字体更小（`text-[9px]`），且 `uppercase tracking-tighter`

### 5.3 当前选中高亮

选中状态通过 `sessionStore.currentSelectedItem?.id === agent.id` 判定：

```vue
:class="[
  sessionStore.currentSelectedItem?.id === agent.id
    ? 'glass-panel-active'
    : 'border-transparent hover:bg-black/5 dark:hover:bg-white/5',
  ...
]"
```

> 源码位置：`src/features/agent/AgentList.vue` 第 403–406 行

高亮样式：
- **选中**：`glass-panel-active` — 通常表现为边框色变化或背景提亮（定义于 `src/assets/themes.css`）
- **未选中悬停**：`hover:bg-black/5 dark:hover:bg-white/5` — 极淡的悬停反馈
- **点击中**：`active:opacity-75` — 瞬时透明度下降

> **注意**：高亮样式不使用 Accent Bar（2px 侧边条），而是使用整体面板背景变化。这是因为列表项高度较低，侧边条会显得过于拥挤。

### 5.4 未读消息角标

未读数量由 `assistantStore.unreadCounts` 字典维护，该字典通过批量 IPC 调用一次性刷新：

```ts
const refreshUnreadCounts = async () => {
  const counts = await invoke<Record<string, number>>("get_unread_counts");
  unreadCounts.value = counts;
};
```

> 源码位置：`src/core/stores/assistant.ts` 第 65–72 行

角标显示逻辑：
- `unreadCounts[agent.id] === -1`：显示角标（表示存在未读，但具体数量未知或无需精确显示）
- `unreadCounts[agent.id] > 0`：显示角标（有未读消息）
- `unreadCounts[agent.id] === 0` 或 `undefined`：隐藏角标

角标为**纯点状**（无数字），这是移动端高密度列表的常见做法，避免两位数以上数字破坏卡片视觉平衡。

---

## 6. 拖拽排序

### 6.1 SortableJS 集成

`AgentList.vue` 在 `onMounted` 生命周期中为 Group 列表和 Agent 列表分别初始化 `Sortable` 实例：

```ts
import Sortable from "sortablejs";

onMounted(() => {
  initSortable();
});
```

> 源码位置：`src/features/agent/AgentList.vue` 第 138–140 行

`initSortable` 对两个容器分别调用 `Sortable.create`：

| 配置项 | 值 | 说明 |
|--------|-----|------|
| `animation` | 150 | 拖拽过程中的重排动画时长 |
| `handle` | `".drag-handle"` | 仅通过拖拽柄触发排序（整个卡片都是柄） |
| `delay` | 200 | 触摸后延迟 200ms 才启动排序 |
| `delayOnTouchOnly` | `true` | 仅在触摸设备上启用延迟，鼠标操作无延迟 |
| `touchStartThreshold` | 3 | 忽略小于 3px 的微小移动，防止误触 |
| `direction` | `"vertical"` | 垂直排序 |
| `forceFallback` | `true` | 使用自定义拖拽代理（移动端兼容性更好） |
| `fallbackOnBody` | `true` | 拖拽代理挂载到 body |
| `ghostClass` | `"opacity-50"` | 被拖拽元素的半透明样式 |

> 源码位置：`src/features/agent/AgentList.vue` 第 70–136 行

**延迟设计的核心目的**：`delay: 200` + `delayOnTouchOnly: true` 是为了给 Swipe 手势留出判定窗口。如果用户意图是水平滑动展开编辑按钮，手指在 200ms 内移动的水平距离会触发 Swipe 逻辑，而 SortableJS 因延迟未启动，从而避免两种手势的冲突。

### 6.2 排序变更的本地更新

`onEnd` 回调在拖拽释放时触发，计算新顺序并立即更新 Store：

```ts
onEnd: (evt) => {
  isSorting.value = false;
  const newOrder = orderedAgents.value.map((a) => a.id);
  const [movedItem] = newOrder.splice(evt.oldIndex!, 1);
  newOrder.splice(evt.newIndex!, 0, movedItem);
  settingsStore.updateSettings({ agentOrder: newOrder });
}
```

> 源码位置：`src/features/agent/AgentList.vue` 第 127–133 行（Agent 列表）；第 94–100 行（Group 列表）

更新流程：

```
用户释放拖拽
    │
    ▼
┌─────────────────────┐
│ 读取当前 orderedXxx │──> computed 自动反映旧顺序
│ 的 ID 序列          │
└──────────┬──────────┘
           ▼
┌─────────────────────┐
│ 用 splice 调整顺序  │──> oldIndex 移除，newIndex 插入
│ (纯数组操作)         │
└──────────┬──────────┘
           ▼
┌─────────────────────┐
│ settingsStore.      │──> 乐观更新：本地 settings.agentOrder
│ updateSettings({    │    立即变更，列表响应式重排
│   agentOrder: ...   │
│ })                  │
└──────────┬──────────┘
           ▼
┌─────────────────────┐
│ 异步持久化到后端     │──> update_settings Command
│ (updateSettings 内部)│    失败时不回滚（下次启动回读）
└─────────────────────┘
```

### 6.3 排序持久化到后端

`settingsStore.updateSettings` 将 `agentOrder` / `groupOrder` 持久化到 Rust 后端：

```ts
const updateSettings = async (updates: Record<string, any>) => {
  // 合并到本地 settings 对象
  Object.assign(settings.value, updates);
  // 异步保存到后端
  await saveSettings();
};
```

> 源码位置：`src/core/stores/settings.ts` 第 70–85 行

Rust 后端对应的字段：
- `agentOrder: string[]` — Agent ID 数组
- `groupOrder: string[]` — Group ID 数组

持久化策略为**全量覆盖**（`saveSettings` 将整个 settings 对象序列化后写入 SQLite），而非增量 patch。由于 settings 对象体积很小，全量覆盖的开销可忽略。

---

## 7. Swipe 手势

### 7.1 水平滑动检测

`AgentList.vue` 在每个卡片上直接绑定原生 Touch Event，实现卡片级 Swipe：

```ts
const SWIPE_THRESHOLD = 50;   // 展开/折叠的判定阈值
const MAX_SWIPE = 80;         // 完全展开时的最大位移
```

> 源码位置：`src/features/agent/AgentList.vue` 第 146–147 行

手势状态机由以下变量共同维护：

| 变量 | 类型 | 作用 |
|------|------|------|
| `isSorting` | `Ref<boolean>` | 拖拽排序期间禁用所有 Swipe |
| `activeSwipeId` | `Ref<string \| null>` | 当前已展开的卡片 ID，同时只能展开一张 |
| `currentSwipeX` | `Ref<number>` | 当前卡片水平位移（px），驱动 `transform: translateX()` |
| `isDragging` | `Ref<boolean>` | 当前是否有手指正在按压卡片 |
| `isVerticalScroll` | `boolean` | 本次触控是否被判定为垂直滚动 |
| `hasDeterminedDirection` | `boolean` | 是否已完成方向锁定（防止摇摆） |
| `isStartedAsSwiped` | `boolean` | 手指按下时卡片是否已处于展开态 |

### 7.2 编辑/删除操作按钮

Swipe 展开后，卡片向左平移，露出背后的**设置按钮**：

```
正常态:                    展开态:
┌──────────────────┐      ┌────────┬──────────────────┐
│ Avatar  Name     │  →   │ [设置] │ Avatar  Name     │
│        Model     │      │  图标  │        Model     │
└──────────────────┘      └────────┴──────────────────┘
                          ↑ MAX_SWIPE = 80px
```

背景按钮区域：
- **Group**：紫色图标（`text-purple-600/70`），点击打开 `overlayStore.openGroupSettings(id)`
- **Agent**：蓝色图标（`text-blue-600/70`），点击打开 `overlayStore.openAgentSettings(id)`

> 源码位置：`src/features/agent/AgentList.vue` 第 324–340 行（Group 背景按钮）；第 381–397 行（Agent 背景按钮）

按钮点击后执行：
1. 强制折叠 Swipe：`activeSwipeId.value = null; currentSwipeX.value = 0`
2. 关闭侧边栏：`layoutStore.setLeftDrawer(false)`
3. 打开设置页 Overlay：`overlayStore.openAgentSettings(id)` 或 `openGroupSettings(id)`

> **注意**：当前 Swipe 仅暴露"编辑"入口（通过设置页可进一步删除），未直接提供删除按钮。删除操作需通过 Agent/Group 设置面板内的删除按钮完成。

### 7.3 与 v-longpress 的关系

VCP Mobile 同时存在两种快速操作入口：

| 交互方式 | 触发条件 | 操作对象 | 提供的功能 |
|----------|---------|---------|-----------|
| **Swipe（滑动）** | 水平滑动 > 50px | 单条卡片 | 展开编辑按钮 |
| **LongPress（长按）** | 按压 > 500ms | 单条卡片 | 打开 ContextMenu（含编辑、删除等） |

`v-longpress` 是全局自定义指令（定义于 `src/core/directives/vLongpress.ts`），在 `AgentList.vue` 中并未直接使用，但 `App.vue` 或 `ChatView` 中可能通过事件委托为列表项提供长按菜单。

两种机制**功能重叠但互不干扰**：
- Swipe 是物理滑动，适合快速单手操作
- LongPress 是时间触发，适合精确选择多选项菜单
- 它们共享同一触控序列，`touchstart` 的先后顺序决定哪个手势获胜

### 7.4 手势冲突处理（与侧边栏滑出）

Swipe 手势的冲突仲裁是 `AgentList.vue` 中最复杂的逻辑。核心策略通过**状态分流**实现：

**方向锁定（首次移动判定）**：

```ts
if (absY / absX > 0.577) {   // tan(30°) ≈ 0.577
  isVerticalScroll = true;    // 判定为垂直滚动，放弃 Swipe
  isDragging.value = false;
  return;
}
```

> 源码位置：`src/features/agent/AgentList.vue` 第 189–203 行

**折叠态 vs 展开态的分流处理**：

| 初始状态 | 滑动方向 | 行为 | 阻止默认/冒泡？ |
|----------|---------|------|----------------|
| 折叠 | 向左（deltaX < 0） | 不响应卡片 Swipe，允许事件传递 → 触发侧边栏关闭 | 否 |
| 折叠 | 向右（deltaX > 0） | 响应卡片展开，记录 `activeSwipeId` | 是（防滚动震颤） |
| 展开 | 向左 | 卡片阻尼缩回 | 是（防滚动 + 防侧边栏误关闭） |
| 展开 | 向右 | 卡片微阻尼延伸 | 是 |

> 源码位置：`src/features/agent/AgentList.vue` 第 206–241 行

**关键代码注释**：

```ts
// 核心修复：如果是从折叠状态开始滑动，必须强制把 currentSwipeX 重置为 0
// 核心修复：如果是已展开的卡片被二次触摸，我们强制锁定水平手势
// 核心手势状态分流与防穿透逻辑：使用整个手势周期内固定不变的 isStartedAsSwiped 状态
```

`touchend` 的收尾逻辑：

```ts
const shouldKeepOpen = wasSwiped && currentSwipeX.value > SWIPE_THRESHOLD;
if (shouldKeepOpen) {
  currentSwipeX.value = MAX_SWIPE;  // Snap open
  e.stopPropagation();              // 阻止触发侧边栏关闭
} else {
  activeSwipeId.value = null;
  currentSwipeX.value = 0;          // Snap closed
  // 不阻止冒泡，保证能左滑关闭侧边栏
}
```

> 源码位置：`src/features/agent/AgentList.vue` 第 244–259 行

**与排序的互斥**：
- `onChoose`（SortableJS 开始拖拽）时设置 `isSorting = true`，同时强制 `activeSwipeId = null`
- `onTouchStart` 中若 `isSorting.value` 为真，直接返回，不进入 Swipe 逻辑
- `onTouchMove` 中若 `isSorting.value` 为真，调用 `e.preventDefault()` + `e.stopPropagation()`，绝对防止排序期间的手指滑动误触外层

---

## 8. 新建 Agent（AgentsCreator.vue）

### 8.1 触发方式

`AgentsCreator.vue` 渲染两个并列按钮，位于侧边栏底部：

```
┌─────────────────┬─────────────────┐
│  [+] 创建 Agent │  [+] 创建 Group │
│   蓝底/蓝字      │   紫底/紫字      │
└─────────────────┴─────────────────┘
```

> 源码位置：`src/features/agent/AgentsCreator.vue` 第 86–106 行

按钮样式遵循领域色语义：蓝色代表 Agent（个体），紫色代表 Group（集合）。

### 8.2 新建流程

以创建 Agent 为例，流程如下：

```
用户点击 "创建 Agent"
    │
    ▼
┌─────────────────────┐
│ overlayStore.       │──> 打开 Prompt 弹窗
│ openPrompt({        │    标题："创建 Agent"
│   title,            │    占位符："为你的助手起个名字..."
│   placeholder,      │
│   onConfirm         │
│ })                  │
└──────────┬──────────┘
           ▼
┌─────────────────────┐
│ 用户输入名称并确认   │
└──────────┬──────────┘
           ▼
┌─────────────────────┐
│ assistantStore.     │──> 调用 Rust create_agent Command
│ createAgent(name)   │    后端自动生成 ID + 默认话题
└──────────┬──────────┘
           ▼
┌─────────────────────┐
│ assistantStore.     │──> 刷新本地 Agent 列表
│ fetchAgents()       │
└──────────┬──────────┘
           ▼
┌─────────────────────┐
│ sessionStore.       │──> 设置当前选中项
│ currentSelectedItem │    type = 'agent'
└──────────┬──────────┘
           ▼
┌─────────────────────┐
│ topicListStore.     │──> 加载该 Agent 的话题列表
│ loadTopicList(...)  │
└──────────┬──────────┘
           ▼
┌─────────────────────┐
│ layoutStore.        │──> 关闭侧边栏
│ setLeftDrawer(false)│
└──────────┬──────────┘
           ▼
┌─────────────────────┐
│ overlayStore.       │──> 自动打开 Agent 设置面板
│ openAgentSettings   │    引导用户配置模型、提示词等
│ (newAgent.id)       │
└─────────────────────┘
```

> 源码位置：`src/features/agent/AgentsCreator.vue` 第 14–47 行

创建 Group 流程与 Agent 完全对称，仅调用的 Command 和打开的设置面板不同（`create_group` + `openGroupSettings`）。

### 8.3 与 create_agent Command 的集成

`createAgent` 定义于 `assistantStore`：

```ts
const createAgent = async (name: string) => {
  const newAgent = await invoke<AgentConfig>("create_agent", { name });
  notificationStore.addNotification({
    type: "success",
    title: "Agent 创建成功",
    message: `助手 "${name}" 已就绪`,
    toastOnly: true,
  });
  return newAgent;
};
```

> 源码位置：`src/core/stores/assistant.ts` 第 115–133 行

Rust 后端 `create_agent` 的处理（详见 `docs/modules/13_Agent领域总览.md` §3.9）：
- ID 生成：`{sanitized_name}_{timestamp}`
- 自动创建默认话题：`"主要对话"`，`locked = true`
- 默认配置：`temperature = 0.7`，`model = "gemini-2.5-flash"`
- 事务边界：agents 表 + topics 表插入包裹在单一 SQLite 事务中

---

## 9. 数据流时序

### 9.1 打开侧边栏的时序

```
用户从屏幕左边缘向右滑动 (>60px)
    │
    ▼
useSidebarSwipe (type: 'global')
    │
    ▼
layoutStore.setLeftDrawer(true)
    │
    ├──► setRightDrawer(false)      [互斥关闭右侧边栏]
    │
    ├──► leftDrawerOpen.value = true
    │
    └──► registerModal('LeftDrawer', closeCallback)
    │
    ▼
AgentSidebar.vue :class="{ 'is-open': true }"
    │
    ▼
CSS transform: translateX(-100%) → translateX(0)
    │
    ▼
侧边栏完全展开 (400ms)
```

### 9.2 切换 Agent 的时序

```
用户点击 Agent 卡片
    │
    ▼
AgentList.vue: selectAgent(agentId)
    │
    ▼
emit('select-agent', agent)
    │
    ▼
AgentSidebar.vue: handleSelectItem(item)
    │
    ▼
sessionStore.selectItem(item)
    │
    ├──► 若已选中且已有话题 → 提前返回（防重复）
    │
    ├──► 从 lastActiveTopicMap 获取上次话题 ID
    │
    ├──► 若无记录 → invoke('get_topics', { ownerId, ownerType })
    │       │
    │       └──► Rust 查询 topics 表，按 updated_at DESC
    │
    └──► selectTopicById(ownerId, topicId)
            │
            ├──► currentTopicId.value = topicId
            │
            ├──► lastActiveTopicMap[ownerId] = topicId
            │
            ├──► currentSelectedItem.value = { ...agent, type: 'agent' }
            │
            └──► loadHistoryCallback(itemId, ownerType, topicId)
                    │
                    ▼
                ChatView 加载历史消息并渲染
```

> 注：`loadHistoryCallback` 的注入由 `App.vue` 在初始化时完成，解耦了 `chatSessionStore` 与 `chatHistoryStore` 的直接依赖。

### 9.3 拖拽排序的时序

```
用户长按卡片 (>200ms) 并上下拖拽
    │
    ▼
SortableJS onChoose
    │
    ├──► isSorting = true
    ├──► activeSwipeId = null      [强制折叠任何展开卡片]
    └──► currentSwipeX = 0
    │
    ▼
SortableJS onStart
    │
    ▼
用户移动手指，SortableJS 实时更新 ghost 位置
    │
    ▼
用户释放手指
    │
    ▼
SortableJS onEnd
    │
    ├──► isSorting = false
    │
    ├──► 计算 newOrder（splice 调整）
    │
    └──► settingsStore.updateSettings({ agentOrder: newOrder })
            │
            ├──► Object.assign(settings, { agentOrder })
            │       │
            │       └──► orderedAgents computed 自动重排
            │
            └──► 异步 saveSettings() → Rust update_settings
                        │
                        └──► SQLite settings 表全量写入
```

---

## 10. 与 Rust 后端的 IPC 交互

### 10.1 Tauri Commands 调用表

| Command | 调用方 | 输入 | 输出 | 触发场景 |
|---------|--------|------|------|---------|
| `get_agents` | `assistantStore.fetchAgents()` | — | `AgentConfig[]` | 应用启动、Agent 创建/删除/保存后刷新列表 |
| `get_groups` | `assistantStore.fetchGroups()` | — | `GroupConfig[]` | 应用启动、Group 创建/删除/保存后刷新列表 |
| `create_agent` | `assistantStore.createAgent(name)` | `{ name: string }` | `AgentConfig` | AgentsCreator 中点击"创建 Agent" |
| `create_group` | `assistantStore.createGroup(name)` | `{ name: string }` | `GroupConfig` | AgentsCreator 中点击"创建 Group" |
| `delete_agent` | `assistantStore.deleteAgent(id)` | `{ agentId: string }` | `boolean` | Agent 设置面板中执行删除 |
| `delete_group` | `assistantStore.deleteGroup(id)` | `{ groupId: string }` | `boolean` | Group 设置面板中执行删除 |
| `save_agent_config` | `assistantStore.saveAgent(agent)` | `{ agent: AgentConfig }` | `boolean` | Agent 设置面板保存配置 |
| `save_group_config` | `assistantStore.saveGroup(group)` | `{ group: GroupConfig }` | `boolean` | Group 设置面板保存配置 |
| `save_avatar_data` | `assistantStore.saveAvatar(...)` | `{ ownerType, ownerId, mimeType, imageData }` | `string` (hash) | 头像裁剪后上传二进制数据 |
| `get_avatar` | `VcpAvatar.vue` 内部 | `{ ownerType, ownerId }` | `AvatarResult \| null` | 头像组件挂载时加载头像 |
| `get_unread_counts` | `assistantStore.refreshUnreadCounts()` | — | `Record<string, number>` | fetchAgents / fetchGroups 后批量刷新未读 |
| `get_topics` | `sessionStore.selectItem()` | `{ ownerId, ownerType }` | `Topic[]` | 切换 Agent/Group 时获取话题列表 |
| `update_settings` | `settingsStore.saveSettings()` (内部) | `Settings` 对象 | `boolean` | 拖拽排序后持久化顺序、任何设置变更 |

### 10.2 事件监听表

本模块涉及的 Rust → 前端事件推送较少，主要为同步相关事件：

| 事件名 | 来源 | 前端监听方 | 说明 |
|--------|------|-----------|------|
| `sync-completed` | `sync_service.rs` | `main.ts`（全局） | 同步完成后触发 `window.location.reload()`，间接刷新 Agent 列表 |

> Agent 侧边栏本身未注册独立的 Tauri `listen` 监听器。所有数据更新均通过 Command 调用的返回值 + Pinia Store 的响应式更新驱动视图。

---

## 11. 设计决策与注意事项

### 11.1 为何 Swipe 只暴露"编辑"而非"删除"？

删除 Agent/Group 是不可逆操作（虽然 Rust 侧为软删除，但前端无回收站功能）。Swipe 手势的触发门槛较低，容易因误触导致数据丢失。将删除操作隐藏在设置面板内，增加了操作路径长度，符合**破坏性操作需要刻意性**的安全设计原则。

### 11.2 `filteredCombinedItems` 与列表级过滤的冗余问题

`AgentList.vue` 中同时存在 `filteredCombinedItems`（用于空态判定）和 `orderedXxx.filter(...)`（用于实际渲染）。两者过滤逻辑相同但执行两次，存在轻微冗余。保留这种结构的原因是：
- `filteredCombinedItems` 服务于"无结果"的顶层空态（覆盖 Group + Agent 整体）
- 列表级过滤服务于各分组的独立渲染（未来可能支持分组独立空态）
- 数据量极小（通常 < 100 条），重复过滤的性能损耗可忽略

### 11.3 搜索为何不支持描述/标签/模型过滤？

当前过滤仅限 `name` 字段，原因有三：
1. **性能**：Agent 列表数据常驻内存，但名称是最稳定的记忆锚点；用户通常记得"助手的名字"而非"助手的模型"
2. **UI 密度**：如果搜索匹配模型标识（如 `gemini-2.5-flash`），结果相关性低，且副标题已展示模型信息，视觉冗余
3. **极简哲学**：VCP Mobile 的交互宪法强调"高密度线性布局"与"技术精确感"，不过度扩展搜索维度

若未来需要扩展，可在 `filteredCombinedItems` 的 `filter` 回调中追加字段，改动量极小。

### 11.4 排序持久化为何放在 settings 而非 agent 配置？

`agentOrder` 与 `groupOrder` 存储在 `settingsStore` 中，而非 `AgentConfig` 的字段。这是关键的设计分界：
- `AgentConfig` 描述的是**Agent 本身的属性**（模型、提示词等），与设备无关
- `agentOrder` 描述的是**用户在该设备上的视图偏好**，属于设备本地状态
- 若将顺序存入 `AgentConfig`，同步后会覆盖其他设备的本地排序偏好，破坏多端体验的独立性

### 11.5 未读角标为何不显示具体数字？

`unreadCounts` 后端返回的是精确数字（`-1` 表示"有未读但数量未知"，正数表示具体数量），但前端渲染为纯点状角标：
- 移动端侧边栏宽度有限（82vw / max 340px），数字角标会挤压名称区域
- `-1` 与正数的语义在 UI 上统一为"有未读"，降低认知负担
- 若需精确数字，可进入话题列表查看各话题的未读统计

---

## 12. 术语速查表

| 术语 | 英文/缩写 | 定义 | 相关模块 |
|------|----------|------|----------|
| AgentSidebar | — | 左侧抽屉式导航面板，承载 Agent/Topic 列表、搜索、新建入口 | `AgentSidebar.vue` |
| leftDrawerOpen | — | `layoutStore` 中控制侧边栏可见性的布尔状态 | `layoutStore.ts` |
| combinedItems | — | `assistantStore` 中将 `agents` 和 `groups` 合并并附加 `type` 字段的计算属性 | `assistantStore.ts` |
| orderedAgents / orderedGroups | — | 基于 `settings.agentOrder` / `groupOrder` 重排后的计算属性 | `AgentList.vue` |
| agentOrder / groupOrder | — | 存储在 settings 中的 ID 数组，定义列表的自定义显示顺序 | `settingsStore.ts` |
| unreadCounts | — | 字典结构 `Record<string, number>`，记录每个 Agent/Group 的未读消息数 | `assistantStore.ts` |
| currentSelectedItem | — | `sessionStore` 中当前选中的 Agent 或 Group 对象 | `chatSessionStore.ts` |
| activeSwipeId | — | `AgentList.vue` 中当前 Swipe 展开的卡片 ID，同时只能展开一个 | `AgentList.vue` |
| SWIPE_THRESHOLD | — | Swipe 展开判定阈值，50px | `AgentList.vue` |
| MAX_SWIPE | — | Swipe 完全展开时的最大位移，80px | `AgentList.vue` |
| SortableJS | — | 第三方拖拽排序库，用于 Agent/Group 列表的拖拽重排 | `sortablejs` (npm) |
| drag-handle | — | SortableJS 的拖拽柄 CSS 类，本模块中整个卡片均可作为柄 | `AgentList.vue` |
| useSidebarSwipe | — | 全局组合式函数，统一处理侧边栏边缘滑动手势 | `useSidebarSwipe.ts` |
| global / left 模式 | — | `useSidebarSwipe` 的两种工作模式：`global` 从屏幕边缘打开侧边栏；`left` 在侧边栏内滑动关闭或切 Tab | `useSidebarSwipe.ts` |
| glass-panel | — | UnoCSS 快捷类，玻璃拟态面板样式（半透明背景 +  backdrop-blur） | `uno.config.ts` |
| glass-panel-active | — | 选中态的玻璃面板变体，通常表现为边框色或背景提亮 | `src/assets/themes.css` |
| Modal History | — | `useModalHistory` 提供的模态栈管理，使系统返回键可按层级关闭覆盖层 | `useModalHistory.ts` |
| ToastOnly | — | 通知配置项，仅显示 Toast 浮层，不写入通知中心历史 | `notificationStore.ts` |

---
*最后更新：2026-05-27 | VCP Mobile v0.9.14*
