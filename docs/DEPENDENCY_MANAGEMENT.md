# VCP Mobile 依赖管理宪章

> 本文档为 VCP Mobile 项目依赖更新的唯一权威参考。任何涉及依赖版本变更的操作，必须先阅读本文件对应章节。
> 适用范围：Rust 后端、Vue 3 前端、Android Gradle 构建、CI/CD 环境。

---

## 1. 版本锁定哲学

### 1.1 为什么使用精确版本

VCP Mobile 采用**完全精确版本锁定**策略（Exact Version Pinning），禁止在核心依赖中使用 `^`、`~`、`>=` 或 `"latest"`。理由如下：

1. **可复现构建（Reproducible Builds）**：Android 发布包一旦签名即不可篡改。若构建产物因依赖隐式升级而发生行为漂移，将无法追溯。
2. **Tauri 跨层契约**：Tauri 是一个横跨 Rust crate、npm CLI、npm API 包、Android Gradle 插件、Kotlin 运行时的大型框架。任意一层版本错配都会导致编译失败或运行时 ABI 不兼容。
3. **移动端特殊性**：Android NDK、Gradle Plugin、compileSdk 之间存在硬编码的兼容性矩阵。非确定性升级极易在真机上触发原生崩溃（native crash）。
4. **审计与回滚**：精确版本使 `git diff` 即可定位依赖变更，回滚只需一次 `git revert`。

### 1.2 Tauri 跨层版本契约

Tauri 核心组件必须**严格同版本或遵循官方发布的兼容矩阵**。以下组合必须视为**原子单位**：

- `tauri` (Rust crate)
- `@tauri-apps/cli` (npm devDependency)
- `@tauri-apps/api` (npm dependency)
- `tauri-build` (Rust build dependency) — 版本独立，但需兼容
- `tauri-utils` (Rust utility crate) — 版本独立，但需兼容

> **铁律**：当 `tauri` crate 从 `2.11.1` 升级到 `2.12.0` 时，`@tauri-apps/cli`、`@tauri-apps/api` 必须在**同一次 commit** 内同步升级。`tauri-build` 和 `tauri-utils` 也需检查兼容性，但它们有自己的独立版本号（如 `tauri-build 2.6.1`、`tauri-utils 2.9.1`），不需要与 `tauri` core 版本号一致。

### 1.3 版本号书写规范

| 文件类型 | 正确示例 | 错误示例 | 说明 |
|---------|---------|---------|------|
| `Cargo.toml` | `version = "2.11.1"` | `version = "2"` / `version = "^2.11"` | Rust Cargo 默认即精确匹配，但禁止人为使用语义范围 |
| `package.json` | `"2.11.2"` | `"^2.11.2"` / `"latest"` / `"*"` | pnpm 会尊重 `package.json` 中的前缀，必须显式去除 |
| Gradle | `version = "2.11.1"` | — | Kotlin DSL 通常为精确字符串，保持现状 |

---

## 2. 依赖清单

### 2.1 Rust 后端层 (`src-tauri/Cargo.toml`)

