# VCP Mobile 依赖管理宪章

> 本文档维护当前依赖清单、兼容约束与更新流程；具体版本和解析结果以 manifest 与锁文件为准。涉及依赖版本变更时，先阅读对应章节。
> 适用范围：Rust 后端、Vue 3 前端、Android Gradle 构建、CI/CD 环境。
> 维护时原位更新相应条目，不追加按日期排列的升级经过、测试成绩或临时报告路径。

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

下表区分 manifest 声明范围与锁文件解析；仅传递依赖在备注中注明，不为记录版本新增直接依赖。

| 包名 | 当前声明或锁定版本 | 更新频率建议 | 最新版本来源 | 备注 |
|------|-------------|-------------|-------------|------|
| `tauri` | `2.12.4` | 跟随官方 Release Note | crates.io | 核心运行时，**必须与 CLI/API 对齐** |
| `tauri-build` | `2.7.3` | 与 `tauri` 同步检查 | crates.io | Build script 依赖，版本独立于 tauri core |
| `tauri-plugin-log` | `2.10.1` | 每季度检查 | crates.io | 日志插件 |
| `tauri-plugin-opener` | `2.7.2` | 每季度检查 | crates.io | 系统打开器插件 |
| `tauri-plugin-os` | `2.4.0` | 按平台信息需求检查 | crates.io | 与前端绑定配套，提供启动时注入的平台身份 |
| `serde` | `1` | 仅安全补丁 | crates.io | 序列化基石，极稳定 |
| `serde_json` | `1` | 仅安全补丁 | crates.io | — |
| `ts-rs` | `12.0.1` | 配置契约工具变更时检查 | crates.io | 可选 `config-export` feature；仅供 host 开发工具从 Rust 配置类型生成 TypeScript，移动端默认不启用 |
| `tokio` | `1` | 仅安全补丁 | crates.io | 异步运行时 |
| `symphonia` | `0.6.1` | 音频解码需求变更时检查 | crates.io / 官方文档 | 纯 Rust TTS 完整 WAV / MP3 解码；关闭默认特性，仅启用 `mp3`、`pcm`、`wav` 与 ID3 元数据；按文件元数据处理 gapless，不猜测裁剪静音 |
| `rubato` | `5.0.1` | 重采样需求变更时检查 | crates.io / 官方文档 | 纯 Rust FFT 抗混叠重采样至 24kHz，关闭默认特性并显式启用 `fft_resampler`；解码与重采样统一在可协作取消的 blocking worker 内执行 |
| `reqwest` | `0.12` | 每季度检查 | crates.io | HTTP 客户端，使用 `rustls-tls`；升级约束见 §4.6 |
| `sqlx` | `=0.9.0` | 每季度检查 | crates.io | SQLite 异步 ORM；与 rusqlite 共用 `libsqlite3-sys 0.37.0` |
| `rusqlite` | `=0.39.0` | 每季度检查 | crates.io | 同步 SQLite，启用 `bundled`；保留当前精确约束，不能单独升级到不兼容的 SQLite links 版本 |
| `tokio-tungstenite` | `0.30` | 每季度检查 | crates.io | WebSocket 客户端，保留 `rustls-tls-webpki-roots` |
| `axum` | `0.8.9` | 每季度检查 | crates.io | 仅作为 dev-dependency 提供 HTTP 测试服务，不启用 `ws` feature |
| `syntect` | `5.3.0` | 每半年检查 | crates.io | 语法高亮，体积敏感 |
| `pulldown-cmark` | `0.13.3` | 每半年检查 | crates.io | Markdown 解析 |
| `html5ever` / `markup5ever_rcdom` | `0.38` / `0.38` | 每半年检查 | crates.io | HTML 解析直接依赖 |
| `htmd` | `0.5.5` | 每半年检查 | crates.io | HTML 转 Markdown |
| `fancy-regex` | `0.16.2` | 每半年检查 | crates.io | 传递依赖，实际版本由 Cargo.lock 冻结 |
| `zstd` | `0.13` | 每半年检查 | crates.io | 压缩 |
| `zip` | `9.0.5` | 每半年检查 | crates.io | ZIP 处理，禁用默认 feature，仅启用 deflate |
| `office_oxide` | `0.1.15` | 每季度检查 | crates.io | 文档解析传递依赖，与 zip 9 共同检查 |
| `quinn-proto` | `0.11.19` | 仅安全补丁 | crates.io | 网络栈传递依赖，记录 Cargo.lock 解析结果 |
| `dashmap` | `6` | 每季度检查 | crates.io | 并发 HashMap |
| `lru` | `0.19.0` | 每半年检查 | crates.io | LRU 缓存 |
| `uuid` | `1` | 仅安全补丁 | crates.io | UUID 生成 |
| `chrono` | `0.4` | 仅安全补丁 | crates.io | 日期时间 |
| `base64` | `0.22` | 仅安全补丁 | crates.io | Base64 编解码 |
| `sha2` | `0.11` | 仅安全补丁 | crates.io | SHA-256；锁文件另保留传递依赖所需的 0.10.9 |
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
| `async-trait` | `0.1` | 仅安全补丁 | crates.io | 异步 trait |
| `libc` | `0.2` | 仅安全补丁 | crates.io | FFI C 库绑定 |
| `memmap2` | `0.9.11` | 仅安全补丁 | crates.io | 内存映射 |
| `pdf_oxide` | `0.3.77` | 每季度检查 | crates.io | 不可信 PDF 解析；禁用默认 feature |
| `encoding_rs` | `0.8` | 仅安全补丁 | crates.io | 编码转换 |
| `chardetng` | `0.1` | 每半年检查 | crates.io | 编码检测 |
| `jni` | `0.21` | 每季度检查 | crates.io | Android JNI 绑定 |

