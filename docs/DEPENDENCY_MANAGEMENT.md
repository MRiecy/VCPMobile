# VCP Mobile 依赖管理宪章

> 本文档为 VCP Mobile 项目依赖更新的唯一权威参考。任何涉及依赖版本变更的操作，必须先阅读本文件对应章节。
> 适用范围：Rust 后端、Vue 3 前端、Android Gradle 构建、CI/CD 环境。

---

## 1. 版本锁定哲学

### 1.1 为什么以锁文件作为构建真相

VCP Mobile 采用**manifest 表达兼容范围、锁文件冻结实际解析**的策略。Tauri 跨层包与 Gradle 直接依赖使用明确版本；其余 npm/Cargo 依赖可保留受控范围，但 CI/Release 必须分别使用 `pnpm install --frozen-lockfile` 与 Cargo `--locked`。任何场景都禁止 `"latest"`。理由如下：

1. **可复现构建（Reproducible Builds）**：Android 发布包一旦签名即不可篡改。若构建产物因依赖隐式升级而发生行为漂移，将无法追溯。
2. **Tauri 跨层契约**：Tauri 是一个横跨 Rust crate、npm CLI、npm API 包、Android Gradle 插件、Kotlin 运行时的大型框架。任意一层版本错配都会导致编译失败或运行时 ABI 不兼容。
3. **移动端特殊性**：Android NDK、Gradle Plugin、compileSdk 之间存在硬编码的兼容性矩阵。非确定性升级极易在真机上触发原生崩溃（native crash）。
4. **审计与回滚**：`package.json`/`Cargo.toml` 与两份 lockfile 的 diff 共同定位意图和实际解析，回滚边界明确。

### 1.2 Tauri 跨层版本契约

Tauri 核心组件必须**严格同版本或遵循官方发布的兼容矩阵**。以下组合必须视为**原子单位**：

- `tauri` (Rust crate)
- `@tauri-apps/cli` (npm devDependency)
- `@tauri-apps/api` (npm dependency)
- `tauri-build` (Rust build dependency) — 版本独立，但需兼容
- `tauri-utils` (Rust transitive crate) — 当前不在 manifest 直接声明，但需在 `Cargo.lock` 解析图中检查兼容性

> **铁律**：当 `tauri` crate 跨主/次版本升级时，`@tauri-apps/cli`、`@tauri-apps/api` 必须在**同一次 commit** 内同步升级。`tauri-build` 需检查独立兼容版本；`tauri-utils` 等传递依赖以新的 `Cargo.lock` 解析图为准，不得凭文档手工添加不存在的 direct dependency。

### 1.3 版本号书写规范

| 文件类型 | 正确示例 | 错误示例 | 说明 |
|---------|---------|---------|------|
| `Cargo.toml` | `tauri = "2.11.5"` + committed `Cargo.lock` | `tauri = "2"` / `">=2.11"` | Cargo manifest 值仍是兼容范围；CI/Release 用 `--locked` 冻结实际解析 |
| `package.json` | Tauri 包 `"2.11.4"`；普通库可用 `^`/`~` + committed lock | `"latest"` / `"*"` | CI/Release 必须 `pnpm install --frozen-lockfile`，不允许隐式改 lock |
| Gradle | `implementation("androidx.webkit:webkit:1.14.0")` | 动态版本 `1.+` | 直接依赖用精确字符串，并由 verification metadata 固定制品摘要 |

---

## 2. 依赖清单

### 2.1 Rust 后端层 (`src-tauri/Cargo.toml`)