| 包名 | 当前锁定版本 | 更新频率建议 | 最新版本来源 | 备注 |
|------|-------------|-------------|-------------|------|
| `tauri` | `2.11.1` | 跟随官方 Release Note | crates.io | 核心运行时，**必须与 CLI/API 对齐** |
| `tauri-build` | `2.6.1` | 与 `tauri` 同步检查 | crates.io | Build script 依赖，版本独立于 tauri core |
| `tauri-utils` | `2.9.1` | 与 `tauri` 同步检查 | crates.io | 工具函数集，版本独立于 tauri core |
| `tauri-plugin-log` | `2.8.0` | 每季度检查 | crates.io | 日志插件 |
| `tauri-plugin-opener` | `2.5.4` | 每季度检查 | crates.io | 系统打开器插件 |
| `serde` | `1` | 仅安全补丁 | crates.io | 序列化基石，极稳定 |
| `serde_json` | `1` | 仅安全补丁 | crates.io | — |
| `tokio` | `1` | 仅安全补丁 | crates.io | 异步运行时 |
| `reqwest` | `0.12` | 每季度检查 | crates.io | HTTP 客户端，注意 `rustls-tls` feature |
| `sqlx` | `0.8.6` | 每季度检查 | crates.io | SQLite 异步 ORM |
| `rusqlite` | `0.32.1` | 每季度检查 | crates.io | 同步 SQLite，启用 `bundled` |
| `tokio-tungstenite` | `0.26` | 每季度检查 | crates.io | WebSocket 客户端 |
| `image` | `0.25` | 每季度检查 | crates.io | 图像处理 |
| `syntect` | `5.3.0` | 每半年检查 | crates.io | 语法高亮，体积敏感 |
| `pulldown-cmark` | `0.13.3` | 每半年检查 | crates.io | Markdown 解析 |
| `scraper` | `0.19` | 每半年检查 | crates.io | HTML 解析 |
| `fancy-regex` | `0.13` | 每半年检查 | crates.io | 正则引擎 |
| `zstd` | `0.13` | 每半年检查 | crates.io | 压缩 |
| `zip` | `2` | 每半年检查 | crates.io | ZIP 处理，注意 feature 裁剪 |
| `dashmap` | `6` | 每季度检查 | crates.io | 并发 HashMap |
| `lru` | `0.12` | 每半年检查 | crates.io | LRU 缓存 |
| `uuid` | `1` | 仅安全补丁 | crates.io | UUID 生成 |
| `chrono` | `0.4` | 仅安全补丁 | crates.io | 日期时间 |
| `base64` | `0.22` | 仅安全补丁 | crates.io | Base64 编解码 |
| `sha2` | `0.10` | 仅安全补丁 | crates.io | SHA-256 |
| `hex` | `0.4` | 仅安全补丁 | crates.io | 十六进制 |
| `log` | `0.4` | 仅安全补丁 | crates.io | 日志 facade |
| `lazy_static` | `1.4` | 仅安全补丁 | crates.io | 懒加载静态变量 |
| `futures-util` | `0.3` | 仅安全补丁 | crates.io | Future 工具 |
| `tokio-util` | `0.7` | 仅安全补丁 | crates.io | Tokio 扩展 |
| `url` | `2` | 仅安全补丁 | crates.io | URL 解析 |
| `percent-encoding` | `2` | 仅安全补丁 | crates.io | URL 编码 |
| `urlencoding` | `2` | 仅安全补丁 | crates.io | URL 编码 |
| `regex` | `1` | 仅安全补丁 | crates.io | 标准正则 |
| `rand` | `0.8` | 仅安全补丁 | crates.io | 随机数 |
| `semver` | `1` | 仅安全补丁 | crates.io | 语义版本 |
| `ego-tree` | `0.6` | 每半年检查 | crates.io | DOM 树操作 |
| `async-trait` | `0.1` | 仅安全补丁 | crates.io | 异步 trait |
| `libc` | `0.2` | 仅安全补丁 | crates.io | FFI C 库绑定 |
| `memmap2` | `0.9` | 仅安全补丁 | crates.io | 内存映射 |
| `postcard` | `1` | 每半年检查 | crates.io | 序列化格式 |
| `encoding_rs` | `0.8` | 仅安全补丁 | crates.io | 编码转换 |
| `chardetng` | `0.1` | 每半年检查 | crates.io | 编码检测 |
| `jni` | `0.21` | 每季度检查 | crates.io | Android JNI 绑定 |

### 2.2 前端层 (`package.json`)

#### Runtime Dependencies

