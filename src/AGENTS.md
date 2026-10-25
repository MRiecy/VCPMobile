# AGENTS.md — VCP Mobile Vue 渲染层

> 本文件适用于 `src/**`。它继承仓库根规则，只补充 Vue 应用、前端状态、覆盖层、消息渲染与移动交互边界；当前行为必须从本目录生产代码及其实际 Rust 调用方恢复。

---

## 1. 定位与数据流

`src/` 是 Project Avatar 的 Vue 渲染层。它负责将 Tauri IPC 返回值、异步事件与流式消息归约为响应式状态，在统一组件树中呈现移动端界面，并将用户交互转换为 Store 变更或 IPC 调用。

```text
Tauri Result / Event ──> App / Store / Composable ──> Feature / Component ──> UI
                              ^                                  |
                              └────── invoke / 用户意图 ─────────┘

Rust AST / TailFrame ──> MessageRenderer ──> AstExecutor / DOM Engine ──> Rich Blocks
```

---

## 2. 代码地图

| 路径 | 唯一职责 |
| ---- | -------- |
| `main.ts` | 主应用装配：Vue 实例、Pinia 状态池、Router 路由、全局指令与错误捕获 |
| `App.vue` | 根级挂载面：进程级窗口生命周期、安全区域 (Insets)、系统事件与主工作区 |
| `core/stores/` | 跨 Feature 全局状态（智能体、设置快照、覆盖层栈、分享与会话） |
| `core/composables/` | 浏览器能力、Tauri 事件监听与移动端交互生命周期桥接 |
| `core/dom-engine/` | 流式富文本与 AST 消息片段的局部高性能 DOM 渲染执行器 |
| `core/constants/` | 全局常量定义与语义化层级系统 (`layers.ts`) |
| `features/` | 领域业务模块（chat、agent、topic、settings、sync、rag 等），包含领域专属 Store 与交互 |
| `components/` | 应用基础外壳、设置原子控件（SettingsCanvas）与通用 UI 原语 |
| `assets/` | 主题定义 (`themes.css`)、全局重置与富文本消息样式 |

---

## 3. 核心不变量

| # | 不变量 / 约束 | 锚点 | 破坏后果 |
| - | ------------- | ---- | -------- |
| F1 | 跨 Feature 共享状态归 `core/stores/`，领域专属状态封闭在 `features/<domain>/` 内，禁止跨领域私自持有状态镜像 | `core/stores/` | 状态生命周期分叉，引发更新丢失或双重渲染 |
| F2 | 虚拟多页与抽屉导航必须由 `overlayStore` 的页面栈统一调度，禁止在 Feature 内部复制独立的全局全屏路由栈 | `core/stores/overlayStore.ts` | 移动端物理返回键拦截失序，导致回退死循环或意外退出应用 |
| F3 | 全局弹层（Prompt、Confirm、Toast、Viewer）必须经由 `GlobalOverlayManager` 呈现并消费语义层级，严禁手写硬编码 Z-Index | `core/constants/layers.ts` | 弹窗互相遮挡、模态被底层内容穿透或点击事件穿透 |
| F4 | 消息流式渲染必须基于 AST 快照与 TailFrame 增量驱动，稳定块与尾部渲染分离，禁止以全量重渲染替代局部计算 | `features/chat/composables/useMessageTailRenderer.ts` | 导致超长对话流式打字卡顿、打字机跳动或滚动丢失 |
| F5 | 消息上下文菜单由根级 `v-longpress.message` 代理，内部需保留原生选区或点击的控件须显式标记 `data-longpress-native` | `core/directives/` | 长按消息复制与局部富文本选中、表单操作相互冲突 |
| F6 | 设置页编辑必须通过 `useSettingsPageSession` 维持生命周期，修改提交携基准版本原子比对，离开页面必须释放会话 | `features/settings/` | 迟到配置覆盖最新修改，或未保存草稿泄漏至后续页面 |
| F7 | 通话与朗读权威状态归 Rust，前端 Store 仅提交意图并消费快照，后台切换与组件销毁不自动篡改业务终态 | `core/stores/` | 产生幽灵语音任务、音频播放失控或前台错位重连 |
| F8 | 严禁在滚动容器、列表卡片或气泡内容中使用 `backdrop-filter: blur()`，仅允许在 fixed/sticky 导航条极小面积评估保留 | `assets/themes.css` | 移动端 GPU 合成层爆炸，导致低端 Android 设备列表滑动掉帧卡顿 |
| F9 | 手机、平板与折叠屏必须共享同一套组件树与业务 Store，仅通过当前窗口宽度做响应式断点适配，禁止分支维护 | `src/App.vue` | 状态逻辑多处分叉，多端表现无法对齐且增加维护成本 |

---

## 4. 覆盖层与导航规范

- **虚拟导航**: 主应用基于单页架构，次级业务视窗通过 `SlidePage` 压栈管理，返回顺序与 Z-Index 由 `overlayStore.getPageZIndex` 动态关联。
- **全局覆盖**: 全局弹窗与抽屉使用统一遮罩管理，同一时刻只激活最高优先级交互面，避免多模态并发阻断用户输入。

---

## 5. 修改与验证

| 修改范围 | 验证命令 |
| -------- | -------- |
| 前端代码、组件与 Store 逻辑 | 在仓库根目录运行 `pnpm lint` 与 `pnpm vue-tsc --noEmit` |
| 跨端 IPC 调用与配置类型一致性 | 在仓库根目录运行 `pnpm check` |
| 前端单元与状态集成测试 | 在仓库根目录运行 `pnpm test:run` |

**完成判定**：交互闭环由对应 Store 或 Composable 接管，语义层级无冲突，组件销毁时事件监听与定时器完全释放，静态检查零报错。