配置契约导出工具位于 `src-tauri/crates/config-export/`，直接编译生产配置类型与规则。修改这些定义后运行 `pnpm config:generate` 更新已提交的 `src/core/config/generated/`；`pnpm config:check` 只检查生成结果，不写文件，也不读取设备配置或凭据。`pnpm check` 当前按 `config:check → lint → vue-tsc --noEmit → tsc --noEmit --project tsconfig.node.json → cargo check --locked` 顺序执行，其中 Node 配置项目覆盖 `vite.config.ts`、`vitest.config.ts` 与 `uno.config.ts`。普通手机构建不执行导出。

### 2.2 前端层 (`package.json`)

#### Runtime 与前端构建生态

| 包名 | 当前 manifest 声明 | 更新频率建议 | 最新版本来源 | 备注 |
|------|-------------|-------------|-------------|------|
| `@tauri-apps/api` | `2.12.4` | 与 `tauri` crate 同步 | npm | 根应用与本地插件必须保持一致 |
| `@tauri-apps/plugin-opener` | `2.7.2` | 与 `tauri-plugin-opener` 同步 | npm | 插件前端绑定 |
| `@tauri-apps/plugin-os` | `2.4.0` | 与 `tauri-plugin-os` 同步 | npm | 仅由统一平台入口消费同步 `platform()` |
| `vue` | `^3.5.44` | 每月检查 | npm | 核心框架 |
| `vue-router` | `^5.4.0` | 每月检查 | npm | 路由 |
| `pinia` | `^4.1.0` | 每月检查 | npm | 状态管理；运行时 API 按 WebView 117 / WKWebView 26 基线核对 |
| `pinia-plugin-persistedstate` | `^4.7.1` | 每季度检查 | npm | 状态持久化 |
| `@vueuse/core` | `^15.0.0` | 每月检查 | npm | 组合式工具库 |
| `vite` | `^8.3.4` | 每月检查 | npm | 使用 Rolldown/Oxc，显式配置移动端构建目标 |
| `@vitejs/plugin-vue` | `^6.0.10` | 与 `vite` 同步 | npm | Vue Vite 插件 |
| `unocss` | `^66.11.0` | 每季度检查 | npm | 原子 CSS |
| `@unocss/*` | `^66.11.0` | 与 `unocss` 同步 | npm | 使用 `preset-wind3`，生态包统一版本 |
| `dompurify` | `^3.4.17` | 每季度检查 | npm | XSS 净化 |
| `katex` | `^0.18.10` | 每半年检查 | npm | LaTeX 渲染，与 Mermaid 的 0.18 范围配套；使用包内类型 |
| `mermaid` | `^12.2.0` | 每季度检查 | npm | 图表渲染；主版本受 WebView 兼容边界约束，见 §4.6 |
| `marked` | `^18.1.1` | 每季度检查 | npm | Markdown 解析 |
| `morphdom` | `^2.7.8` | 每半年检查 | npm | 增量 DOM 更新 |
| `@lucide/vue` | `^1.59.0` | 每月检查 | npm | Vue 图标组件 |
| `@xterm/xterm` / `@xterm/addon-fit` | `6.0.0` / `0.11.0` | 配套检查 | npm | 终端渲染与窗口适配 |
| `sortablejs` | `^1.15.7` | 每半年检查 | npm | 拖拽排序 |
| `vue-cropper` | `^1.1.4` | 每半年检查 | npm | 图像裁剪 |
| `date-fns` | `^4.4.0` | 每季度检查 | npm | 日期工具 |

