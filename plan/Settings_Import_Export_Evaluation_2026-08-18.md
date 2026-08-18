# 设置导入导出功能评估（服务器连接页）

> 日期：2026-08-18 | 状态：评估完成，待决策
> 目标：在「服务器连接」设置子页面新增「导入导出配置」，导出覆盖「用户身份」+「服务器连接」全部配置（含用户头像），保存到系统 Downloads；导入一键还原，服务真机重装测试场景。

---

## 1. 现状调研结论

### 1.1 配置数据来源

两页全部字段均来自单一 `Settings` 结构（`settings` 表 `key='global'` 的一行 JSON）：

| 页面 | 字段 |
|---|---|
| 用户身份 | `userName`、`adminUsername`、`adminPassword` + 用户头像（`avatars` 表 `user/user_avatar`，BLOB） |
| 服务器连接·核心 | `vcpServerUrl`、`vcpApiKey`、`vcpLogUrl`、`vcpLogKey` |
| 服务器连接·同步 | `syncHttpUrl`、`syncServerUrl`、`syncToken`、`fileKey` |

- 读：`read_settings` → `settingsStore.fetchSettings()`
- 写：`update_settings`（JSON merge patch，自动触发 VCPLog/VCPInfo/分布式 runtime reconcile）
- 头像读：`get_avatar("user","user_avatar")` → `{ mimeType, imageData: number[] }`
- 头像写：`save_avatar_data`（前端 `assistantStore.saveAvatar` 已封装）

**结论：导出/导入所需的全部读写命令已存在，Rust 业务层零改动。**

### 1.2 文件通道盘点

| 需求 | 现有能力 | 结论 |
|---|---|---|
| 导出到 Downloads | ❌ 无。项目未装 tauri-plugin-fs/dialog；WebView `<a download>` 在 Tauri Android 上不可靠；插件仅有 MediaStore **图片**写入（`DIRECTORY_PICTURES`） | 需新增 1 个插件命令 |
| 导入选文件 | ✅ 两条路：(a) 插件 `pick_file`（SAF，复制到 cache 返回真实路径）；(b) 前端 `<input type="file">` + FileReader（头像选择已用此模式） | 推荐 (b)，零插件成本 |

### 1.3 minSdk 26 的关键约束

`MediaStore.Downloads` 集合是 **API 29+** 才有的。API 29+ 应用向自己的 Downloads 条目写入**无需任何权限**；API 26–28 需要：
- 运行时申请 `WRITE_EXTERNAL_STORAGE`（Manifest 已声明 `maxSdkVersion=32`，可直接复用），写 `Environment.getExternalStoragePublicDirectory(DIRECTORY_DOWNLOADS)`；或
- 退化为 SAF `ACTION_CREATE_DOCUMENT`（系统保存对话框，免权限，但非"一键"）。

---

## 2. 推荐方案

### 2.1 导出（前端组装 → 新插件命令落盘）

1. 前端 composable `useConfigBackup.ts`：
   - 从 `settingsStore.settings` 按**字段白名单**提取 8+4 个字段（白名单防 `extra` flatten 灌入垃圾键）
   - `get_avatar` 取用户头像 → base64 内嵌（裁剪后通常 <200KB，单次 invoke 无压力）
   - 产出 JSON：`{ app: "vcp-mobile", kind: "settings-backup", version: 1, exportedAt, settings: {...}, avatar: { mimeType, dataBase64 } | null }`
2. 新插件命令 `save_to_downloads(fileName, mimeType, contentBase64)`：
   - Kotlin：API 29+ `MediaStore.Downloads` + `RELATIVE_PATH=Download/VCPMobile` + IS_PENDING 两段写（项目已有同模式图片写入代码可参照）
   - API 26–28：请求 `WRITE_EXTERNAL_STORAGE` 后直接写公共 Downloads 目录
   - 四重注册：`lib.rs` invoke_handler → `build.rs` COMMANDS → `permissions/*.toml` → `guest-js/index.ts`，然后 `pnpm check`
3. 文件名：`vcp-mobile-config-20260818-1430.json`，成功后 toast 显示 mono 文件名。

### 2.2 导入（WebView 文件选择 → 白名单 patch）

1. `<input type="file" accept=".json,application/json">` + FileReader（复用头像选择模式，零插件改动）
2. 解析校验：`kind`/`version` 匹配、`settings` 为对象、字段类型检查、大小上限（如 2MB 拒绝）
3. **二次确认对话框**（覆盖现有配置 + 明文敏感信息警告）
4. `settingsStore.updateSettings(白名单 patch)` → 自动完成持久化与运行时重连
5. 头像存在则 `assistantStore.saveAvatar("user","user_avatar", ...)`

### 2.3 UI 落点

「服务器连接」子页面底部新增第三个分组「配置备份」：`SettingsCard` + 两行 `SettingsRow`（导出 / 导入），复用 `SettingsActionWithStatus` 反馈模式。符合高密度线性布局宪法，无新增层级。

---

## 3. Magi 三方思辨

- **Melchior（逻辑/系统）**：导出源用前端 store 而非 Rust 序列化，避免 camelCase 双轨；IPC 载荷 <1MB 单次 invoke 安全；导入走 `update_settings` 天然继承并发锁与 generation reconcile，VCPLog 断连重连无需额外处理。
- **Balthasar（直觉/美学）**：导入是破坏性操作，必须确认对话框；成功反馈用 toast + 等宽文件名；不引入新覆盖层级，沿用 Settings 原子组件。
- **Casper（务实/交付）**：MVP = 1 个 Kotlin 方法 + 1 个 Rust 薄封装 + 1 个 composable + 1 个 Section 组件。不做加密、不做自动备份、不做多端合并。预估 1 个工作日。

---

## 4. 风险与注意点

1. **明文敏感信息**：导出文件含 API Key / Sync Token / 管理员密码，位于公共 Downloads 可被其他应用读取。导出成功提示中必须警告；加密（口令 AES）列为可选 v2。
2. **API 26–28 兼容**：需真机/模拟器验证权限分支；若测试机均 ≥29，可将 fallback 简化为 SAF。
3. **字段漂移**：新增设置字段时需同步白名单——用 L4 单测锁定白名单与 `AppSettings` 的对应关系。
4. **真机重装场景**：重装后首次导入时 DB 为空，`update_settings` 从默认值 merge，行为正确；头像 `save_avatar_data` 对 `user/user_avatar` 固定单例天然支持。

## 5. 测试计划

- L4 Vitest：导出 payload 构造、导入解析校验（白名单/版本/坏 JSON/超限）
- L3 Kotlin（Robolectric）：`saveToDownloads` 参数解析与 API 分支
- L5 契约：插件命令四重注册快照
- L7 真机：导出 → 卸载重装 → 导入 → 验证连接

## 6. 待决策

1. 导出文件是否需要口令加密？（建议 v1 不做，仅警告）
2. API 26–28 fallback：直接写 + 权限申请，还是 SAF 对话框？（建议前者，Manifest 已有权限声明）
3. 导出范围是否包含分布式节点配置（`distributed*`）？当前评估按需求仅含身份+连接两页。
