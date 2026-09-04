#![allow(dead_code, unused_imports)]

//! 渲染性能基准（criterion harness）。
//!
//! ## 测量标准（2026-09 重订）
//!
//! ### 静态渲染
//! - `static_end_to_end`：一次性 content 管线（`parse_content` → JSON 序列化），
//!   按文档尺寸 tier 报均值；相邻 tier 缩放比应接近线性，超线性即存在隐藏 O(n²)。
//!
//! ### 流式渲染（增量语义）
//! 核心不变量：**稳态单帧成本与已积累尺寸无关**（O(chunk)）。每组对比 F(2048) 与
//! F(40960) 两个端点，比值 < 2 视为守住增量语义（允许缓存/锁等常数项劣化）。
//!
//! - `stream_frame_code_fence`：代码围栏快速路径（生产热点，AuroraBuffer 全链路）
//! - `stream_frame_protocol`：协议块封印直通（TOOL_REQUEST tail）
//! - `stream_frame_markdown`：通用 markdown tail —— 当前仍是非增量路径，曲线随
//!   tier 爬坡为预期，不设红线，仅用于量化"还剩多少未增量化的尾巴"
//! - `stream_worst_frame_fence_close`：闭合围栏那一帧（触发 tail 全量重解析），
//!   尾部延迟指标，标准为不超过稳态帧的 ~10×
//! - `stream_full_code_fence`：0→tier 全流累计（原 tail_end_to_end_aurora），
//!   校验 累计 ≈ 帧数 × 稳态单帧
//! - `tail_syntect_highlight`：全量 Syntect 高亮基线（对照用）
//!
//! 运行方式（用 perf profile 逼近发布版热路径性能）：
//!   cargo bench --locked --profile perf
//!
//! fixture 全部 `include_str!` 编译期内嵌（`src-tauri/benches/fixtures/`），素材
//! 来自 `scripts/tail-test/` 的真实消息片段，杜绝运行时绝对路径依赖。

#[path = "../src/distributed/mod.rs"]
mod distributed;
#[path = "../src/vcp_modules/mod.rs"]
mod vcp_modules;

use crate::vcp_modules::aurora_pipeline::{AuroraBuffer, TailFrame};
use crate::vcp_modules::chat::content_parser::parse_content;
use crate::vcp_modules::pre_renderer::code_highlighter::{highlight_code_block, CodeBlockShell};
use criterion::{black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};

/// 40KB HTML 文档（827 行）：html 代码围栏内容、Syntect 高亮基线素材。
const GENESIS_HTML: &str = include_str!("fixtures/v1.1.0-aurora-genesis.html");
/// 27KB 纯 Markdown 长文（元思考链）。
const MD_META: &str = include_str!("fixtures/md_meta.txt");
/// 5KB Markdown + 多个小代码围栏。
const MD_CODE_PRE: &str = include_str!("fixtures/md_code_pre.txt");
/// 14KB 纯 HTML（另一份代码围栏内容素材）。
const HTML_CODEBLOCK: &str = include_str!("fixtures/html_codeblock.txt");
/// 4.7KB 含 `<!--brk-->` 分块标记的 Markdown。
const BRK: &str = include_str!("fixtures/brk.txt");

/// 按字节上限安全截断到 char 边界。
fn truncate_on_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

const TIERS: [usize; 8] = [2048, 4096, 8192, 16384, 24576, 32768, 40000, 40960];

/// 模拟一次 SSE delta 的固定 chunk 字节数。
const CHUNK_BYTES: usize = 48;

/// 未闭合的流式 html 代码围栏（模拟 AI 正在吐出 ```html 块）。
fn as_open_code_fence(content: &str) -> String {
    format!("```html\n{}", content)
}

/// 未闭合的流式 TOOL_REQUEST 协议块（走 aurora 封印直通路径）。
fn as_open_tool_request(content: &str) -> String {
    format!(
        "<<<[TOOL_REQUEST]>>>\ntool_name:「始」Diary「末」\ncontent:「始」{}",
        content
    )
}

