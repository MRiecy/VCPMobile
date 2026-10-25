# VCP Mobile — TTS 三级流水线调度、多格式解码与双端本地缓存架构设计方案

> **状态**：三级流水线与本轮审阅优化已实现；静态检查、Rust 回归和 Android arm64 编译检查通过，待真机听感验证
>
> **日期**：2026-10-25
>
> **关联模块**：[`src-tauri/src/vcp_modules/chat/voice_service/`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/mod.rs)、[`src-tauri/src/vcp_modules/infra/file_manager.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/infra/file_manager.rs)
>
> **前置方案**：[`plan/tts.md`](file:///e:/VCPMobile/plan/tts.md)、[`plan/tts-fix.md`](file:///e:/VCPMobile/plan/tts-fix.md)

---

## 零、 架构决策锁定表（Executive Decisions）

| 决策项 | 锁定规格 | 依据与目的 |
| :--- | :--- | :--- |
| **D1. 流水线解耦与预取窗口** | 拆分为「分段提取 → 合成与缓存 Worker（并发度=1）→ 播放消费 Worker」，预取窗口 **$K = 2$** | 当前段 $S_0$ 向原生供包期间，后台推进 $S_1, S_2$；取得预取槽位之后才能取片、查缓存或合成，避免网络接收等待播放器背压 |
| **D2. 中断与取消策略** | **立即掐断（Immediate Drop）并由所有者收束工作** | 任意终止动作立即取消在途 HTTP、缓存发布退避与后续供包；解码和文件操作由所属任务等待收尾，未成功提交的文件清理，已原子提交的完整缓存保留 |
| **D3. 分段切分算法统一** | 移除 [`ReplyBuffer::policy`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/segmentation.rs#L116-L130) 对 `streaming_tts` 的分支依赖 | 业务逻辑（流式/非流式传输开关）不得改变文本分段切分算法，确保同一正文在任意传输模式下切出完全相同的 `ReplyPiece` 序列，保障缓存 100% 互通命中 |
| **D4. 缓存寻址 Key 规范** | 请求与缓存共用规范化参数，采用带字段标签及长度前缀的 SHA-256 | 模型与音色分字段；`None` 与空指导按实际请求行为归一，正文原样编码；当前不可变样本 ID 直接参与身份，不另做样本内容哈希 |
| **D5. 音频格式接收与落盘** | 非流式完整 **WAV / MP3**，流式仅 **SSE PCM16** | JSON Base64 或直接音频响应按真实格式保存；流式 PCM16 正常完结后封装 `.wav`。完整音频在 Rust `spawn_blocking` 内归一为 24kHz 单声道 PCM16；符合该规格的 PCM16 WAV 有界解析后直接复制 PCM，其他 WAV / MP3 使用解码库。本轮不实现压缩格式增量流式解码 |
| **D6. 适用范围隔离** | `Chat`（通话回复）与 `Bubble`（气泡朗读）启用缓存与 $K=2$ 预取；**`Preview`（试听）不走缓存** | 试听为设置页独立调试功能，直接合成直通播放，不读盘、不落盘 |
| **D7. 双端存储位置与清理** | 存于持久数据目录 `get_data_root_dir(app)?.join("tts_cache")`，**暂不设后台自动淘汰或管理 UI** | 不登记为附件，不进入配置备份或业务同步；首次孤儿清理先于本进程首次写入 |
| **D8. 服务范围** | 通用流水线、缓存与音频基础设施，保留当前 MiMo 请求协议 | 不新增原生 GPT-SoVITS、provider 选择或其他服务请求适配；本地模型仅作为未来扩展背景 |
| **D9. 请求错误立即终止** | **所有 TTS 网络请求均不自动重试** | 连接、超时、HTTP 或流式响应错误首次发生即终止本次 TTS，取消播放与后续预取；例如 $S_2$ 只请求一次，失败后不请求 $S_3$，不等待队列中音频播完 |
| **D10. 缓存失败隔离** | 缓存 IO 失败不终止有效音频播放；`rename` 按 **50 / 100 / 200 ms** 快速退避 | 只重试同一个 `.pending` 文件；耗尽后清理并放弃本段缓存，朗读继续 |

这里的「切片」「分段」均指一个 `ReplyPiece`，不是整条消息，也不是传输中的 PCM 小包。网络错误统一终止，不分类为可重试错误、不重置分段、不跳过失败切片；ASR 保持无自动重试。保留的快速重试仅用于本地缓存文件的原子 `rename`，不产生新网络请求。

---

## 一、 现状瓶颈复盘：为什么播放背压会拖死网络合成？

### 1.1 重构前的单协程串行耦合链
本节记录重构前的瓶颈：当时 [`tts.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/tts.rs) 的 `speak_inner` 在同一异步任务内依次执行合成与供包：

