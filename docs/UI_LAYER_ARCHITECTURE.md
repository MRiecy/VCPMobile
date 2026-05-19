# UI 层级架构规范

> 文档类型：工程规范  
> 版本：v1.0  
> 日期：2026-05-18  
> 关联变更：commit `f7dbd21` 之后的一组重构提交

---

## 1. 背景与动机

随着页面功能区分越来越成熟，项目中的覆盖层、抽屉、弹窗、提示等 UI 元素数量激增。此前，这些元素的层级管理处于完全失控状态：

- **魔法数字满天飞**：`z-50`、`z-[60]`、`z-[999]`、`z-[1000]`、`z-[10000]` 等硬编码散布在 20+ 个文件中。
- **三种策略混用**：有的靠 DOM 顺序分层，有的写死 z-index，有的用 `z999` 粗暴解决问题。
- **架构缺陷**：`GlobalOverlayManager`（`fixed` + `z-[60]`）创建了独立层叠上下文，其内部的 `ContextMenuSheet`（`fixed` + `z-[100]`）未使用 Teleport，导致 `z-[100]` 被父容器吞没，实际等效于 `z-60`。
- **幽灵耦合**：`SlidePage` 默认 50、`overlayStore.getPageZIndex` 基准 50、`ChatView` 置底按钮 50，三者语义完全不同却共享同一个数字。

本规范旨在建立一套**语义化、可维护、可扩展**的层级管理系统。

---

## 2. 层级体系总表

项目采用 **11 级分层架构**，每层间隔 10，预留插入空间。

| 层级 | 语义名 | 数值 | 用途 | 代表性组件 |
|------|--------|------|------|-----------|
| L0 | `content` | 0 | 页面内容默认层 | 消息气泡、列表项 |
| L1 | `local` | 10 | 页面内局部悬浮元素 | 置底按钮、StreamingTag、角标、hover 覆盖 |
| L2 | `drawer` | 20 | 侧边栏抽屉 + 遮罩 | `AgentSidebar`, `RightSidebar`, `App.vue` 遮罩 |
| L3 | `overlay` | 30 | 全局覆盖容器 | `GlobalOverlayManager` |
| L4 | `page` | 40–49 | 页面栈（SlidePage） | `SettingsView`, `AgentSettingsView`, `SyncSessionView`… |
| L5 | `sheet` | 50 | 底部弹层 | `BottomSheet`, `ModelSelector` |
| L6 | `dialog` | 60 | 对话框 / 提示 | `VcpPrompt`, `UpdatePrompt`, `ContextMenu` |
| L7 | `viewer` | 70 | 全屏查看器 / 编辑器 | `AttachmentViewer`, `FullScreenEditor`, `AvatarCropper` |
| L8 | `editor` | 80 | 最高级全屏编辑器 | `HtmlPreviewBlock` (fullscreen) |
| L9 | `toast` | 90 | Toast 通知 | `ToastManager` |
| L10 | `boot` | 100 | 启动屏 | `BootScreen` |
| L11 | `gate` | 110 | 权限门禁页 | `PermissionGate` |

**层叠秩序口诀**：

```
内容 < 局部 < 抽屉 < 覆盖 < 页面 < 弹层 < 对话框 < 查看器 < 编辑器 < Toast < 启动 < 门禁
```

---

## 3. 三层保障机制

层级规范通过 **CSS 变量 + UnoCSS Theme + TypeScript 常量** 三重机制落地，确保模板、样式、逻辑代码都能引用同一套语义。

### 3.1 CSS 变量层

定义于 `src/assets/themes.css` 的 `:root` 中：

```css
:root {
  --layer-content: 0;
  --layer-local: 10;
  --layer-drawer: 20;
  --layer-overlay: 30;
  --layer-page: 40;
  --layer-sheet: 50;
  --layer-dialog: 60;
  --layer-viewer: 70;
  --layer-editor: 80;
  --layer-toast: 90;
  --layer-boot: 100;
  --layer-gate: 110;
}
```