| 包名 | 当前锁定版本 | 更新频率建议 | 最新版本来源 | 备注 |
|------|-------------|-------------|-------------|------|
| `@tauri-apps/api` | `2.11.0` | 与 `tauri` crate 同步 | npm | **必须与 Rust tauri 版本对齐** |
| `@tauri-apps/plugin-opener` | `2.5.4` | 与 `tauri-plugin-opener` 同步 | npm | 插件前端绑定 |
| `vue` | `^3.5.33` | 每月检查 | npm | 核心框架 |
| `vue-router` | `^5.0.6` | 每月检查 | npm | 路由 |
| `pinia` | `^3.0.4` | 每月检查 | npm | 状态管理 |
| `pinia-plugin-persistedstate` | `^4.7.1` | 每季度检查 | npm | 状态持久化 |
| `@vueuse/core` | `^14.2.1` | 每月检查 | npm | 组合式工具库 |
| `vite` | `^6.4.2` | 每月检查 | npm | 构建工具 |
| `@vitejs/plugin-vue` | `^5.2.4` | 与 `vite` 同步 | npm | Vue Vite 插件 |
| `unocss` | `^66.6.8` | 每季度检查 | npm | 原子 CSS |
| `@unocss/*` | `^66.6.8` | 与 `unocss` 同步 | npm | UnoCSS 生态 |
| `highlight.js` | `^11.11.1` | 每季度检查 | npm | 语法高亮 |
| `dompurify` | `^3.4.2` | 每季度检查 | npm | XSS 净化 |
| `katex` | `^0.16.45` | 每半年检查 | npm | LaTeX 渲染 |
| `mermaid` | `^11.14.0` | 每季度检查 | npm | 图表渲染 |
| `pdfjs-dist` | `^5.7.284` | 每季度检查 | npm | PDF 渲染 |
| `mammoth` | `^1.12.0` | 每半年检查 | npm | Word 文档解析 |
| `lucide-vue-next` | `^0.576.0` | 每月检查 | npm | 图标库 |
| `sortablejs` | `^1.15.7` | 每半年检查 | npm | 拖拽排序 |
| `vue-cropper` | `^1.1.4` | 每半年检查 | npm | 图像裁剪 |
| `date-fns` | `^4.1.0` | 每季度检查 | npm | 日期工具 |

#### Dev Dependencies

| 包名 | 当前锁定版本 | 更新频率建议 | 最新版本来源 | 备注 |
|------|-------------|-------------|-------------|------|
| `@tauri-apps/cli` | `2.11.2` | 与 `tauri` crate 同步 | npm | **必须与 Rust tauri 版本对齐** |
| `typescript` | `~5.6.3` | 每季度检查 | npm | TS 编译器 |
| `vue-tsc` | `^2.2.12` | 与 `vue`/`typescript` 同步 | npm | Vue 类型检查 |
| `eslint` | `^10.2.1` | 每季度检查 | npm | 代码检查 |
| `@typescript-eslint/*` | `^8.59.1` | 与 `eslint`/`typescript` 同步 | npm | TS ESLint 规则 |
| `eslint-plugin-vue` | `^10.9.0` | 与 `eslint`/`vue` 同步 | npm | Vue ESLint 规则 |
| `prettier` | `^3.8.3` | 每季度检查 | npm | 代码格式化 |
| `eslint-config-prettier` | `^10.1.8` | 与 `prettier`/`eslint` 同步 | npm | Prettier 兼容配置 |
| `eslint-plugin-prettier` | `^5.5.5` | 与 `prettier`/`eslint` 同步 | npm | Prettier ESLint 插件 |
| `@types/katex` | `^0.16.8` | 与 `katex` 同步 | npm | 类型定义 |
| `@types/sortablejs` | `^1.15.9` | 与 `sortablejs` 同步 | npm | 类型定义 |
| `@iconify-json/ph` | `^1.2.2` | 每季度检查 | npm | Iconify 图标数据 |

> **注意**：前端非 Tauri 生态的依赖当前使用 `^` 前缀。鉴于本宪章确立，未来应将核心 runtime 依赖（`vue`、`vite`、`pinia`、`@vueuse/core`）逐步迁移为精确版本。

### 2.3 Android Gradle 层

| 包名 / 配置 | 当前锁定版本 | 更新频率建议 | 最新版本来源 | 备注 |
|------------|-------------|-------------|-------------|------|
| Android Gradle Plugin (AGP) | `8.11.0` | 每季度检查 | Google Maven | 与 Gradle Wrapper 版本耦合 |
| Kotlin Gradle Plugin | `1.9.25` | 每季度检查 | Maven Central | **必须与 Tauri Android 模板要求对齐** |
| `compileSdk` | `36` | 跟随 AGP / 每年 | Android SDK Manager | — |
| `targetSdk` | `36` | 跟随 `compileSdk` | Android SDK Manager | 必须与 `compileSdk` 一致 |
| `minSdk` | `26` | 仅业务需求驱动 | Android SDK Manager | `tauri.conf.json` 中同步声明 |
| Android NDK | `29.0.13846066` | 每年或跟随 Tauri 要求 | Android SDK Manager | Rust `aarch64-linux-android` 目标依赖 |
| `androidx.webkit:webkit` | `1.14.0` | 每季度检查 | Google Maven | WebView 扩展 |
| `androidx.appcompat:appcompat` | `1.7.1` | 每季度检查 | Google Maven | AppCompat |
| `androidx.activity:activity-ktx` | `1.10.1` | 每季度检查 | Google Maven | Activity KTX |
| `com.google.android.material:material` | `1.12.0` | 每季度检查 | Google Maven | Material Design |
| `androidx.lifecycle:lifecycle-process` | `2.10.0` | 每季度检查 | Google Maven | Tauri 自动生成依赖 |
| `junit:junit` | `4.13.2` | 仅安全补丁 | Maven Central | 测试框架 |
| `androidx.test.ext:junit` | `1.1.4` | 仅安全补丁 | Google Maven | Android 测试 |
| `androidx.test.espresso:espresso-core` | `3.5.0` | 仅安全补丁 | Google Maven | UI 测试 |