```text
loop {
    1. 从 NarrationInput 取出一段 ReplyPiece::Text(piece)
    2. 调用 network::synthesize(..., |pcm_packet| async {
           append_playback(pcm_packet).await // <-- 直接等待原生播放器队列接纳！
       }).await
    3. 只有当本段所有 PCM 包全部进入原生队列且 HTTP 流结束，才进入下一轮循环请求下一段
}
```

### 1.2 原生播放器背压传导链
1. Android 侧 [`VoicePlaybackQueue`](file:///e:/VCPMobile/src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/voice/VoicePlaybackPcm.kt#L21) 容量为 **`4 个已入队包 + 1 个 waiting 包`**（每包最大 32,768 字节，24kHz 单声道 PCM16 约 683 ms；SSE 流式小包可能仅 100 ms）。
2. [`VoiceStreamPlayback.kt`](file:///e:/VCPMobile/src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/voice/VoiceStreamPlayback.kt#L120-L133) 的播放线程以每次最多 480 采样点（20 ms）非阻塞写入 `AudioTrack`，受硬件真实播放速度（$1.0\times$ 实时率）严格约束。
3. 当第 5 个包调用 `append_playback().await` 时，Kotlin 保存 `waiting` 并不立即完成 `Invoke`；Rust 侧 [`synthesize`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/network.rs#L248) 挂起在 `output(packet).await` 上，既停止读取后续网络数据，也无法返回以触发下一段 HTTP 请求。

### 1.3 面向本地 SoVITS 模型时的致命缺陷
- **段间断音（Inter-segment Underrun）**：假设 $S_0$ 有 6 秒音频，前 4 包（约 2.7 秒）入队后，Rust 被卡住約 3.3 秒直到最后 1 包入队才返回并发起 $S_1$ 的 HTTP 请求。此时原生队列只剩最后几百毫秒音频，若本地 SoVITS 合成 $S_1$ 需要 2.5 秒，中间必出现长达 ~2 秒的死寂断音。
- **重复算力消耗**：通话中刚听过的回复，退出通话后点气泡重听，或同一气泡反复点击朗读，每次都要全量重新请求推理。

---

## 二、 Magi 三贤者架构审议

- **Melchior（逻辑与系统）**：
  - **内存与连接边界**：解耦合成与播放不能变成无界 `mpsc::unbounded_channel`。必须以**分段槽位（Segment Slot）**为单位做有界背压：当前供包段 $S_n$ + 最多 $K=2$ 个预取段（$S_{n+1}, S_{n+2}$）。单段 PCM 上限为 **8 MiB**，编码文件上限为 **16 MiB**；检查贯穿接收、解码和重采样，避免解码完才发现超限。段内共享缓冲及读游标按字节管理，避免很多小 SSE 包提前耗尽消息计数容量，也避免为播放和缓存各复制一份完整 PCM。
  - **R3 & R6 不变量**：MP3 解码、WAV 解析、采样率转换属于 CPU 密集型任务，必须放入 `tokio::task::spawn_blocking` 并支持 `CancellationToken` 协作中断；`TtsServiceState` 的单槽位注册表与 `flush_player` 原生墓碑确认机制原样保留。
- **Balthasar（直觉与听觉体验）**：
  - **跨段连续供包**：磁盘缓存保存 `.wav` 或 `.mp3`，播放时统一解包为 24kHz 单声道 PCM16，推入同一个 `VoiceStreamPlayback` 会话，不按文件重建原生播放器。MP3 的编码延迟与末尾补零依据 gapless 元数据裁剪，不通过猜测静音长度删改音频。
- **Casper（务实与工程尺度）**：
  - **最小侵入改造**：保留现有前端 IPC 契约（`start_tts_session`、`submit_tts_text`、`preview_tts`、`stop_tts` 及 `vcp-tts` 事件）与原生插件接口（`startVoicePlayback`、`appendVoicePlayback`、`finishVoicePlayback`、`stopVoicePlayback`）零改动，将全部重构收敛在 `src-tauri/src/vcp_modules/chat/voice_service/` 内部与 `file_manager.rs` 路径导出。

---

## 三、 模块一：分段切分算法统一（`segmentation.rs`）

### 3.1 问题定位
重构前的 [`ReplyBuffer`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/segmentation.rs) 持有 `streaming_tts: bool` 字段，并在 `ReplyBuffer::policy` 中区分两套窗口：

```rust
// 重构前：流式与非流式切分出的段落文本不一致
match (self.emitted_segments, self.streaming_tts) {
    (0, _) => (10, 24, 60),
    (1, true) => (28, 64, 140),
    (_, true) => (40, 90, 160),
    (1, false) => (60, 100, 150),
    (_, false) => (120, 160, 220),
}
```

如果保留该分支，用户在流式模式下听过的消息，切换为非流式（或切到本地非流式 SoVITS）后，由于第 1 段起切分点不同，后续所有分段的 `SegmentKey` 全部失配。

### 3.2 改造方案
1. **统一阶梯策略**：移除 `ReplyBuffer`、`MessageSpeech`、`ChatFeed` 中的 `streaming_tts: bool` 参数，统一使用一套兼顾首包响应速度与中后段语义完整度的阶梯阈值：
   ```rust
   fn policy(&self) -> ChunkPolicy {
       let (min_sentence_chars, clause_cut_chars, max_chars) = match self.emitted_segments {
           0 => (10, 24, 60),   // 首段：极速出声，降低首包等待（TTFA）
           1 => (40, 80, 150),  // 次段：平滑过渡，为 K=2 预取建立安全时间窗
           _ => (80, 120, 180), // 稳定段：兼顾长句语调连贯性与单次推理耗时
       };
       ChunkPolicy {
           min_sentence_chars,
           clause_cut_chars,
           max_chars,
       }
   }
   ```
2. **构造函数收敛**：
   - `ReplyBuffer::new_with_mode(inherit_global_style: bool, streaming_tts: bool)` 简化为 `ReplyBuffer::new(inherit_global_style: bool)`。
   - `MessageSpeech::new()` 与 `ChatFeed::new()` 不再接收 `streaming_tts` 参数。

---

## 四、 模块二：双端持久缓存与内容寻址设计（`cache.rs`）

### 4.1 双端物理目录设计
在 [`file_manager.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/infra/file_manager.rs) 中新增权威路径解析函数：

```rust
/// 核心路径解析：获取语音合成（TTS）持久化缓存根目录
/// - Android: /storage/emulated/0/Android/data/com.vcp.avatar/files/tts_cache
/// - iOS: Library/Application Support/<id>/tts_cache
/// 位于应用持久数据目录，不受系统 cacheDir / Library/Caches 自动回收影响，由用户手动管理。
pub fn get_tts_cache_root_dir<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Result<std::path::PathBuf, String> {
    let mut path = get_data_root_dir(app_handle)?;
    path.push("tts_cache");
    Ok(path)
}
```

### 4.2 规范化缓存键（`TtsCacheKey`）与 `Option` 参数支持
缓存身份从本次任务的不可变合成配置快照生成，与网络请求共用同一套规范化参数；不重新读取可能已经改变的设置。采用**带字段标签与长度前缀的二进制 SHA-256**，区分字段边界与可选值：

```rust
pub(super) struct TtsCacheIdentity<'a> {
    /// 缓存协议版本，固定为 b"vcp-tts-cache-v1"
    pub endpoint: &'a str,            // 规范化后的 endpoint URL（区分不同后端/本地服务地址）
    pub mode: VoiceTtsMode,           // Builtin / Clone
    pub model: &'a str,               // 实际请求使用的模型
    pub voice: Option<&'a str>,       // 实际请求使用的内置音色；未发送则为 None
    pub clone_sample_id: Option<&'a str>,// Clone 时为 Some(sample_id)，否则为 None
    pub guidance: Option<&'a str>,    // 请求实际使用的指导，None 与空白指导等价
    pub text: &'a str,                // 分段清洗后的实际朗读正文 piece
}
```

**哈希编码规则（`compute_segment_key`）**：
```rust
fn write_opt_field(hasher: &mut Sha256, tag: u8, value: Option<&str>) {
    hasher.update([tag]);
    match value {
        Some(v) => {
            hasher.update([1u8]);
            hasher.update((v.len() as u64).to_le_bytes());
            hasher.update(v.as_bytes());
        }
        None => {
            hasher.update([0u8]);
        }
    }
}
```
- 生成 64 位小写十六进制字符串 `<hex64>`。
- `write_opt_field` 只编码已规范化的参数，不擅自修剪每个字段。指导的空值归一与 trim 与请求构造共用；正文按实际合成文本原样编码，不做额外 trim 或折叠空白。模型、音色、复刻样本 ID 均为独立字段。
- `clone_sample_id` 使用现有不可变样本 ID；不增加样本内容哈希或迁移流程。API Key 不进入身份或文件名。
- **注意**：`tts_stream`（流式/非流式开关）与期望封装格式**不进入 Key**。这意味着流式下合成落盘的 `<hex64>.wav`，在切换为非流式后依然能直接命中并播放；反之，上游返回并落盘的 `<hex64>.mp3` 也能在任意模式下直接命中。

### 4.3 磁盘文件布局与双格式后缀（`.wav` / `.mp3`）
在 `tts_cache/` 目录下采用扁平或两级前缀目录（推荐直接扁平存放 `<hex64>.wav` 或 `<hex64>.mp3`，与现有 `attachments/`、`thumbnails/` 风格一致）：

```text
tts_cache/
├── a1b2c3...90.wav                  # 非流式 WAV 响应，或流式 PCM16 完结封装的标准 WAV
├── d4e5f6...12.mp3                  # 上游返回的 MP3 响应
└── .pending-550e8400-e29b...tmp     # 正在写入的临时文件（原子 rename 前）
```

1. **缓存查找（`lookup_cached_segment`）**：
   - 按顺序检查 `root.join(format!("{key}.wav"))` 与 `root.join(format!("{key}.mp3"))`。
   - 校验文件非空且不超过 16 MiB；格式嗅探只用于选择解析路径，必须完成容器验证、PCM 直接提取或解码，并检查 8 MiB PCM 上限后才能交付播放器。
   - 明确的损坏、空文件或后缀与实际容器错配才可清理；资源不足、解码 worker 失败、不支持的格式、大小超限和取消都不能证明磁盘内容损坏，不删除原文件。缓存读取、解码或删除失败可按未命中继续合成，任务取消则向上返回取消。
   - 查找仅在同 key 的短临界区内打开文件并捕获当前 `Arc` 代际标识，文件读取与解码不持锁。若旧读取稍后判断损坏，必须重新取得同 key 锁并确认代际未改变，才可删除，避免旧解码结果删掉已发布的新文件。
2. **原子落盘（`commit_segment_cache`）**：
   - **铁律**：绝不向正式路径 `<key>.wav` / `<key>.mp3` 执行流式追加写。
   - 完结数据准备好后：
     1. 若数据来源是流式 `pcm16`，先在内存或写入流头部构造标准 44 字节 RIFF WAVE 头（采样率 24000、单声道、16-bit LE，`data` 块长度 = `pcm.len()`）。
     2. 写入 `tts_cache/.pending-<uuid>.tmp`。
     3. 调用 `tokio::fs::rename(&pending, &final_path).await` 原子发布为 `<key>.wav` 或 `<key>.mp3`。
     4. `rename` 失败时对同一个 `.pending` 文件按 **50 / 100 / 200 ms** 等待后重试，退避可被取消。写入失败或重试耗尽则由提交所有者清理临时文件、记录错误并放弃本段缓存，已有音频继续播放。
     5. 文件提交任务持有文件和路径的完整所有权。取消发生时先等待已经发起的文件操作收束，再按最终结果保留已提交缓存或删除临时文件；丢弃异步 future 不能代替 IO 收尾。
   - 每次提交使用独立 `.pending` 路径，写入与退避不持根锁或 key 锁。只在同 key 的原子发布临界区内执行 `rename`，成功后更新 `Arc` 代际标识；每轮失败后释放锁再等待。
3. **启动期孤儿临时文件清理**：
   - 首次访问 `tts_cache` 时异步清理上次进程崩溃遗留的 `.pending-*.tmp`，并在清理完成前禁止本进程向该目录新建临时文件；不触碰已提交的 `.wav` / `.mp3` 正式缓存。
   - 根级状态只协调首次恢复，不覆盖日常查找、解码、写入或发布。per-key 注册表使用弱引用，只服务并发活跃操作；操作结束后不继续保存每个历史磁盘文件的内存条目。
4. **存储边界**：
   - 缓存不登记附件，不进入配置备份与业务同步。本轮不做自动淘汰、容量管理 UI 或手动清理入口。

---

## 五、 模块三：多格式音频接收、解码与 24kHz 归一化（`audio_codec.rs`）

### 5.1 上游音频格式兼容矩阵
请求仍采用当前 MiMo 协议；响应处理支持完整 WAV / MP3 与 SSE PCM16。直接二进制响应纳入通用接收边界，不因此新增服务类型或原生 SoVITS 请求协议：

| 传输模式 | 上游返回内容 | 内容识别方式 | 落盘后缀 | 播放消费前处理 |
| :--- | :--- | :--- | :--- | :--- |
| **非流式 JSON / 二进制** | Base64 编码的 WAV / MP3，或直接返回完整音频字节 | 格式嗅探后完整解析验证 | `.wav` 或 `.mp3` | 24kHz 单声道 PCM16 WAV 直接提取 PCM；其他 WAV / MP3 解码、混音、按需重采样；缓存保留原容器字节 |
| **流式 SSE (`pcm16`)** | 每个 delta 为 24kHz 单声道 PCM16 Base64 块 | 请求固定为 `pcm16` | 正常完结后封装标准 WAV 存为 `.wav` | PCM 写入按字节限额的共享分段缓冲，Consumer 按读游标切出不超过 32KB 的包供原生播放 |

本轮不支持跨网络分片的 WAV / MP3 增量解封装、解码或重采样。网络分片不等于独立音频文件；后续需要此能力时，在音频解码边界维护整段解码状态。

### 5.2 纯 Rust 解码与采样率归一化（遵循 R3 不变量）
为了在不阻塞 Tokio 异步线程、不引入跨 JNI MediaCodec 复杂状态机的前提下支持 `.mp3` 解码与任意采样率 `.wav`（如 SoVITS 的 32kHz）：
1. **依赖与职责**：在 `src-tauri/Cargo.toml` 引入裁剪特性的纯 Rust Symphonia 处理通用 WAV / MP3 解码，Rubato 负责带抗混叠滤波的重采样；标准 24kHz 单声道 PCM16 WAV 走直接提取路径。实际版本与特性以 Cargo 配置及 lockfile 为准。host、Android 与 Apple 构建结果分别记录。
2. **统一归一化函数（在 `spawn_blocking` 中执行）**：
   ```rust
   pub(super) enum ContainerFormat {
       Wav,
       Mp3,
   }

   pub(super) struct DecodedAudio {
       pub format: ContainerFormat,
       pub pcm_24k_mono: Vec<u8>, // 严格对齐的 24kHz 单声道 PCM16 LE 字节流
   }
   ```
   - **PCM16 WAV 快速路径**：先有界扫描 RIFF/WAVE chunk，检查长度、对齐、`fmt ` 与 `data`，不假设所有输入均为固定 44 字节文件头。确认 24kHz、单声道、PCM16 后直接复制有效 PCM 字节，不经过 Symphonia 或 `i16 → f32 → i16` 转换；其他受支持 WAV 交给 Symphonia 与按需重采样。
   - **MP3 解析职责**：删除手写 `validate_mp3` 逐帧校验器，由 Symphonia 负责容器与帧解析，接受其支持的标准元数据和 padding。HTTP 响应接收完整只代表传输完整，不能代替音频解析、有效样本及大小边界检查；不声明支持解码库之外的任意标签或损坏修复。
   - **声道折叠**：若上游返回双声道（Stereo），自动均值混音为单声道（Mono）。
   - **采样率对齐**：源采样率为 `24000 Hz` 时跳过重采样；`16000 / 32000 / 44100 / 48000 Hz` 等受支持输入经带抗混叠滤波的重采样器转为 `24000 Hz`。音频样本最终统一为单声道 PCM16 LE。
   - **大小与工作区边界**：编码输入上限 16 MiB、最终 PCM 上限 8 MiB；解码逐帧、混音与重采样逐块检查，并限制中间工作区，不先把整个输入展开为无界浮点样本数组。共享 PCM、编码原文件、解码工作区分开计入内存预算，不能只用三个槽位乘 PCM 上限估算整个任务内存。
   - **所有权与拷贝边界**：网络收到的完整原容器使用 `Bytes` 共享给解码与缓存提交，避免对最多 16 MiB 的文件执行深拷贝。完整解码所得 PCM 作为一个 `Vec<u8>` 交付；空 `SegmentBuffer` 直接接管该数组，原生消费适配器才按不超过 32KB 切包，试听直通播放共用这一供包边界。
   - **MP3 补零处理**：使用解码库的 gapless 元数据处理编码延迟与尾部 padding，不猜测并删除静音。
   - **协作式取消**：解码、重采样循环检查 `cancel.is_cancelled()`；异步调用方持有并等待 `spawn_blocking` 的任务句柄收尾，不能只丢弃等待 future。
   - **ASR 隔离**：保留现有 `wav.rs` 的严格 ASR 解码入口；TTS 多格式解析新增在 `audio_codec.rs`，不放宽 ASR 的输入协议。

---

## 六、 模块四：三级流水线与 $K=2$ 预取调度器（`pipeline.rs`、`tts.rs` 与 `network.rs`）

### 6.1 核心拓扑重构
在 `pipeline.rs` 中拆分合成生产与播放消费，通过**有界分段队列（预取容量 = $K = 2$）**协作。`tts.rs` 保留任务生命周期与原生播放适配；父任务持有取消令牌并等待 Producer、Consumer、解码与文件提交收尾。不能用提前返回的 `try_join!` 丢弃仍有文件操作或 `spawn_blocking` 工作的另一侧：

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│                                  TtsJob                                      │
│                                                                              │
│  ┌─────────────────────┐    segment_tx (cap=2)    ┌───────────────────────┐  │
│  │  1. SynthProducer   │ ───────────────────────► │  2. PlaybackConsumer  │  │
│  │  (合成与缓存调度器)   │    PreparedSegment       │  (原生播放器供包协程)   │  │
│  └──────────┬──────────┘                          └───────────┬───────────┘  │
│             │                                                 │              │
│   ┌─────────┴─────────┐                                       ▼              │
│   ▼                   ▼                             append_playback().await  │
│ 本地磁盘缓存       MiMo HTTP 合成                     (受 AudioTrack 4+1 背压) │
│ (tts_cache/)   (In-flight 并发 = 1)                                          │
└──────────────────────────────────────────────────────────────────────────────┘
```

### 6.2 分段传递契约与所有权
为什么队列元素是「分段（Segment）」而不是散装的「PCM 小包」？
因为如果把所有分段的 PCM 小包混进同一个通道，就无法控制「预取 $K=2$ 个分段」的边界。因此，外层队列承载分段句柄，内层使用共享 PCM 缓冲、读游标和唤醒通知：

- **分段槽位**：在取出下一个 `ReplyPiece`、读取缓存和开始网络请求之前预留；缓存命中与网络合成都计入同一个 $K=2$ 窗口。
- **段内缓冲**：上限为完整合法分段的 8 MiB PCM，不以 SSE 包数量限制容量。Consumer 从共享字节缓冲按游标读取；网络不等待原生消费速度，也不为缓存另复制一整份 PCM。
- **分段状态**：每个切片只有一次生产过程，记录可读 PCM、完成或失败状态；不再保留网络尝试编号、认领与重置互斥机制。网络错误直接终止任务，不复用失败分段重新生产。
- **完成职责**：`prepare_segment` 独占 `segment.finish()`：缓存命中在交付完整 PCM 后完成，网络成功在音频已校验且全部交付后立即完成，然后才执行缓存 IO。`pipeline::run` 只校验 prepare 返回时已完成，不再次调用 `finish()`；缓存写入或 `rename` 退避不能延迟 Consumer 得知本段音频已收齐。
- **Consumer 所有权**：只有 Consumer 调用原生 `start / append / finish`，并驱动首次进入 `speaking`。Producer 只准备数据，预取不能把界面重新切回“正在合成”。
- **正常终态**：所有切片消费完且原生播放完成后才结束朗读；网络完结、缓存提交成功、分段缓冲关闭都不代表扬声器已经播完。

### 6.3 $K=2$ 预取流水线时序推演

以一段包含 $S_0, S_1, S_2, S_3$ 四个分段的消息为例，`segment_tx` 的通道容量设为 **`2`**：

#### 场景 A：非流式与缓存混合
1. **`SynthProducer` 处理 $S_0$**：
   - 先保留一个分段槽位，再取 $S_0$，创建并交付共享分段句柄；随后查询 `tts_cache`。命中则解析并交付完整 PCM、标记分段完成；未命中则 HTTP 合成、解析并交付完整 PCM、标记分段完成，最后提交完整缓存。
   - 缓存 IO 失败不影响有效 PCM 的交付，也不延迟本段完成通知。
2. **`PlaybackConsumer` 取出 $S_0$**：
   - 此时 `segment_tx` 缓冲队列变为空（`len = 0`），`PlaybackConsumer` 开始逐包调用 `append_playback().await` 喂给 Android `AudioTrack`（耗时数秒）。
3. **在 $S_0$ 向原生供包期间，`SynthProducer` 预取 $S_1, S_2$**：
   - 不等待 $S_0$ 播完，取得下一个槽位后拉取 $S_1$ $\rightarrow$ 交付句柄并合成 $\rightarrow$ 交付音频、标记完成 $\rightarrow$ 落盘。
   - 再取得一个槽位后拉取 $S_2$，按相同顺序准备音频并落盘。
   - $K=2$ 槽位耗尽后，**必须在拉取 $S_3$ 之前阻塞**，不能先合成 $S_3$ 再等待发送，否则实际上已提前了三个切片。
4. **滑动窗口推进**：
   - `PlaybackConsumer` 将 $S_0$ 的最后一个包成功交给原生队列后，取出 $S_1$ 继续供包；空出的槽位允许 Producer 拉取 $S_3$。窗口依据 Rust 向原生供包完成推进，不等待 $S_0$ 实际从扬声器播完，也不新增逐段播放进度接口。

#### 场景 B：流式 SSE PCM16
1. **`SynthProducer` 处理 $S_0$（未命中缓存）**：
   - 预留槽位后创建共享分段缓冲并交付句柄；Producer 接收 SSE PCM16，Consumer 等待可读音频。
   - 第一个可读 PCM 包就绪后，Consumer 读取并供给原生播放器，无需等待整段合成完毕。
   - **关键改进**：段内缓冲可容纳完整的合法切片，因此 Producer 接收 $S_0$ 不再等待原生 `append_playback` 的消费速度；很多小 SSE 包也不会提前耗尽计数容量。
   - 当 $S_0$ 的 SSE 流在 0.8 秒内接收完毕，而播放器才播到 $S_0$ 的第 0.5 秒（总长 5 秒）时：
     - `SynthProducer` 标记分段正常完结，将完整 PCM 封装 WAV 并原子提交；只有正常完结的音频可保存缓存。
     - Consumer 继续从共享缓冲读取剩余音频，Producer 则在取得槽位后继续请求、接收和缓存 $S_1$、$S_2$。

#### 场景 C：预取切片首次错误即终止
1. $S_0$ 正在供包，$S_1$ 等待消费，预取 $S_2$ 的请求失败：$S_2$ 不重新请求，也不进入网络退避；失败向父任务传播，取消 Consumer 与后续预取。
2. 原生播放器立即停止并清空待播放音频，不等待 $S_0/S_1$ 播完；$S_3$ 不发起请求。已经原子提交的完整 $S_0/S_1$ 缓存继续保留，$S_2$ 未完结内容不提交。
3. 当前供包段流式中断同样立即失败，不因是否已经送播而改变策略。配置、鉴权、格式/大小限制、解码和原生播放错误也不重试；ASR 保持原行为。

### 6.4 终止与立即掐断（Immediate Drop）闭环
当发生任何终止事件（用户点停止、切换气泡、开启通话、删除消息、通话 VAD 打断）时：
1. `narration.cancel.cancel()` 被触发。
2. **`SynthProducer` 侧**：
   - `tokio::select!` 监听取消，停止读取并丢弃在途 HTTP 请求或 SSE 响应，停止新的请求与缓存发布退避等待；服务端是否立即停止推理由服务端处理客户端取消的能力决定。
   - 未完结音频不提交缓存。解码任务协作退出并等待句柄；文件提交所有者等待已发起的写入/重命名收尾，成功提交的完整文件保留，未提交临时文件清理。不能依赖析构执行异步 `remove_file`。
   - 已完整合成且原子提交的分段（如 $S_0$ 或 $S_1$）保留，下次点播复用。
3. **`PlaybackConsumer` 侧**：
   - `playback_call(&narration.cancel, ...)` 立即返回 `Err(CANCELLED)`。
   - 外层 `speak()` 走既有的 `flush_player(&app, &narration).await` 通知原生层 `stopVoicePlayback` 清空硬件 `AudioTrack` 缓冲，确认原生墓碑后调用 `retire(&app, &narration)` 释放槽位。
4. **父任务收束**：网络、解码与文件提交各自保有所有者，最终等待双方退出；失败或取消不能留下后台任务继续修改已退休任务的状态。

---

## 七、 分模块代码改造与文件清单

| 路径 | 动作 | 核心改造内容 |
| :--- | :--- | :--- |
| [`src-tauri/Cargo.toml`](file:///e:/VCPMobile/src-tauri/Cargo.toml)、`src-tauri/Cargo.lock` | 修改 | 裁剪特性的 Symphonia 解码与 Rubato 重采样依赖 |
| `docs/DEPENDENCY_MANAGEMENT.md` | 修改 | 同步新增音频依赖的用途与职责 |
| [`src-tauri/src/vcp_modules/infra/file_manager.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/infra/file_manager.rs) | 修改 | 新增并导出 `get_tts_cache_root_dir(app)`，指向双端持久数据目录下的 `tts_cache` |
| [`src-tauri/src/vcp_modules/chat/voice_service/segmentation.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/segmentation.rs) | 修改 | 统一 [`ReplyBuffer::policy`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/segmentation.rs#L116-L130) 分段窗口，移除 `streaming_tts` 对切分边界的影响；更新关联单测 |
| [`src-tauri/src/vcp_modules/chat/voice_service/chat_feed.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/chat_feed.rs) | 修改 | 同步移除 `MessageSpeech::new` 与 `ChatFeed::new` 的 `streaming_tts` 参数 |
| `src-tauri/src/vcp_modules/chat/voice_service/cache.rs` | **新增** | 实现独立模型/音色字段的缓存身份、双后缀验证、原子提交与 rename 退避；根状态仅首次恢复，key 短锁与 Arc 代际保护同名文件，按错误类型决定是否可删除 |
| `src-tauri/src/vcp_modules/chat/voice_service/audio_codec.rs` | **新增** | 实现有界 RIFF 扫描和 24kHz 单声道 PCM16 WAV 快速路径；其他 WAV / MP3 由 Symphonia 解码、Rubato 按需重采样；支持协作取消，错误区分损坏与资源/能力边界 |
| [`src-tauri/src/vcp_modules/chat/voice_service/network.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/network.rs) | 修改 | 单次请求使用统一参数快照；接收完整 JSON Base64 / 直接 WAV、MP3 及 SSE PCM16；原文件 Bytes 共享、完整 PCM 一次交付；移除网络可重试分类，TTS/ASR 均无自动重试 |
| `src-tauri/src/vcp_modules/chat/voice_service/pipeline.rs` | **新增** | Producer / Consumer 调度、先 reserve 后取片的 $K=2$ 窗口、按字节限制的共享缓冲及完整 Vec 接管；首次错误即取消，只校验 prepare 完成，不代为 finish |
| [`src-tauri/src/vcp_modules/chat/voice_service/tts.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/tts.rs) | 修改 | prepare_segment 独占分段 finish 且先于缓存 IO；原生消费统一按 32KB 供包；保留任务 owner、状态、原生墓碑及 Preview 与 Chat/Bubble 的隔离 |
| [`src-tauri/src/vcp_modules/chat/voice_service/mod.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/mod.rs) | 修改 | 挂载 `cache`、`audio_codec`、`pipeline` 子模块并同步构造入口 |
| `docs/vue_docs/components/21_设置原子组件与主题系统.md` | 修改 | 同步统一阶梯切句与通话/气泡缓存、试听无缓存的功能边界 |

---

## 八、 验证与测试矩阵（Verification Plan）

1. **分段一致性测试（`segmentation.rs`）**：
   - 验证相同正文按 stable 多次更新并取片，与全文一次提交并取片得到相同的 `ReplyPiece` 序列；流式传输开关不再进入分段构造器。
2. **缓存 Key 与 `None` 参数正交测试（`cache.rs`）**：
   - 验证 `guidance: None` 与空指导同键，有效指导、模型、音色、样本 ID 各自改变时异键；正文空白原样参与身份，传输模式不参与身份。
   - 验证 `.wav` 与 `.mp3` 双后缀落盘；明确损坏、空文件与容器后缀错配可剔除；资源、worker、不支持、超限和取消不删缓存。缓存读写失败不打断朗读，同 pending 的 rename 保留快速退避及取消清理。
   - 验证解码、写入与退避不持根级或 key 级锁，不同 key 能独立推进；旧解码失败不能删除新发布的同 key 文件，弱引用注册表不积累已结束操作对应的历史条目。
   - 验证 `.pending-*` 孤儿清理先于本进程写入，不删除当前活跃提交或已完成缓存。
3. **多格式解码与重采样测试（`audio_codec.rs`）**：
   - 验证 24kHz 单声道 PCM16 WAV 经 RIFF 快速路径逐字节保留采样，包括额外 chunk、奇数长度 chunk 的对齐和长度越界检查；32kHz / 44.1kHz WAV、其他受支持 WAV 与真实 MP3 经解码库归一化。
   - 验证双声道混音、gapless 元数据、Symphonia 支持的 MP3 元数据与 padding、明确损坏/截断及中途超限；不以 HTTP 完整接收代替音频有效性检查。
4. **$K=2$ 预取与播放器背压解耦测试（`pipeline.rs` / `network.rs`）**：
   - 构造慢速 Mock 播放消费器，验证 $S_0$ 等待原生接纳时，$S_1/S_2$ 仍可完成请求，$S_3$ 必须等 $S_0$ 供包完并取出 $S_1$ 后才开始；缓存命中同样受窗口控制。
   - 用大量小 SSE 包验证段内缓冲依据实际字节限制，不因包计数提前把背压传回网络。
   - 验证 $S_2$ 首次请求失败时仅发出一次请求，不请求 $S_3$，并取消 $S_0/S_1$ 的播放消费；覆盖 HTTP 错误、连接失败和流式中断，无网络退避或重新请求。
   - 验证缓存命中与网络成功的 `prepare_segment` 均独立完成分段，缓存提交等待期间 Consumer 已能读到完成；prepare 未完成便返回应被调度器识别，而不是调度器补调 `finish()`。
   - 验证完整 PCM 一次交付后由空 SegmentBuffer 接管；原生适配器始终按不超过 32KB 的偶数字节包供给，Preview 也遵循此边界。
   - 验证 Producer 不驱动 `speaking`，原生播放未完成时网络/缓存完结不提前发出正常终态。
5. **立即掐断（Immediate Drop）测试**：
   - 验证在 $S_0$ 播放、$S_1$ 已落盘、$S_2$ 正在流式接收中途调用 `stop_tts` 时：HTTP 连接立即断开，$S_1$ 缓存保留，$S_2$ 半截数据不落盘。
   - 验证 `Preview` 试听全程不产生任何磁盘缓存文件。
   - 在解码、写入、rename 及其快速退避阶段取消，验证实际任务已收束且无迟到供包、半截正式缓存或遗留活跃提交。
6. **全量静态检查与回归测试**：
   - 运行 `pnpm check` 与 `cargo test --locked --lib`。
   - host 检查、Android arm64 编译、Apple 构建与具名设备的播放、听感、取消观察分别报告；不以静态检查或交叉编译替代真机验证。

本轮执行结果：`pnpm check` 通过；语音相关 142 项测试及 Rust 全量 565 项测试全部通过；`cargo check --locked --target aarch64-linux-android --lib` 通过。验证覆盖网络首次失败不重发、跨模式离线缓存命中、缓存发布失败不影响音频、资源错误不删缓存及并发发布保护。本轮尚未重新打包安装 Debug，也未进行真机听感验证或 Apple 构建；上述检查不替代这些证据。