适用场景：`<style scoped>` 块中需要设置 z-index 时。

### 3.2 UnoCSS Theme 层

定义于 `uno.config.ts` 的 `theme.zIndex` 中：

```ts
theme: {
  zIndex: {
    content: '0',
    local: '10',
    drawer: '20',
    overlay: '30',
    page: '40',
    sheet: '50',
    dialog: '60',
    viewer: '70',
    editor: '80',
    toast: '90',
    boot: '100',
    gate: '110',
    },
    },
    }


适用场景：Vue Template 的 `class` 属性中直接使用，如 `class="fixed inset-0 z-dialog"`。

### 3.3 TypeScript 常量层

定义于 `src/core/constants/layers.ts`：

```ts
export const LAYER_CONTENT = 0;
export const LAYER_LOCAL = 10;
export const LAYER_DRAWER = 20;
export const LAYER_OVERLAY = 30;
export const LAYER_PAGE_BASE = 40;
export const LAYER_SHEET = 50;
export const LAYER_DIALOG = 60;
export const LAYER_VIEWER = 70;
export const LAYER_EDITOR = 80;
export const LAYER_TOAST = 90;
export const LAYER_BOOT = 100;
export const LAYER_GATE = 110;

export const getPageZIndex = (stackIndex: number): number => {
  const offset = Math.min(stackIndex, 9); // 封顶到 49
  return LAYER_PAGE_BASE + offset;
};
```

适用场景：运行时动态计算，如 `overlayStore.getPageZIndex()`。

---

## 4. 使用方式速查

### 4.1 Vue Template（推荐）

```vue
<!-- 固定定位的全屏对话框 -->
<div class="fixed inset-0 z-dialog bg-black/40">
  ...
</div>

<!-- 底部弹层（遮罩和内容同层级，靠 DOM 顺序覆盖） -->
<div class="fixed inset-0 bg-black/50 z-sheet"></div>
<div class="fixed bottom-0 left-0 right-0 z-sheet bg-white"></div>
```

### 4.2 `<style scoped>`

```css
.my-drawer {
  position: absolute;
  z-index: var(--layer-drawer);
}
```

### 4.3 TypeScript / Pinia Store

```ts
import { LAYER_PAGE_BASE, getPageZIndex } from '@/core/constants/layers';

// 静态引用
const baseZ = LAYER_PAGE_BASE; // 40

