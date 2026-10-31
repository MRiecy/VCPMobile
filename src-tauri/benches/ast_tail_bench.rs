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
//! - `stream_worst_frame_fence_close/full_event`：闭合围栏、完成态高亮及完整
//!   StreamEvent JSON（含 stable_append）；不含原生 IPC 传输与前端 DOM，
//!   与旧版只序列化 TailFrame 的结果不可直接比较，也不以 host 耗时判定移动端掉帧。
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
use crate::vcp_modules::vcp_client::StreamEvent;
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

const FENCE_CLOSE_CHUNK: &str = "\n```\n";

/// 模拟闭合前的内容已经成功交付；take_tail_frame 不更新 pushed_len 等交付游标，
/// 若仅消费 TailFrame，测量的下一条 delta 会错误地携带整份预填充原文。
fn prefill_delivered_code_fence(pre: &str) -> AuroraBuffer {
    let mut buf = AuroraBuffer::new();
    buf.append_chunk(pre);
    let _ = buf.process_queue();
    let (_, commit) = buf.prepare_snapshot_update();
    buf.commit_delivery(commit);
    buf
}

/// 使用生产 DTO 和交付游标，序列化完整闭合事件；真实 Channel 传输不在 host 基准中。
fn close_and_serialize_code_fence(buf: &mut AuroraBuffer) -> String {
    buf.append_chunk(black_box(FENCE_CLOSE_CHUNK));
    let _ = buf.process_queue();
    let (update, commit) = buf
        .prepare_delta_update()
        .expect("closing a prefilled fence produces an Aurora delta");
    let event = StreamEvent::aurora("bench-fence-close".to_string(), update, None);
    let serialized = serde_json::to_string(&event).expect("serialize complete fence-close event");
    buf.commit_delivery(commit);
    serialized
}

/// 计时外核实实际序列化载荷，防止漏测 stable_append 或重复统计已交付正文。
/// 字节统计为此 fixture 的观测值，不设通用 HTML 膨胀比或耗时门禁。
fn verify_and_report_fence_close_payload(pre: &str, tier: usize, fixture_bytes: usize) {
    let mut buf = prefill_delivered_code_fence(pre);
    let serialized = close_and_serialize_code_fence(&mut buf);
    let wire: serde_json::Value =
        serde_json::from_str(&serialized).expect("parse serialized fence-close event");
    assert_eq!(wire["type"], "aurora");
    let update = &wire["aurora"];
    assert_eq!(update["kind"], "delta");
    assert_eq!(update["chunk"], FENCE_CLOSE_CHUNK);
    assert!(update.get("content").is_none());
    assert!(update.get("stableBlocks").is_none());
    // find_matching_fence_end 的结束位置不消费围栏行末的换行；本 fixture
    // 因此留下一个新的 Markdown tail，而非 Clear。核实内容与空 AST，确保
    // 大代码块已经沉淀、没有继续夹在新 tail 中，也不为基准断言改变生产定界。
    assert_eq!(update["tailOp"]["op"], "replace");
    assert_eq!(update["tailOp"]["content"], "\n");
    assert_eq!(update["tailOp"]["mode"], "ast");
    assert_eq!(update["tailOp"]["blockType"], "markdown");
    assert_eq!(update["tailFrame"]["reset"], true);
    assert!(update["tailFrame"]["snapshot"]
        .as_array()
        .expect("fence handoff carries a tail AST snapshot")
        .is_empty());
    let stable_append = &update["stableAppend"];
    assert_eq!(stable_append["baseCount"], 0);
    let blocks = stable_append["blocks"]
        .as_array()
        .expect("serialized delta contains stableAppend.blocks");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["type"], "html-preview");
    let code = blocks[0]["content"]
        .as_str()
        .expect("stable HTML preview contains source");
    let highlighted_html = blocks[0]["highlighted_content"]
        .as_str()
        .expect("stable HTML preview contains completed highlighting");
    assert!(!code.is_empty());
    assert!(!highlighted_html.is_empty());
    assert!(buf.prepare_delta_update().is_none());

    let stable_append_json_bytes = serde_json::to_vec(stable_append)
        .expect("serialize stable append for payload accounting")
        .len();
    eprintln!(
        "fence_close_payload tier_bytes={tier} fixture_bytes={fixture_bytes} \
         prefill_bytes={} code_bytes={} highlighted_html_bytes={} \
         stable_append_json_bytes={stable_append_json_bytes} event_json_bytes={}",
        pre.len(),
        code.len(),
        highlighted_html.len(),
        serialized.len(),
    );
}

/// 闭合围栏的后端最坏帧：整块高亮、prepare_delta_update、完整 StreamEvent JSON
/// 与成功交付提交。每轮预填充及实际载荷核验不计时；不据 host 数值推导 Android UI 帧时。
fn bench_worst_frame_fence_close(c: &mut Criterion) {
    let mut group = c.benchmark_group("stream_worst_frame_fence_close");
    group.sample_size(10);
    group.measurement_time(std::time::Duration::from_secs(2));

    for &tier in [4096usize, 16384, 40960].iter() {
        let fixture = truncate_on_char_boundary(GENESIS_HTML, tier);
        let pre = as_open_code_fence(fixture);
        verify_and_report_fence_close_payload(&pre, tier, fixture.len());
        // 新 ID 隔离旧的 tail-only Criterion baseline，避免把测量范围扩大误报为回归。
        group.bench_with_input(BenchmarkId::new("full_event", tier), &pre, |b, pre| {
            b.iter_batched(
                || prefill_delivered_code_fence(pre),
                |mut buf| {
                    black_box(close_and_serialize_code_fence(&mut buf));
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