| 包名 | 当前锁定版本 | 更新频率建议 | 最新版本来源 | 备注 |
|------|-------------|-------------|-------------|------|
| `tauri` | `2.11.5` | 跟随官方 Release Note | crates.io | 核心运行时，**必须与 CLI/API 对齐** |
| `tauri-build` | `2.6.3` | 与 `tauri` 同步检查 | crates.io | Build script 依赖，版本独立于 tauri core |
| `tauri-plugin-log` | `2.9.0` | 每季度检查 | crates.io | 日志插件 |
| `tauri-plugin-opener` | `2.5.4` | 每季度检查 | crates.io | 系统打开器插件 |
| `serde` | `1` | 仅安全补丁 | crates.io | 序列化基石，极稳定 |
| `serde_json` | `1` | 仅安全补丁 | crates.io | — |
| `tokio` | `1` | 仅安全补丁 | crates.io | 异步运行时 |
| `reqwest` | `0.12` | 每季度检查 | crates.io | HTTP 客户端，注意 `rustls-tls` feature |
| `sqlx` | `0.8.6` | 每季度检查 | crates.io | SQLite 异步 ORM |
| `rusqlite` | `0.32.1` | 每季度检查 | crates.io | 同步 SQLite，启用 `bundled` |
| `tokio-tungstenite` | `0.26` | 每季度检查 | crates.io | WebSocket 客户端 |
| `syntect` | `5.3.0` | 每半年检查 | crates.io | 语法高亮，体积敏感 |
| `pulldown-cmark` | `0.13.3` | 每半年检查 | crates.io | Markdown 解析 |
| `scraper` | `0.19` | 每半年检查 | crates.io | HTML 解析 |
| `fancy-regex` | `0.16.2` | 每半年检查 | crates.io | manifest 下限 `0.16`，实际版本由 Cargo.lock 冻结 |
| `zstd` | `0.13` | 每半年检查 | crates.io | 压缩 |
| `zip` | `8.6.0` | 每半年检查 | crates.io | ZIP 处理，禁用默认 feature，仅启用 deflate |
| `dashmap` | `6` | 每季度检查 | crates.io | 并发 HashMap |
| `lru` | `0.16.4` | 每半年检查 | crates.io | LRU 缓存；清单安全下限 `0.16.3` |
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
| `memmap2` | `0.9.11` | 仅安全补丁 | crates.io | 内存映射 |
| `pdf_oxide` | `0.3.77` | 每季度检查 | crates.io | 不可信 PDF 解析；禁用默认 feature |
| `encoding_rs` | `0.8` | 仅安全补丁 | crates.io | 编码转换 |
| `chardetng` | `0.1` | 每半年检查 | crates.io | 编码检测 |
| `jni` | `0.21` | 每季度检查 | crates.io | Android JNI 绑定 |

### 2.2 前端层 (`package.json`)

#### Runtime Dependencies

| 包名 | 当前锁定版本 | 更新频率建议 | 最新版本来源 | 备注 |
|------|-------------|-------------|-------------|------|
| `@tauri-apps/api` | `2.11.1` | 与 `tauri` crate 同步 | npm | 根应用与本地插件必须保持一致 |
| `@tauri-apps/plugin-opener` | `2.5.4` | 与 `tauri-plugin-opener` 同步 | npm | 插件前端绑定 |
| `vue` | `^3.5.41` | 每月检查 | npm | 核心框架 |
| `vue-router` | `^5.0.6` | 每月检查 | npm | 路由 |
| `pinia` | `^3.0.4` | 每月检查 | npm | 状态管理 |
| `pinia-plugin-persistedstate` | `^4.7.1` | 每季度检查 | npm | 状态持久化 |
| `@vueuse/core` | `^14.2.1` | 每月检查 | npm | 组合式工具库 |
| `vite` | `^6.4.3` | 每月检查 | npm | 构建工具；6.x 安全补丁线，Vite 8 需独立迁移 |
| `@vitejs/plugin-vue` | `^5.2.4` | 与 `vite` 同步 | npm | Vue Vite 插件 |
| `unocss` | `^66.6.8` | 每季度检查 | npm | 原子 CSS |
| `@unocss/*` | `^66.6.8` | 与 `unocss` 同步 | npm | UnoCSS 生态 |
| `dompurify` | `^3.4.13` | 每季度检查 | npm | XSS 净化 |
| `katex` | `^0.16.45` | 每半年检查 | npm | LaTeX 渲染 |
| `mermaid` | `^11.16.1` | 每季度检查 | npm | 图表渲染；处理不可信模型内容 |
| `marked` | `^18.0.4` | 每季度检查 | npm | Markdown 解析 |
| `morphdom` | `^2.7.8` | 每半年检查 | npm | 增量 DOM 更新 |
| `lucide-vue-next` | `^0.576.0` | 每月检查 | npm | 图标库 |
| `sortablejs` | `^1.15.7` | 每半年检查 | npm | 拖拽排序 |
| `vue-cropper` | `^1.1.4` | 每半年检查 | npm | 图像裁剪 |
| `date-fns` | `^4.1.0` | 每季度检查 | npm | 日期工具 |