/// 大体积纯 Markdown 填充料（约 64KB，覆盖 40960 tier）。
fn big_markdown() -> String {
    format!("{}\n\n{}\n\n{}\n\n{}", MD_META, BRK, MD_CODE_PRE, MD_META)
}

/// 静态混合文档（约 51KB）：长文 + html 代码围栏 + 小围栏集 + brk 分块。
fn static_doc() -> String {
    format!(
        "{}\n\n```html\n{}\n```\n\n{}\n\n<!--brk-->\n\n{}",
        MD_META, HTML_CODEBLOCK, MD_CODE_PRE, BRK
    )
}

fn serialize_frame(frame: &TailFrame) -> String {
    serde_json::to_string(frame).unwrap()
}

/// 静态渲染：一次性 content 管线 end-to-end（parse_content → JSON 序列化）。
/// 混合文档（长文 + 代码围栏 + brk 分块），相邻 tier 缩放比应接近线性。
fn bench_static_end_to_end(c: &mut Criterion) {
    let doc = static_doc();
    let mut group = c.benchmark_group("static_end_to_end");

    for &tier in TIERS.iter() {
        let content = truncate_on_char_boundary(&doc, tier).to_string();
        group.bench_with_input(BenchmarkId::from_parameter(tier), &tier, |b, _| {
            b.iter(|| {
                let blocks = parse_content(black_box(&content));
                let serialized = serde_json::to_string(&blocks).unwrap();
                black_box(serialized);
            });
        });
    }
    group.finish();
}

/// 稳态单帧测量（流式增量语义的核心标准）：setup（不计时）把 AuroraBuffer 喂到
/// tier 尺寸，计时区只包含"追加一个 48B chunk + process_queue + take_tail_frame
/// + 序列化"，即生产环境处理一次 SSE delta 的完整成本。
///
/// 判定标准：F(40960)/F(2048) < 2 视为守住 O(chunk) 增量语义。
fn bench_steady_frame_group(
    c: &mut Criterion,
    name: &str,
    prefill: impl Fn(usize) -> String,
    chunk: &'static str,
) {
    let mut group = c.benchmark_group(name);
    for &tier in TIERS.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(tier), &tier, |b, &tier| {
            let pre = prefill(tier);
            b.iter_batched(
                || {
                    let mut buf = AuroraBuffer::new();
                    buf.append_chunk(&pre);
                    let _ = buf.process_queue();
                    let _ = buf.take_tail_frame();
                    buf
                },
                |mut buf| {
                    buf.append_chunk(black_box(chunk));
                    let _ = buf.process_queue();
                    if let Some(f) = buf.take_tail_frame() {
                        black_box(serialize_frame(&f));
                    }
                    buf
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

/// 稳态单帧 · 代码围栏快速路径（生产热点）。
fn bench_stream_frame_code_fence(c: &mut Criterion) {
    bench_steady_frame_group(
        c,
        "stream_frame_code_fence",
        |tier| as_open_code_fence(truncate_on_char_boundary(GENESIS_HTML, tier)),
        truncate_on_char_boundary(GENESIS_HTML, CHUNK_BYTES),
    );
}

/// 稳态单帧 · 协议块封印直通（TOOL_REQUEST tail）。
fn bench_stream_frame_protocol(c: &mut Criterion) {
    let big = big_markdown();
    bench_steady_frame_group(
        c,
        "stream_frame_protocol",
        move |tier| as_open_tool_request(truncate_on_char_boundary(&big, tier)),
        truncate_on_char_boundary(MD_META, CHUNK_BYTES),
    );
}

/// 稳态单帧 · 通用 markdown tail（非增量路径，爬坡为预期，仅记录不设红线）。
fn bench_stream_frame_markdown(c: &mut Criterion) {
    let big = big_markdown();
    bench_steady_frame_group(
        c,
        "stream_frame_markdown",
        move |tier| truncate_on_char_boundary(&big, tier).to_string(),
        truncate_on_char_boundary(MD_META, CHUNK_BYTES),
    );
}

/// 稳态单帧 · HtmlContainer 树权威 patch 路径。tail = 永不闭合的最外层 div +
/// 平衡的 HTML 内容。注意：后端每帧全量 html5ever parse（O(tail)，native），
/// IPC/前端为 O(增量)；本组如实刻画后端这条残余全量曲线的爬升率。
fn bench_stream_frame_html_container(c: &mut Criterion) {
    bench_steady_frame_group(
        c,
        "stream_frame_html_container",
        |tier| {
            format!(
                "<div id=\"bench-root\">\n{}",
                truncate_on_char_boundary(GENESIS_HTML, tier)
            )
        },
        truncate_on_char_boundary(GENESIS_HTML, CHUNK_BYTES),
    );
}

/// 最坏帧 · 闭合围栏那一帧：触发 tail 全量重解析，是 UI 掉帧的真正来源。
/// 每轮 setup 都要完整重喂一遍 buffer，故只取三档代表 tier 并压缩采样数。
fn bench_worst_frame_fence_close(c: &mut Criterion) {
    let mut group = c.benchmark_group("stream_worst_frame_fence_close");
    group.sample_size(10);
    group.measurement_time(std::time::Duration::from_secs(2));

    for &tier in [4096usize, 16384, 40960].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(tier), &tier, |b, &tier| {
            let pre = as_open_code_fence(truncate_on_char_boundary(GENESIS_HTML, tier));
            b.iter_batched(
                || {
                    let mut buf = AuroraBuffer::new();
                    buf.append_chunk(&pre);
                    let _ = buf.process_queue();
                    let _ = buf.take_tail_frame();
                    buf
                },
                |mut buf| {
                    buf.append_chunk(black_box("\n```\n"));
                    let _ = buf.process_queue();
                    if let Some(f) = buf.take_tail_frame() {
                        black_box(serialize_frame(&f));
                    }
                    buf
                },
                BatchSize::PerIteration,
            );
        });
    }
    group.finish();
}

/// 全量 Syntect 高亮基线，用于对照增量路径的单帧成本。
fn bench_syntect_highlight(c: &mut Criterion) {
    let mut group = c.benchmark_group("tail_syntect_highlight");

    for &tier in TIERS.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(tier), &tier, |b, &tier| {
            let content = truncate_on_char_boundary(GENESIS_HTML, tier).to_string();
            b.iter(|| {
                let out = highlight_code_block(black_box(&content), "html", CodeBlockShell::Html);
                black_box(out);
            });
        });
    }
    group.finish();
}