// 动态计算（页面栈）
const zIndex = getPageZIndex(stackIndex); // 40 + min(index, 9)
```

---

## 5. 设计原则

1. **全局宏观层级必须使用语义化命名**。禁止在任何全局覆盖层组件中直接使用裸露的 `z-50`、`z-[999]` 等魔法数字。
2. **局部微观层级保持自由**。组件内部的角标、hover 覆盖、加载状态等（如 `z-10`、`z-20`）属于组件内部层叠上下文，不影响全局秩序，可继续使用常规数值。
3. **新增覆盖层前先查表**。若无法归入已有 11 个层级，再提议新增。每层之间预留 10 的间隔，供未来插入。
4. **SlidePage 页面栈使用动态计算**。通过 `overlayStore.getPageZIndex(type)` 确保页面打开顺序与层级正相关。
5. **BootScreen 的 error 层是唯一例外**。允许使用 `z-[101]`（boot + 1），因为它只在 BootScreen 内部与 loading 层区分。

---

## 6. 重构修改清单

### 6.1 新建文件

| 文件 | 说明 |
|------|------|
| `src/core/constants/layers.ts` | 层级常量、类型、`getPageZIndex` 计算函数 |

### 6.2 配置文件

| 文件 | 修改内容 |
|------|----------|
| `uno.config.ts` | `theme.zIndex` 扩展 11 个语义层级 |
| `src/assets/themes.css` | `:root` 注入 `--layer-*` CSS 变量 |

### 6.3 Store & 基础组件

| 文件 | 修改内容 |
|------|----------|
| `src/core/stores/overlay.ts` | `getPageZIndex` 改用 `LAYER_PAGE_BASE` 常量 |
| `src/components/ui/SlidePage.vue` | 默认 `zIndex` prop 从 `50` 改为 `LAYER_PAGE_BASE` (40) |

### 6.4 全局覆盖层（高 → 低）

| 文件 | 原值 | 新值 | 备注 |
|------|------|------|------|
| `BootScreen.vue` | `z-[1000]` / `z-[1001]` | `z-boot` / `z-[101]` | 启动屏最高 |
| `PermissionGate.vue` | 新增 | `z-gate` | 物理权限门禁 |
| `HtmlPreviewBlock.vue` | `z-[10000]` | `z-editor` | 全屏HTML编辑器 |
| `FullScreenEditor.vue` | `z-[2000]` | `z-viewer` | 全屏文本编辑器 |
| `AvatarCropper.vue` | `z-[2000]` | `z-viewer` | 头像裁剪 |
| `AttachmentViewer.vue` | `z-[1000]` | `z-viewer` | 附件查看 |
| `BottomSheet.vue` | `z-[999]` / `z-[1000]` | `z-sheet` | 遮罩和内容同层级 |
| `ModelSelector.vue` | `z-[999]` / `z-[1000]` | `z-sheet` | 模型选择器 |
| `VcpPrompt.vue` | `z-[300]` | `z-dialog` | 输入提示框 |
| `UpdatePrompt.vue` | `z-[300]` | `z-dialog` | 更新提示 |
| `ToastManager.vue` | `z-[200]` | `z-toast` | Toast 通知 |
| `ContextMenuSheet.vue` | `z-[100]` (inline) | `z-dialog` + **Teleport** | 关键修复：脱离父层叠上下文 |
| `GlobalOverlayManager.vue` | `z-[60]` | `z-overlay` | 全局覆盖容器 |

### 6.5 布局层与内容层

| 文件 | 原值 | 新值 |
|------|------|------|
| `AgentSidebar.vue` | `z-index: 60` (移动端) | `var(--layer-drawer)` |
| `AgentSidebar.vue` | `z-index: 10` (桌面端) | `var(--layer-local)` |
| `RightSidebar.vue` | `z-index: 60` | `var(--layer-drawer)` |
| `App.vue` (遮罩) | 无 z-index | `z-drawer` |
| `ChatView.vue` (置底按钮) | `z-50` | `z-local` |
| `GroupStopAllButton.vue` | `z-50` | `z-local` |
| `ToolInteractionOverlay.vue` | `z-50` | `z-overlay` |

### 6.6 Feature 页面（SlidePage 使用者）

| 文件 | 修改内容 |
|------|----------|
| `FeatureOverlays.vue` | `SyncSessionView` / `RebuildSessionView` 传入 `:z-index` prop |
| `SyncSessionView.vue` | 硬编码 `:z-index="100"` → 接收 `zIndex` prop 传入 SlidePage |
| `RebuildSessionView.vue` | 同上 |

### 6.7 保持不变（局部微观层级）

以下组件的 `z-10` / `z-20` / `z-index: 0/1/2` 属于组件内部微观层级，处于独立的层叠上下文中，不影响全局秩序：

- `TopicList.vue`（角标）
- `AgentList.vue`（卡片、角标、背景层）
- `ThemePicker.vue`（选中标记）
- `AttachmentPreviewBase.vue`（删除按钮、加载遮罩）
- `StagedAttachmentPreview.vue`（加载遮罩）
- `AttachmentPreview.vue`（hover 按钮）
- `SettingsView.vue` / `AboutSection.vue` / `UserProfileSection.vue`（各类局部叠加）
- `ChatBubble.vue`（伪元素阴影、流式边框）
- `ToolBlock.vue`（装饰边框、内容层）
- `NotificationStatusBar.vue`（状态栏）
- `message-blocks.css`（Diary 表情 `z-index: 2`）

---

## 7. 关键架构修复

### 7.1 ContextMenuSheet 层级失效

**问题**：`ContextMenuSheet` 直接挂载在 `GlobalOverlayManager`（`fixed` + `z-[60]`）内部，且未使用 Teleport。由于父元素创建了新的层叠上下文，`ContextMenuSheet` 的 `z-[100]` 实际等效于 `z-60`，无法压过其他 `z-[60]` 以上的元素。

**修复**：为 `ContextMenuSheet` 添加 `<Teleport to="body">`，使其直接挂载到 body，脱离父层叠上下文限制，其 `z-dialog(60)` 在视口层面真正生效。

### 7.2 页面栈幽灵耦合

**问题**：`SlidePage` 默认 50、`overlayStore.getPageZIndex` 基准 50、`ChatView` 置底按钮 50 三者语义不同却共享同一数值。

**修复**：
- `SlidePage` 默认 → `LAYER_PAGE_BASE` (40)
- `overlayStore.getPageZIndex` 基准 → `LAYER_PAGE_BASE` (40)
- `ChatView` 置底按钮 → `LAYER_LOCAL` (10)

### 7.3 z999/z10000 粗暴策略

**问题**：`BottomSheet`、`ModelSelector` 使用 `z-[999]`/`z-[1000]`，`HtmlPreviewBlock` 使用 `z-[10000]`。这不是"足够高"，而是"放弃思考"。

**修复**：全部归入语义化层级。`HtmlPreviewBlock` 从 10000 降至 `editor(90)`，由于 `BootScreen` 只在启动时存在，两者不会同时出现，90 已足够保证其最高优先级。

---

## 8. 动态层级：页面栈

`overlayStore` 维护一个 `pageStack` 数组，记录当前打开的 SlidePage 页面序列。

```ts
// overlay.ts
const getPageZIndex = (type: string) => {
  const index = pageStack.value.findIndex(p => p.type === type);
  if (index === -1) return LAYER_PAGE_BASE;
  return LAYER_PAGE_BASE + Math.min(index, LAYER_PAGE_MAX_OFFSET);
};
```

**行为**：
- 页面栈中的第一个页面 → z-index 40
- 第二个 → 41
- ...
- 第十个及以上 → 49（封顶）

**封顶原因**：Toast(50) 以上有独立的提示/弹层系统，页面栈不应无限侵占上层空间。

---

## 9. 后续维护指南

### 9.1 新增覆盖层组件

1. 查看本规范第 2 节的层级总表，判断新组件应归入哪个层级。
2. 在 Vue Template 中使用语义类名：`z-{layerName}`。
3. 若需要在 `<style>` 中使用，引用 `var(--layer-{name})`。
4. 若需要在 TypeScript 中动态计算，引用 `LAYER_{NAME}` 常量。

### 9.2 新增层级（极少发生）

若现有 11 个层级无法容纳新的 UI 类型：

1. 在 `layers.ts` 中新增常量（如 `LAYER_GUIDE = 55`）。
2. 在 `uno.config.ts` 的 `theme.zIndex` 中同步添加。
3. 在 `themes.css` 的 `:root` 中同步添加 CSS 变量。
4. 更新本规范文档第 2 节的层级总表。
5. 运行 `pnpm check` 验证。

### 9.3 禁止事项

- ❌ 禁止在全局覆盖层组件中使用 `z-50`、`z-[999]`、`z-[10000]` 等裸露数字。
- ❌ 禁止将 `SlidePage`、`BottomSheet`、`Prompt` 等组件的 z-index 写死在组件内部（应通过 prop 或 store 动态传入）。
- ❌ 禁止在 `GlobalOverlayManager` 内部直接渲染 `fixed` 定位组件而不使用 `Teleport`（会受父层叠上下文限制）。

---

## 10. 相关文件索引

| 文件 | 角色 |
|------|------|
| `src/core/constants/layers.ts` | 层级常量和计算函数（单一事实来源） |
| `uno.config.ts` | UnoCSS Theme 扩展 |
| `src/assets/themes.css` | CSS 变量定义 |
| `src/core/stores/overlay.ts` | 页面栈动态层级计算 |
| `AGENTS.md` §6 | 代理编码规范中的层级速查 |