#### Dev Dependencies

| 包名 | 当前锁定版本 | 更新频率建议 | 最新版本来源 | 备注 |
|------|-------------|-------------|-------------|------|
| `@tauri-apps/cli` | `2.11.4` | 与 `tauri` crate 同步 | npm | **必须与 Rust tauri 版本对齐** |
| `typescript` | `~5.6.3` | 每季度检查 | npm | TS 编译器 |
| `vue-tsc` | `^2.2.12` | 与 `vue`/`typescript` 同步 | npm | Vue 类型检查 |
| `eslint` | `^10.8.1` | 每季度检查 | npm | 代码检查 |
| `@typescript-eslint/*` | `^8.66.0` | 与 `eslint`/`typescript` 同步 | npm | TS ESLint 规则 |
| `eslint-plugin-vue` | `^10.9.0` | 与 `eslint`/`vue` 同步 | npm | Vue ESLint 规则 |
| `prettier` | `^3.8.3` | 每季度检查 | npm | 代码格式化 |
| `eslint-config-prettier` | `^10.1.8` | 与 `prettier`/`eslint` 同步 | npm | Prettier 兼容配置 |
| `eslint-plugin-prettier` | `^5.5.5` | 与 `prettier`/`eslint` 同步 | npm | Prettier ESLint 插件 |
| `@types/katex` | `^0.16.8` | 与 `katex` 同步 | npm | 类型定义 |
| `@types/sortablejs` | `^1.15.9` | 与 `sortablejs` 同步 | npm | 类型定义 |
| `@iconify-json/ph` | `^1.2.2` | 每季度检查 | npm | Iconify 图标数据 |

> **注意**：前端非 Tauri 生态依赖使用 `^`/`~` 是当前 manifest 策略；可复现性由 committed `pnpm-lock.yaml` 与 frozen install 保证。不要只改前缀而不审查 lockfile 实际解析。

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
| Node.js | `22.x` (LTS) | 每年 major / 每季度 minor | nodejs.org | Actions 固定 Node 22 主线 |
| pnpm | `10.x` | 每季度检查 | pnpm.io | Actions 固定 pnpm 10 主线，解析由 lockfile 冻结 |
| Rust Toolchain | `stable` | 每季度检查 | rustup | Actions toolchain 本身固定 commit，MSRV 以依赖图为准 |
| Java (Temurin) | `17` | 每年 LTS | adoptium.net | Android 构建必需 |
| Gradle (Wrapper) | `8.14.3` | 跟随 AGP | Gradle 官方 | 官方分发 URL + `distributionSha256Sum` |

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

**步骤 1：同步修改以下跨层字段，并更新两份 lockfile**

| 文件 | 字段 | 新值 |
|------|------|------|
| `src-tauri/Cargo.toml` | `[dependencies] tauri` | 新版本 |
| `src-tauri/Cargo.toml` | `[build-dependencies] tauri-build` | 官方兼容的独立版本 |
| `package.json` | `devDependencies["@tauri-apps/cli"]` | 与新版本一致（允许小版本差异，如 `2.11.2`） |
| `package.json` | `dependencies["@tauri-apps/api"]` | 与新版本一致 |

> `@tauri-apps/cli` 与 `@tauri-apps/api` 的版本通常与 Rust 侧 `tauri` **主版本.次版本**一致，补丁号可能略有差异，以官方发布与实际 lockfile 为准。`tauri-build` 使用自己的版本线；`tauri-utils` 当前仅作为传递依赖检查。

**步骤 2：执行强制检查**