#### Dev Dependencies

| 包名 | 当前 manifest 声明 | 更新频率建议 | 最新版本来源 | 备注 |
|------|-------------|-------------|-------------|------|
| `@tauri-apps/cli` | `2.12.3` | 与 `tauri` crate 同步 | npm | **必须与 Rust tauri 版本对齐** |
| `typescript` | `~6.0.3` | 每季度检查 | npm | TS 编译器；当前共享源码/测试项目显式声明 Vite 与 Node 类型 |
| `vue-tsc` | `^3.3.13` | 与 `vue`/`typescript` 同步 | npm | Vue 类型检查 |
| `eslint` | `^10.13.0` | 每季度检查 | npm | `eslint.config.mjs` flat config，由 `pnpm check` 调用 |
| `@eslint/js` | `^10.0.1` | 与 `eslint` 同步 | npm | JavaScript 推荐规则 |
| `@typescript-eslint/*` | `^8.71.2` | 与 `eslint`/`typescript` 同步 | npm | TS ESLint 规则与 parser |
| `eslint-plugin-vue` | `^10.11.1` | 与 `eslint`/`vue` 同步 | npm | Vue ESLint 规则 |
| `vue-eslint-parser` | `^10.4.1` | 与 `eslint-plugin-vue` 同步 | npm | Vue SFC 外层 parser，脚本交给 TS parser |
| `globals` | `^17.13.0` | 每季度检查 | npm | 分别声明浏览器与 Node 执行环境 |
| `prettier` | `^3.9.10` | 每季度检查 | npm | 代码格式化 |
| `eslint-config-prettier` | `^10.1.8` | 与 `prettier`/`eslint` 同步 | npm | Prettier 兼容配置 |
| `eslint-plugin-prettier` | `^5.5.7` | 与 `prettier`/`eslint` 同步 | npm | 当前 lint 不启用 prettier/prettier 格式规则 |
| `@types/node` | `^24.19.2` | 与 Node 主版本同步 | npm | 工具与测试使用的 Node 类型 |
| `vitest` / `@vitest/coverage-v8` | `^5.0.5` / `^5.0.5` | 同步检查 | npm | 前端测试与覆盖率 |
| `happy-dom` | `^21.0.3` | 与测试环境同步检查 | npm | DOM 测试环境；iframe fixture 明确使用无网络 `srcdoc` |
| `@vue/test-utils` | `^2.5.1` | 与 Vue 测试同步检查 | npm | Vue 组件测试 |
| `@pinia/testing` | `^2.0.2` | 与 Pinia 同步检查 | npm | 与 Pinia 4 配套 |
| `@vue/devtools-api` | `^8.2.1` | 与 Pinia 同步检查 | npm | 显式满足 Pinia 的开发工具 peer 依赖 |
| `@types/sortablejs` | `^1.15.9` | 与 `sortablejs` 同步 | npm | 类型定义 |
| `@iconify-json/ph` | `^1.2.2` | 每季度检查 | npm | Iconify 图标数据 |

> **注意**：前端非 Tauri 生态依赖使用 `^`/`~` 是当前 manifest 策略；可复现性由 committed `pnpm-lock.yaml` 与 frozen install 保证。不要只改前缀而不审查 lockfile 实际解析。