### 2.4 构建工具与环境层

| 工具 | 当前版本 | 更新频率建议 | 来源 | 备注 |
|------|---------|-------------|------|------|
| Node.js | `22.21.1` (LTS) | 每年 major / 每季度 minor | nodejs.org | CI 与本地开发统一 |
| pnpm | `10.33.0` | 每季度检查 | pnpm.io | 包管理器 |
| Rust Toolchain | `1.95.0` | 每季度检查 | rustup | MSRV 以 `tauri` crate 要求为准 |
| Java (Temurin) | `17` | 每年 LTS | adoptium.net | Android 构建必需 |
| Gradle (Wrapper) | 与 AGP `8.11.0` 兼容版本 | 跟随 AGP | Gradle 官方 | 由 `gradle/wrapper/gradle-wrapper.properties` 决定 |

---

## 3. 更新规则与流程

### 3.1 通用前置检查

在任何依赖升级前，必须完成以下检查：

1. `git status` 确认工作区干净（无未提交修改）。
2. 确认当前分支为 `main` 或专门创建的 `deps/xxx` 分支。
3. 阅读目标依赖的 **Changelog / Release Notes**，标记所有 `Breaking Changes`。
4. 对于 Tauri 生态依赖，查阅 [Tauri 官方迁移指南](https://tauri.app/start/migrate/)。

### 3.2 Tauri 核心更新流程（原子升级）

当需要升级 Tauri 核心（如 `2.11.x` → `2.12.x`）：

**步骤 1：同步修改以下 4 个文件**

| 文件 | 字段 | 新值 |
|------|------|------|
| `src-tauri/Cargo.toml` | `[dependencies] tauri` | 新版本 |
| `src-tauri/Cargo.toml` | `[build-dependencies] tauri-build` | 与新版本一致 |
| `src-tauri/Cargo.toml` | `[dependencies] tauri-utils` | 与新版本一致 |
| `package.json` | `devDependencies["@tauri-apps/cli"]` | 与新版本一致（允许小版本差异，如 `2.11.2`） |
| `package.json` | `dependencies["@tauri-apps/api"]` | 与新版本一致 |

> `@tauri-apps/cli` 与 `@tauri-apps/api` 的版本通常与 Rust 侧 `tauri` **主版本.次版本**一致，补丁号可能略有差异（如 `2.11.1` vs `2.11.2`），以 npm 最新可用为准。`tauri-build` 和 `tauri-utils` 有独立的版本号，不需要与 `tauri` core 版本号一致，但升级时需检查兼容性。

**步骤 2：执行强制检查**

```powershell
# 1. 更新 pnpm lockfile
pnpm install

# 2. 前端类型检查 + Rust 编译检查
pnpm check

# 3. Android 开发环境热重载 smoke test
pnpm tauri android dev
```

**步骤 3：运行 Android 真机/模拟器测试清单**

- [ ] 应用正常启动，无闪退
- [ ] WebView 成功加载前端资源
- [ ] Tauri 命令（invoke）正常响应
- [ ] 文件上传/下载功能正常
- [ ] 同步服务 WebSocket 连接正常
- [ ] 日志插件正常输出

**步骤 4：提交规范**

```
deps: bump tauri to 2.12.0

- tauri: 2.11.1 -> 2.12.0
- tauri-build: 2.6.1 -> 检查 crates.io 最新兼容版本
- tauri-utils: 2.9.1 -> 检查 crates.io 最新兼容版本
- @tauri-apps/cli: 2.11.2 -> 2.12.0
- @tauri-apps/api: 2.11.0 -> 2.12.0
```

### 3.3 Tauri 插件更新流程

Tauri 插件采用**双端版本配对**：

- Rust 端：`tauri-plugin-<name>` crate
- 前端端：`@tauri-apps/plugin-<name>` npm 包

**更新步骤**：

1. 在 [Tauri 插件仓库](https://github.com/tauri-apps/plugins-workspace) 或 crates.io/npm 确认两端版本对应关系。
2. 同步修改 `src-tauri/Cargo.toml` 与 `package.json`。
3. 检查插件的 `README.md` 是否有新的权限配置（`tauri.conf.json` / `capabilities/`）。
4. 执行 `pnpm check` 与 `pnpm tauri android dev` smoke test。

### 3.4 Android Gradle 依赖更新流程

**步骤 1：AGP 升级（最敏感）**

AGP 升级通常伴随 Kotlin、Gradle Wrapper、compileSdk 的联动：

1. 查阅 [Android Gradle Plugin 兼容性表](https://developer.android.com/studio/releases/gradle-plugin#updating-gradle)。
2. 同步更新：
   - `src-tauri/gen/android/build.gradle.kts` 中的 `com.android.tools.build:gradle`
   - `src-tauri/gen/android/buildSrc/build.gradle.kts` 中的 `com.android.tools.build:gradle`
   - `gradle/wrapper/gradle-wrapper.properties` 中的 `distributionUrl`
3. 若 AGP 要求更高 `compileSdk`，同步修改：
   - `src-tauri/gen/android/app/build.gradle.kts` 的 `compileSdk`
   - `src-tauri/gen/android/app/build.gradle.kts` 的 `targetSdk`
   - `src-tauri/tauri.conf.json` 的 `bundle.android.minSdkVersion`（若最小 SDK 也调整）

**步骤 2：Kotlin 升级**

- 修改 `src-tauri/gen/android/build.gradle.kts` 中的 `kotlin-gradle-plugin` 版本。
- 确认 Tauri 官方模板是否已支持该 Kotlin 版本。

**步骤 3：AndroidX / Material 升级**

- 修改 `src-tauri/gen/android/app/build.gradle.kts` 的 `dependencies` 块。
- 注意 `tauri.build.gradle.kts` 为自动生成文件，**禁止手动修改**。

**步骤 4：验证**

```powershell
# 清理构建缓存后重新构建
cd src-tauri/gen/android
; .\gradlew clean
; cd ../../..
pnpm tauri android build --apk --target aarch64
```

### 3.5 回滚计划

若升级后发现问题：

1. **立即回滚**：`git revert <upgrade-commit>`。
2. **清理残留**：
   ```powershell
   cd src-tauri; cargo clean; cd ..
   rm -Recurse -Force node_modules
   pnpm install
   ```
3. **验证回滚**：`pnpm check` 通过即为回滚成功。
4. **问题归档**：在 `plans/04_Logs/` 记录失败原因，标记该版本为黑名单。

---

## 4. 版本对齐矩阵

### 4.1 Tauri 核心跨层对齐

| Rust Crate | 当前版本 | npm 包 | 当前版本 | 对齐规则 |
|-----------|---------|--------|---------|---------|
| `tauri` | `2.11.1` | `@tauri-apps/cli` | `2.11.2` | 主.次版本必须一致（`2.11.x`） |
| `tauri` | `2.11.1` | `@tauri-apps/api` | `2.11.0` | 主.次版本必须一致（`2.11.x`） |
| `tauri-build` | `2.6.1` | — | — | 独立版本，需与 `tauri` 兼容 |
| `tauri-utils` | `2.9.1` | — | — | 独立版本，需与 `tauri` 兼容 |

### 4.2 Tauri 插件跨层对齐

| Rust 插件 | 当前版本 | npm 插件 | 当前版本 | 对齐规则 |
|----------|---------|---------|---------|---------|
| `tauri-plugin-opener` | `2.5.4` | `@tauri-apps/plugin-opener` | `2.5.4` | 版本号应完全一致 |
| `tauri-plugin-log` | `2.8.0` | — | — | 无前端包，Rust 单独升级 |

### 4.3 Rust 工具链与 MSRV

| 项目 | 当前值 | 约束来源 |
|------|--------|---------|
| Rust Toolchain | `1.95.0` | 本地开发环境 |
| Tauri MSRV | 见 `tauri` crate 文档 | `tauri` `Cargo.toml` 中 `rust-version` |
| Cargo Edition | `2021` | `src-tauri/Cargo.toml` |

> **检查方法**：运行 `rustc --version`，确认不低于 Tauri 官方要求的 MSRV。若 Tauri 升级后提高 MSRV，必须同步更新 CI（`release.yml`、`ci.yml`）中的 Rust 安装步骤。

### 4.4 Android SDK 对齐

| 配置项 | 当前值 | 声明位置 |
|--------|--------|---------|
| `compileSdk` | `36` | `app/build.gradle.kts` |
| `targetSdk` | `36` | `app/build.gradle.kts` |
| `minSdk` | `26` | `app/build.gradle.kts` + `tauri.conf.json` |
| `kotlinOptions.jvmTarget` | `1.8` | `app/build.gradle.kts` |

**对齐规则**：`compileSdk == targetSdk`，且 `minSdk` 在 Gradle 与 `tauri.conf.json` 中双写一致。

---

## 5. 禁止行为（红线）

以下行为在任何情况下都**严格禁止**：

1. **禁止在 `package.json` 中使用 `"latest"`**。包括核心依赖、devDependencies、以及脚本中的全局安装命令。
2. **禁止在 `Cargo.toml` 中对 Tauri 核心 crate 使用范围版本**。如 `tauri = "2"`、`tauri = "^2.11"`、`tauri = ">=2.11"` 等均属违规。
3. **禁止单层更新**。例如只升级 `@tauri-apps/cli` 而不升级 `tauri-build`，或只升级 Rust 插件而不升级对应 npm 包。
4. **禁止在发布前 7 天内更新任何依赖**。所有依赖更新必须经过至少一周的 soak time（ soak 测试期）。
5. **禁止跳过 `pnpm check` 直接提交**。Rust 编译错误必须在提交前清零。
6. **禁止手动修改 `tauri.build.gradle.kts`**。该文件由 Tauri CLI 自动生成，手动修改会在下次生成时被覆盖。
7. **禁止在 CI 中使用 `pnpm install` 而不加 `--frozen-lockfile`**。`release.yml` 已正确配置，不得移除该标志。
8. **禁止混合使用 npm/yarn 与 pnpm**。项目唯一包管理器为 pnpm，`package-lock.json` 与 `yarn.lock` 不应存在于仓库中。

---

## 6. Android 专项依赖管理

### 6.1 SDK 版本三原则

1. **`compileSdk` 必须等于 `targetSdk`**。两者不一致会导致 Android 构建系统警告，甚至运行时行为差异。
2. **`minSdk` 双写一致**。`app/build.gradle.kts` 中的 `minSdk = 26` 与 `tauri.conf.json` 中的 `bundle.android.minSdkVersion` 必须为同一数值。
3. **SDK 升级顺序**：先升级 `compileSdk`/`targetSdk`，验证通过后再考虑提升 `minSdk`（仅当业务需要新 API 时）。

### 6.2 NDK 版本追踪

| 环境 | 当前 NDK 版本 | 配置位置 |
|------|--------------|---------|
| CI (`release.yml`) | `29.0.13846066` | `.github/workflows/release.yml` |
| 本地开发 | 由开发者通过 Android Studio / `sdkmanager` 安装 | `$ANDROID_SDK_ROOT/ndk/` |

**规则**：

- CI 与本地 NDK 版本应尽量一致。若 CI 升级 NDK，必须在团队内广播。
- NDK 升级后，必须重新编译 Rust 标准库与依赖：`cargo clean` 后重新构建。
- NDK 版本与 `rustc` 的目标 `aarch64-linux-android` 存在隐性兼容关系，升级前查阅 Rust Android 社区反馈。

### 6.3 Kotlin 版本与 Tauri 模板

当前 Kotlin Gradle Plugin：`1.9.25`。

- Kotlin 版本受 AGP 和 Tauri Android 模板双重约束。
- 升级 Kotlin 前，确认 Tauri `tauri-build` 是否已适配新版本 Kotlin 语法。
- `kotlinOptions.jvmTarget = "1.8"` 保持现状，除非 AGP 强制要求提升。

### 6.4 Android Gradle Plugin 版本

当前 AGP：`8.11.0`。

- AGP 与 Gradle Wrapper 版本存在严格对应关系。升级 AGP 时，必须同步更新 `gradle/wrapper/gradle-wrapper.properties`。
- AGP `8.11.0` 要求 Gradle `8.9+`。

---

## 7. 紧急更新预案（安全补丁）

当某个依赖发布**关键安全漏洞修复**（CVE、RUSTSEC、npm audit critical）时，启动以下快速通道：

### 7.1 评估清单

在动手更新前，先回答以下问题：

- [ ] 漏洞是否影响 VCP Mobile 的**实际攻击面**？（例如：仅影响 Windows 桌面端的漏洞对 Android 发布无影响）
- [ ] 漏洞是否影响**发布版本**的构建产物？（仅影响 devDependencies 的漏洞可降低优先级）
- [ ] 补丁版本是否为向后兼容的**补丁号升级**（`x.y.Z` → `x.y.Z+1`）？若是，风险极低，可直接更新。
- [ ] 若涉及次版本或主版本升级，是否存在 Breaking Changes？

### 7.2 快速通道步骤

**情况 A：补丁号升级（推荐直接执行）**

1. 修改对应版本号（如 `2.11.1` → `2.11.2`）。
2. 执行 `pnpm check`。
3. 执行 `pnpm tauri android dev` 快速 smoke test（5 分钟）。
4. 直接提交 PR，标题前缀 `[SECURITY]`。

**情况 B：次版本 / 主版本升级（需评审）**

1. 在 `plans/04_Logs/` 创建漏洞分析文档，记录 CVE 编号、影响范围、升级方案。
2. 执行完整更新流程（第 3 节）。
3. 必须经过 **Magi 三贤者协议**快速评审（见 `AGENTS.md` 第 8.2 节）：
   - Melchior：确认 Rust 侧 ABI 兼容性。
   - Balthasar：确认 Android 端交互与 UI 无异常。
   - Casper：确认升级成本与发布排期不冲突。
4. 合并前必须在真机上完成完整回归测试。

### 7.3 合并前测试清单（安全更新专用）

- [ ] `pnpm check` 零错误。
- [ ] `cargo clippy -- -D warnings` 零警告。
- [ ] `pnpm tauri android dev` 真机/模拟器启动成功。
- [ ] 核心功能回归：登录/同步/聊天/文件上传/设置。
- [ ] APK Release 构建成功：`pnpm tauri android build --apk --target aarch64`。
- [ ] APK 安装后无闪退，签名验证通过。

### 7.4 时间线要求

| 严重级别 | 评估时限 | 合并时限 | 发布后验证 |
|---------|---------|---------|-----------|
| Critical (RCE/权限绕过) | 2 小时 | 24 小时 | 72 小时内真机验证 |
| High (数据泄露/DoS) | 24 小时 | 72 小时 | 一周内验证 |
| Medium/Low | 常规排期 | 下次迭代 | 随版本发布验证 |

---

## 附录 A：快速查询命令

```powershell
# 查询 Rust 依赖最新版本（示例：tauri）
cargo search tauri --limit 1

# 查询 npm 依赖最新版本
npm view @tauri-apps/cli version

# 查询 pnpm 过时的依赖
pnpm outdated

# Rust 安全审计
cargo audit

# npm 安全审计
pnpm audit

# 查看当前 Android NDK 版本
sdkmanager --list_installed | findstr ndk
```

## 附录 B：文件变更映射表

| 依赖类别 | 涉及文件 |
|---------|---------|
| Rust Crates | `src-tauri/Cargo.toml` |
| Rust 插件 | `src-tauri/Cargo.toml` |
| npm Runtime | `package.json` |
| npm Dev | `package.json` |
| npm 插件 | `package.json` |
| AGP / Kotlin | `src-tauri/gen/android/build.gradle.kts`, `src-tauri/gen/android/buildSrc/build.gradle.kts` |
| AndroidX | `src-tauri/gen/android/app/build.gradle.kts` |
| SDK / NDK | `src-tauri/gen/android/app/build.gradle.kts`, `.github/workflows/release.yml` |
| Tauri 配置 | `src-tauri/tauri.conf.json` |
| CI 环境 | `.github/workflows/ci.yml`, `.github/workflows/release.yml` |

---

*文档版本：1.0*  
*最后更新：2026-05-18*  
*维护者：全体贡献者（修改前必读第 5 节红线）*