/// 全流累计：代码围栏从 0 增长到目标尺寸、逐 48B chunk 走真实 AuroraBuffer 管道
/// 的总耗时。校验 累计 ≈ 帧数 × 稳态单帧（无隐蔽的每帧 O(n) 项）。
fn bench_stream_full_code_fence(c: &mut Criterion) {
    let mut group = c.benchmark_group("stream_full_code_fence");

    for &tier in TIERS.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(tier), &tier, |b, &tier| {
            let full = truncate_on_char_boundary(GENESIS_HTML, tier);
            let fenced_full = as_open_code_fence(full);
            let chars_total = fenced_full.len();

            b.iter(|| {
                let mut buffer = AuroraBuffer::new();
                let mut sent = 0usize;
                while sent < chars_total {
                    let mut end = (sent + CHUNK_BYTES).min(chars_total);
                    while end < chars_total && !fenced_full.is_char_boundary(end) {
                        end += 1;
                    }
                    let chunk = &fenced_full[sent..end];
                    sent = end;
                    buffer.append_chunk(chunk);
                    let _ = buffer.process_queue();
                    let frame = buffer.take_tail_frame();
                    if let Some(f) = frame {
                        let _ = serde_json::to_string(&f).unwrap();
                    }
                }
                black_box(());
            });
        });
    }
    group.finish();
}

criterion_group!(
    name = ast_tail_benches;
    config = Criterion::default();
    targets = bench_static_end_to_end,
    bench_stream_frame_code_fence,
    bench_stream_frame_protocol,
    bench_stream_frame_markdown,
    bench_stream_frame_html_container,
    bench_worst_frame_fence_close,
    bench_stream_full_code_fence,
    bench_syntect_highlight,
);
criterion_main!(ast_tail_benches);
