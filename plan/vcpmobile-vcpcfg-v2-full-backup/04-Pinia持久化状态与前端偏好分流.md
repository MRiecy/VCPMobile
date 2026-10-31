# 04 - Pinia 持久化状态与前端偏好分流

## 1. 全端 Store 与 LocalStorage 物理盘点

通过对 `src/` 前端代码库的全面静态扫描与调用链追踪，现对全部 Store 的状态持久化机制进行全量审计：

### 1.1 Pinia 插件持久化 Store (`pinia-plugin-persistedstate`)
在 `src/main.ts` 中注册了 `pinia.use(piniaPluginPersistedstate)`。使用 `persist` 配置的 Store 如下：

| Store 名称 | 物理文件 | 持久化 pick 字段 | 存储 key | 字段语义与数据特征 | 评估结论 |
|---|---|---|---|---|---|
| `topic` (`useTopicStore`) | `src/core/stores/topicListManager.ts` | `["sortMode", "pinnedTopicKeys"]` | `topic` | `sortMode`: 话题排序模式（`created` / `updated`）；`pinnedTopicKeys`: 字符串数组，格式为 JSON 序列化三元组 `JSON.stringify([ownerType, ownerId, topicId])` | ✅ **必须备份**：`sortMode` 归全局偏好；`pinnedTopicKeys` 随实体恢复自动挂钩生效 |
| `chatSession` (`useChatSessionStore`) | `src/core/stores/chatSessionStore.ts` | `["currentSelectedItem", "currentTopicId", "lastActiveTopicMap"]` | `chatSession` | 记录关闭前最后浏览的会话和话题，用于冷启恢复现场 | ❌ **严禁备份**：属于易失会话状态。若在空机上恢复，会指向不存在的幽灵实体，导致路由与页面崩溃 |
| `diary` (`useDiaryStore`) | `src/features/diary/diaryStore.ts` | `["hiddenFolders", "folderOrder", "collapsedCategories"]` | `diary` | 离线日记中心的文件夹隐藏列表、自定义排位顺序、折叠分类 | ✅ **必须备份**：纯用户排版偏好，无外键破坏性，随备份还原极大提升体验 |
| `guide` (`useGuideStore`) | `src/features/guide/stores/guideStore.ts` | `["completed", "lastSeenAppVersion"]` | `guide` | 已完成的引导教程 ID 集合；最后查看的版本号 | ❌ **严禁备份**：空机或换机后，用户**必须**重新看到新手引导和版本变更提示。若导入已完成标记，会导致新机跳过所有关键引导！ |

### 1.2 手工使用 `localStorage` / `readLocalPreference` 的 Store 与模块

| 所属模块 | 物理文件 | LocalStorage Key | 字段语义与格式 | 评估结论 |
|---|---|---|---|---|
| `theme` (`useThemeStore`) | `src/core/stores/theme.ts` | `vcp-theme-name`<br>`vcp-theme-mode` | 主题 TS 模块文件名（如 `themes-star-abyss.ts`）；<br>深浅模式（`light` / `dark` / `system`） | ✅ **已有并继续备份**（归 `preferences.theme`） |
| `chat` (`useChatPreferencesStore`)| `src/features/chat/preferences/store.ts` | `vcp-chat-presentation-mode`<br>`vcp-smooth-streaming-enabled`<br>`vcp-message-haptics-enabled`<br>`vcp-collapse-tools-while-streaming` | 消息呈现（`bubble` / `panel` / `magazine`）；<br>平滑流式动效开关；<br>消息震动开关；<br>流式折叠工具调用开关 | ✅ **已有并继续备份**（归 `preferences.chat`） |
| `update` (`useUpdateStore`) | `src/core/stores/update.ts`<br>`src/core/composables/useAutoUpdate.ts`| `vcp_auto_update_check`<br>`vcp_last_update_check`<br>`vcp_update_skipped_version` | 启动时自动检查更新开关（`on`/`off`）；<br>上次成功检查毫秒时间戳；<br>跳过的构建版本号 | ✅ **部分备份**：仅将 `autoCheckUpdate` 开关纳入备份；时间戳和跳过版本属于设备运行时，排除在外 |
| `developer` (`useDeveloperStore`) | `src/features/settings/developer/store.ts` | `vcp-developer-mode` | 开发者模式开关 | ❌ **严禁备份**：源码明确约束 `never part of config backups`，属单机诊断黑名单 |
| `appLifecycle` | `src/core/stores/appLifecycle.ts` | `vcp_notification_intro_v1` | 系统通知权限引导已读状态（`'seen'`） | ❌ **严禁备份**：属于单机系统权限引导标志，新装机需重新请求通知权限 |
| `logCenter` | `src/tests/unit/logcenter/logCenter.test.ts` | `vcp_log_limit`<br>`vcp_log_reverse`<br>`vcp_log_autoscroll` | 日志中心单页容量、倒序排列、自动滚底 | ❌ **无需备份**：低频调试工具的临时 UI 状态 |

---

## 2. 状态分流与四级分类架构

为了终结以往“什么都想存、存了又报空指针”的混乱状态，确立严格的四级状态分流准则：

```
                             全部前端持久状态
                                    │
       ┌──────────────────┬─────────┴─────────┬──────────────────┐
       ▼                  ▼                   ▼                  ▼
【第一级：通用偏好】  【第二级：实体偏好】 【第三级：独立数据】  【第四级：易失黑名单】
 • 主题/深浅模式     • 置顶话题键        • 收藏模型列表       • 当前选中的话题
 • 聊天排版与动效     • 侧栏助手排序      • Tarven 规则库     • 已完成引导教程
 • 话题排序模式       • 侧栏群组排序                          • 自动更新检查时间
 • 日记目录折叠       (延迟或随实体生效)  (纯离线业务表)       • 通知权限申请标记
 (直接无脑还原)                                               (坚决物理剔除)
```