`pnpm lint` 执行 `eslint . --max-warnings 0`，覆盖 Vue/TypeScript、插件 guest-js 和受控工具脚本；忽略明确的生成树、本地报告及缓存。当前启用 JS/TS/Vue 推荐正确性规则，未启用需类型信息的运行型规则及显式 `any` 规则；隐式 `any` 由 TypeScript `strict` 检查。Prettier 仅通过兼容配置避开格式冲突；富文本与控制字符处理保留逐处说明，不全局关闭对应检查。

### 2.3 Android Gradle 层

| 包名 / 配置 | 当前锁定版本 | 更新频率建议 | 最新版本来源 | 备注 |
|------------|-------------|-------------|-------------|------|
| Android Gradle Plugin (AGP) | `8.11.0` | 每季度检查 | Google Maven | 与 Gradle Wrapper 版本耦合 |
| Kotlin Gradle Plugin | `1.9.25` | 每季度检查 | Maven Central | **必须与 Tauri Android 模板要求对齐** |
| `compileSdk` | `36` | 跟随 AGP / 每年 | Android SDK Manager | — |
| `targetSdk` | `36` | 跟随 `compileSdk` | Android SDK Manager | 必须与 `compileSdk` 一致 |
| `minSdk` | `26` | 仅业务需求驱动 | Android SDK Manager | `tauri.conf.json` 中同步声明 |
| Android NDK | `29.0.14206865` | 每年或跟随 Tauri 要求 | Android SDK Manager | Rust `aarch64-linux-android` 目标依赖 |
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
| Node.js | `24.14.1` (LTS) | 每年 major / 每季度 minor | nodejs.org | CI、Android Release 与 iOS 阶段诊断共享 `.github/toolchain.env` |
| pnpm | `12.6.0` | 每季度检查 | pnpm.io | 三条 workflow 共享 `.github/toolchain.env`，解析由 lockfile 冻结 |
| Rust Toolchain | `1.97.1` | 每季度检查 | rustup | 由 `rust-toolchain.toml` 固定完整版本 |
| Java (Temurin) | `17.0.20+8` | 每年 LTS | adoptium.net | CI/Release 固定完整版本，Android 构建必需 |
| Android command-line tools | `12266719` | 跟随 Android SDK 升级 | Android SDK | CI/Release 固定下载版本 |
| Gradle (Wrapper) | `8.14.3` | 跟随 AGP | Gradle 官方 | 官方分发 URL + `distributionSha256Sum` |

---

## 3. 更新规则与流程

### 3.1 通用前置检查

在任何依赖升级前，必须完成以下检查：

1. 检查 `git status`，识别并保留全部已有修改。按根 `AGENTS.md` 使用已有恢复点或一次范围明确的快照；不得为升级清空工作树，也不得未经授权提交。
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