```powershell
# 1. 更新 pnpm lockfile
pnpm install

# 2. 前端类型检查 + Rust 编译检查
pnpm check

# 3. Android USB Debug 热重载（Agent 安全入口）
pnpm android:debug:doctor -- --json
pnpm android:debug:dev
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
- Cargo.lock: 审查 tauri-utils 等传递依赖的实际解析变化
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
4. 执行 `pnpm check` 与 `pnpm android:debug:dev` USB Debug smoke test。

### 3.4 Android Gradle 依赖更新流程

**步骤 1：AGP 升级（最敏感）**

AGP 升级通常伴随 Kotlin、Gradle Wrapper、compileSdk 的联动：

1. 查阅 [Android Gradle Plugin 兼容性表](https://developer.android.com/studio/releases/gradle-plugin#updating-gradle)。
2. 同步更新：
   - `src-tauri/gen/android/build.gradle.kts` 中的 `com.android.tools.build:gradle`
   - `src-tauri/gen/android/buildSrc/build.gradle.kts` 中的 `com.android.tools.build:gradle`
   - `gradle/wrapper/gradle-wrapper.properties` 中的官方 `distributionUrl` 与官方 `distributionSha256Sum`
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
cd src-tauri/gen/android
.\gradlew :tauri-plugin-vcp-mobile:testDebugUnitTest
cd ../../..
pnpm tauri android build --apk --target aarch64
```

任何 Gradle 直接依赖、插件或 Wrapper 版本变更都必须重新验证 debug 与 release 两条构建链路。Tauri 生成树更新后还必须执行 `git diff --exit-code -- src-tauri/gen/android src-tauri/plugins/vcp-mobile/permissions`，确保生成结果已显式提交。

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
4. **问题归档**：在 tracked issue 或 `docs/` 对应模块记录失败原因与黑名单版本；当前仓库未启用 `plans/`。

---

## 4. 版本对齐矩阵

### 4.1 Tauri 核心跨层对齐

| Rust Crate | 当前版本 | npm 包 | 当前版本 | 对齐规则 |
|-----------|---------|--------|---------|---------|
| `tauri` | `2.11.5` | `@tauri-apps/cli` | `2.11.4` | 主.次版本必须一致（`2.11.x`） |
| `tauri` | `2.11.5` | `@tauri-apps/api` | `2.11.1` | 主.次版本必须一致（`2.11.x`） |
| `tauri-build` | `2.6.3` | — | — | 独立版本，需与 `tauri` 兼容 |

### 4.2 Tauri 插件跨层对齐

| Rust 插件 | 当前版本 | npm 插件 | 当前版本 | 对齐规则 |
|----------|---------|---------|---------|---------|
| `tauri-plugin-opener` | `2.5.4` | `@tauri-apps/plugin-opener` | `2.5.4` | 版本号应完全一致 |
| `tauri-plugin-log` | `2.9.0` | — | — | 无前端包，Rust 单独升级 |

### 4.3 Rust 工具链与 MSRV

| 项目 | 当前值 | 约束来源 |
|------|--------|---------|
| Rust Toolchain | `stable`（本次本地验证 `1.97.1`） | CI 与本地开发环境 |
| Tauri MSRV | 见 `tauri` crate 文档 | `tauri` `Cargo.toml` 中 `rust-version` |
| 当前依赖图最高 MSRV | `1.88` | `pdf_oxide 0.3.77` / `plist 1.10.0` |
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

### 4.5 RustSec 审计基线与例外

2026-08-11 供应链整理后的原始 `cargo audit` 结果为：`1 vulnerability / 21 warnings`。门禁命令统一使用 `pnpm audit:rust`，它只忽略以下已核实例外，任何新增 vulnerability 仍会使命令失败：

- `RUSTSEC-2023-0071` / `rsa 0.9.10`：RustSec 尚无已修版本；该包仅来自 `sqlx-mysql` 的可选依赖。本项目对 `sqlx` 使用 `default-features = false` 且只启用 SQLite，默认与 `aarch64-linux-android` 编译图均不包含 `rsa`。