### 2.1 第一级：通用客户端偏好 (Portable Client Preferences)
- **特征**：纯 UI/交互偏好，不依赖任何特定实体 ID，在任何设备上导入均有意义。
- **存储归属**：`.vcpcfg.json` 中的 `preferences` 根节点。
- **还原方式**：导入时直接通过对应 Store 的 Action / Normalizer 写入，若格式非法自动降级至默认值。

### 2.2 第二级：实体关联偏好 (Scoped Entity References)
- **特征**：包含了具体实体 ID（如 `pinnedTopicKeys`、`agentOrder`、`groupOrder`）。
- **特殊处理**：
  - 空机导入时，这些 ID 在本地数据库中尚无对应实体，**但写入 Pinia 或 `settings.navigation` 不会引发任何系统异常**；
  - 物理代码中的 `sortByOrder(items, order)` 具有极佳的容错设计（见 `src/features/agent/agentOrder.ts`）：
    - 未知 ID 置后，找不到的 ID 自动忽略；
  - 因此，此类偏好可直接写入前端持久层！当后续通过 CDS 同步将这些实体同步回手机后，界面在渲染列表时便能立即按照此前的排序与置顶规则自动显示，**甚至无需用户再次导入！**

### 2.3 第三级：独立离线业务数据 (Standalone Local Tables)
- **特征**：属于业务数据，但纯单机管理，完全独立于双端同步协议（`model_favorites`、`tarven_rules`）。
- **存储归属**：`.vcpcfg.json` 中的 `standaloneData` 根节点。
- **还原方式**：由 Rust 在同一个写入事务中批量执行 `INSERT OR REPLACE`。

### 2.4 第四级：严禁导出的易失黑名单 (Volatile Runtime Blacklist)
- **特征**：设备特定状态、当前交互现场、单机权限标记、开发者诊断开关。
- **严禁导出的清单**：
  1. `chatSessionStore`（`currentSelectedItem`, `currentTopicId`, `lastActiveTopicMap`）；
  2. `guideStore`（`completed`, `lastSeenAppVersion`）；
  3. `appLifecycle`（`NOTIFICATION_INTRO_KEY`）；
  4. `updateStore` 运行时时间戳（`vcp_last_update_check`）；
  5. `developerSettingsStore`（`vcp-developer-mode`，单机调试开关）；
  6. 任何日志或缓存相关字段。

---

## 3. 前端 Capture & Restore 核心实现范式

```typescript
// src/features/settings/useConfigBackup.ts (重构扩展)

export function captureClientPreferences(): ConfigBackupFile['preferences'] {
  const theme = useThemeStore();
  const chat = useChatPreferencesStore();
  const topic = useTopicStore();
  const diary = useDiaryStore();

  const autoCheckRes = readLocalPreference('vcp_auto_update_check');
  const autoCheckUpdate = autoCheckRes.ok ? autoCheckRes.raw !== 'off' : true;

  return {
    theme: {
      themeKey: theme.currentTheme,
      mode: theme.mode,
    },
    chat: {
      presentationMode: chat.presentationMode,
      smoothStreamingEnabled: chat.smoothStreamingEnabled,
      messageHapticsEnabled: chat.messageHapticsEnabled,
      collapseToolsWhileStreaming: chat.collapseToolsWhileStreaming,
    },
    topic: {
      sortMode: topic.sortMode,
      pinnedTopicKeys: [...topic.pinnedTopicKeys],
    },
    diary: {
      hiddenFolders: [...diary.hiddenFolders],
      folderOrder: [...diary.folderOrder],
      collapsedCategories: [...diary.collapsedCategories],
    },
    client: {
      autoCheckUpdate,
    },
  };
}

export async function restoreClientPreferences(
  prefs: ConfigBackupFile['preferences'],
  result: BackupImportResult,
): Promise<void> {
  // 1. 主题与明暗
  if (prefs.theme) {
    const res = await useThemeStore().restorePreferences(prefs.theme);
    if (res.ok) result.applied.push('主题与深浅模式');
  }

  // 2. 聊天偏好
  if (prefs.chat) {
    const chatOwner = useChatPreferencesStore();
    // 逐项应用并记录 applied ...
  }

  // 3. 话题排序与置顶
  if (prefs.topic) {
    const topicStore = useTopicStore();
    if (prefs.topic.sortMode) topicStore.sortMode = prefs.topic.sortMode;
    if (Array.isArray(prefs.topic.pinnedTopicKeys)) {
      topicStore.pinnedTopicKeys = prefs.topic.pinnedTopicKeys.filter(k => typeof k === 'string');
    }
    result.applied.push('话题展示与置顶偏好');
  }

  // 4. 日记排版
  if (prefs.diary) {
    const diaryStore = useDiaryStore();
    if (prefs.diary.hiddenFolders) diaryStore.hiddenFolders = prefs.diary.hiddenFolders;
    if (prefs.diary.folderOrder) diaryStore.folderOrder = prefs.diary.folderOrder;
    if (prefs.diary.collapsedCategories) diaryStore.collapsedCategories = prefs.diary.collapsedCategories;
    result.applied.push('日记目录排版');
  }
}
```