升级 `@tauri-apps/cli` 时，同时按 [§6.3 的模板维护说明](#63-kotlin-版本与-tauri-模板)核对已提交的 Android 工程文件，将需要的模板迁移与依赖升级一起提交。

```powershell
# 1. 更新 pnpm lockfile
pnpm install

# 2. 配置契约 + ESLint + 前端类型 + Rust 编译检查
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
| `tauri` | `2.12.4` | `@tauri-apps/cli` | `2.12.3` | 主.次版本必须一致（`2.12.x`） |
| `tauri` | `2.12.4` | `@tauri-apps/api` | `2.12.4` | 主.次版本必须一致（`2.12.x`） |
| `tauri-build` | `2.7.3` | — | — | 独立版本，需与 `tauri` 兼容；本地插件 build helper `tauri-plugin` 为 2.7.2 |

iOS 插件的 SwiftPM `SwiftRs` 精确固定为 `1.0.9`，与 Cargo.lock 中 `swift-rs 1.0.9` 对齐；Cargo 锁文件本身不会约束 SwiftPM 解析。

### 4.2 Tauri 插件跨层对齐

| Rust 插件 | 当前版本 | npm 插件 | 当前版本 | 对齐规则 |
|----------|---------|---------|---------|---------|
| `tauri-plugin-opener` | `2.7.2` | `@tauri-apps/plugin-opener` | `2.7.2` | 版本号应完全一致 |
| `tauri-plugin-log` | `2.10.1` | — | — | 无前端包，Rust 单独升级 |

### 4.3 Rust 工具链与 MSRV

| 项目 | 当前值 | 约束来源 |
|------|--------|---------|
| Rust Toolchain | `1.97.1` | CI、Release 与本地开发环境 |
| Tauri MSRV | `1.90` | 本地插件 `rust-version` 保持此下限；随 Tauri 升级核对 |
| 依赖图 MSRV | 逐次按实际锁文件检查 | 使用仓库锁定工具链验证完整依赖图，不以单个依赖的下限代替 |
| Cargo Edition | `2021` | `src-tauri/Cargo.toml` |

> **检查方法**：运行 `rustc --version`，确认不低于 Tauri 官方要求的 MSRV。若 Tauri 升级后提高 MSRV，必须同步更新 CI（`release.yml`、`ci.yml`）中的 Rust 安装步骤。

### 4.4 Android SDK 对齐

| 配置项 | 当前值 | 声明位置 |
|--------|--------|---------|
| `compileSdk` | `36` | `app/build.gradle.kts` |
| `targetSdk` | `36` | `app/build.gradle.kts` |
| `minSdk` | `26` | `app/build.gradle.kts` + `tauri.conf.json` |
| `compilerOptions.jvmTarget` | `JVM_1_8` | app 与本地插件 `build.gradle.kts` |

**对齐规则**：`compileSdk == targetSdk`，且 `minSdk` 在 Gradle 与 `tauri.conf.json` 中双写一致。

### 4.5 安全审计

`pnpm audit:rust` 执行 `cargo audit --file src-tauri/Cargo.lock`，需要本机先安装 `cargo-audit`；npm 侧使用 `pnpm audit --audit-level=high`。当前命令不配置漏洞忽略参数。

审计结果以当次公告数据库与锁文件为准。沿目标平台、启用的 feature 和实际调用路径判断影响，分别处理漏洞、维护状态、内存安全和撤回版本警告；已解决项随依赖调整移除，不保留失效例外。

KaTeX 与 Mermaid 使用兼容的 0.18 范围，根依赖为 0.18.10；KaTeX 自带类型，不再安装旧 `@types/katex`。调整数学渲染依赖时同时核对 Mermaid 的范围、锁文件中的重复版本与公式样式，不用强制版本覆盖或审计忽略掩盖兼容问题；漏洞结论仍以实际审计结果为准。

### 4.6 兼容约束与验证要求

| 范围 | 当前约束 |
|------|----------|
| WebView | Android WebView / Chromium 最低 117，Android minSdk 26 保持不变；共享包的 JS/CSS 构建目标同步为 `chrome117` 与 `safari26.0`。Mermaid 12、Pinia 4 按此基线评估；语法转译不能补齐运行时内建 API，新依赖仍须核对实际生产路径。 |
| HTTP / TLS | 应用 HTTP 客户端保持 reqwest 0.12 的 TLS 提供方与信任根语义；调整前核对 Android 原生初始化需求及双端 HTTPS/WSS 行为。 |
| SQLite | SQLx 与 rusqlite 共享 `libsqlite3-sys` 的原生 links 约束，必须成组评估版本兼容性。 |
| 语音 runtime | 维持现有裁剪方案；需要升级时，联合评估模型、算子清单、运行库及 Android/iOS 制品。 |

验证按改动涉及的层次选择。host 静态检查、Android 目标编译、插件 AAR、签名 APK、Apple 构建和设备运行分别提供证据；插件库构建不能替代完整应用 Release 验证，源码核查不能替代 Apple 编译与链接。

图表升级需检查真实 SVG、双实例 ID、主题和图片导出，留意 C4 长关系文字重叠与暗色对比限制。终端升级需检查尺寸调整、输入与 OSC52 拦截；host 浏览器及合成 DOM 事件不替代最低支持的 WebView 117、手机 IME、真实 PTY 和触摸验证。

兼容基线与 CSS 规则以 [移动端 UI 兼容性规范](./MOBILE_UI_COMPATIBILITY.md) 为准。基线内已支持的 `color-mix()` 不再承担低于 WebView 117 的适配义务；现有主题回退无需批量移除。启动兼容说明复用“帮助与指引”，依赖应用运行时可用，不能作为低版本内核在模块加载失败前仍能显示的保障。

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
| CI (`ci.yml`) | `29.0.14206865` | `.github/toolchain.env`；安装后通过 `GITHUB_ENV` 设置 `NDK_HOME`，供初始化及后续步骤使用 |
| Release (`release.yml`) | `29.0.14206865` | `.github/toolchain.env`（由 workflow 读取） |
| Android 原生插件 | `29.0.14206865` | `src-tauri/plugins/vcp-mobile/android/build.gradle.kts` |
| 本地开发 | `29.0.14206865` | Android SDK 的 `ndk/29.0.14206865/` |

随包分发的 PRoot 二进制保留独立的构建来源记录和精确复现脚本；其中的历史 NDK 版本不作为应用当前构建版本。更新这些记录须先重建二进制并核对哈希。

**规则**：

- CI 与本地 NDK 版本应尽量一致。若 CI 升级 NDK，必须在团队内广播。
- NDK 升级后，必须重新编译 Rust 标准库与依赖：`cargo clean` 后重新构建。
- NDK 版本与 `rustc` 的目标 `aarch64-linux-android` 存在隐性兼容关系，升级前查阅 Rust Android 社区反馈。

### 6.3 Kotlin 版本与 Tauri 模板

`src-tauri/gen/android/` 中已有文件进入 Git，升级 Tauri CLI 时须核对新版本模板，尤其是配套的 [BuildTask.kt](../src-tauri/gen/android/buildSrc/src/main/java/com/vcp/avatar/kotlin/BuildTask.kt) 与 [RustPlugin.kt](../src-tauri/gen/android/buildSrc/src/main/java/com/vcp/avatar/kotlin/RustPlugin.kt)。按实际差异迁移，并保留 MainActivity、Manifest 和 Gradle 配置中的项目定制。

不要把重新执行 `tauri android init` 当作完整模板升级：例如 [CLI 2.12.1 的生成逻辑](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.12.1/crates/tauri-cli/src/mobile/android/project.rs#L189)会覆盖 `BuildTask.kt`，却保留已有的 `RustPlugin.kt` 等文件。CI 生成树无差异也不能证明这些保留文件已完成配套迁移。

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

- `gradle-wrapper.properties` 固定官方 Gradle 8.14.3 分发与官方 SHA-256；wrapper JAR/脚本由该版本官方 Wrapper 任务生成，Release 构建前核对官方 wrapper JAR SHA-256。
- CI 在 `tauri android init --ci` 后检查 Android 生成树与插件权限生成树无漂移，并运行 Gradle JVM 测试。
- Release 核对 tag/HEAD/event SHA、四处版本源和 Android versionCode；构建产物经固定版本（37.0.0）build-tools 验签。
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
3. 必须经过 **Magi 三贤者协议**快速评审（见 `AGENTS.md` 第 9.2 节）：
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

# Rust 安全审计（当前无漏洞忽略参数，warning 保持可见）
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
| Tauri CLI 工程模板 | `src-tauri/gen/android/` 中已提交的工程文件，尤其是配套的 `BuildTask.kt` / `RustPlugin.kt`；见 [§6.3](#63-kotlin-版本与-tauri-模板) |
| AGP / Kotlin / Gradle 供应链 | `src-tauri/gen/android/build.gradle.kts`, `src-tauri/gen/android/buildSrc/build.gradle.kts`, `src-tauri/gen/android/gradle/wrapper/` |
| AndroidX | `src-tauri/gen/android/app/build.gradle.kts` |
| SDK / NDK | `src-tauri/gen/android/app/build.gradle.kts`, `.github/workflows/release.yml` |
| Tauri 配置 | `src-tauri/tauri.conf.json` |
| CI 环境 | `.github/workflows/ci.yml`, `.github/workflows/release.yml` |

---

*文档版本：1.0*  
*最后更新：2026-10-20*\
*维护者：全体贡献者（修改前必读第 5 节红线）*