21 条 warning 包含 19 条 unmaintained、Linux GTK3 链上的 1 条 `glib` unsound，以及 SQLx SQLite 链上的 1 条 yanked `spin`。它们是公开的上游维护债，不得表述为“已清零”；升级 SQLx、启用 MySQL/全数据库 feature 或 RustSec 出现 RSA 修复版本时，必须立即撤销例外并重新评估。

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
9. **禁止 CI/Release 中使用可移动 Action tag**。所有 `uses:` 必须固定完整 commit SHA，并在升级时记录对应上游版本。
10. **禁止绕过锁门禁**。Cargo 使用 `--locked`；Gradle Wrapper 使用官方 URL/SHA 并由 CI/Release 核对官方 wrapper JAR SHA-256。

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
- AGP `8.11.0` 要求 Gradle `8.13+`；当前 Wrapper `8.14.3` 满足要求。

Robolectric 的 instrumented Android JAR 由测试运行期解析。仓库只保留 Google Maven、Maven Central 与 Root 能力所需的 content-filtered JitPack。不得恢复第三方镜像绕过下载问题。

### 6.5 发布供应链门禁

- `gradle-wrapper.properties` 固定官方 Gradle 8.14.3 分发与官方 SHA-256；wrapper JAR/脚本由该版本官方 Wrapper 任务生成，CI/Release 另行核对官方 wrapper JAR SHA-256。
- CI 在 `tauri android init --ci` 后检查 Android 生成树与插件权限生成树无漂移，并运行 Gradle JVM 测试。
- Release 仅接受同 commit 已成功的 CI，核对 tag/HEAD/event SHA、四处版本源和 Android versionCode。
- 签名 secrets 只进入恢复、构建与验证步骤；缺任一输入即失败。验签步骤要求 APK 单一签名者且拒绝调试证书。
- Release 只上传 arm64 签名 APK 与其 `.sha256`；不发布可被主 WebView 独立加载的前端 ZIP。

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
3. 执行 `pnpm android:debug:dev` 快速 USB Debug smoke test（5 分钟）。
4. 直接提交 PR，标题前缀 `[SECURITY]`。

**情况 B：次版本 / 主版本升级（需评审）**

1. 在 tracked issue 或 `docs/` 对应模块记录 CVE 编号、影响范围与升级方案；当前仓库未启用 `plans/`。
2. 执行完整更新流程（第 3 节）。
3. 必须经过 **Magi 三贤者协议**快速评审（见 `CLAUDE.md` 第 9.2 节）：
   - Melchior：确认 Rust 侧 ABI 兼容性。
   - Balthasar：确认 Android 端交互与 UI 无异常。
   - Casper：确认升级成本与发布排期不冲突。
4. 合并前必须在真机上完成完整回归测试。

### 7.3 合并前测试清单（安全更新专用）

- [ ] `pnpm check` 零错误。
- [ ] `cargo clippy --locked -- -D warnings` 零警告。
- [ ] `pnpm android:debug:dev` USB 真机启动成功。
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

# Rust 安全审计（包含已核实的具名例外）
pnpm audit:rust

# npm 安全审计门禁
pnpm audit --audit-level=high

# 查看当前 Android NDK 版本
sdkmanager --list_installed | findstr ndk
```

## 附录 B：文件变更映射表

| 依赖类别 | 涉及文件 |
|---------|---------|
| Rust Crates | `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock` |
| Rust 插件 | `src-tauri/Cargo.toml` |
| npm Runtime / Dev / 插件 | `package.json`, `pnpm-lock.yaml` |
| AGP / Kotlin / Gradle 供应链 | `src-tauri/gen/android/build.gradle.kts`, `src-tauri/gen/android/buildSrc/build.gradle.kts`, `src-tauri/gen/android/gradle/wrapper/` |
| AndroidX | `src-tauri/gen/android/app/build.gradle.kts` |
| SDK / NDK | `src-tauri/gen/android/app/build.gradle.kts`, `.github/workflows/release.yml` |
| Tauri 配置 | `src-tauri/tauri.conf.json` |
| CI 环境 | `.github/workflows/ci.yml`, `.github/workflows/release.yml` |

---

*文档版本：1.0*  
*最后更新：2026-05-18*  
*维护者：全体贡献者（修改前必读第 5 节红线）*
