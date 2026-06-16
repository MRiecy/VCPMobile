//! Tail 处理性能基准（仅测试期编译）。
//!
//! 目的：为 `MAX_SPECULATIVE_TAIL_AST_BYTES`（当前 8192）和流式代码块高亮阈值
//! （当前 4096）的取值提供实测数据，并为自适应降帧梯度提供单帧开销曲线。
//!
//! 运行方式（用 perf profile 逼近发布版热路径性能）：
//!   cargo test --profile perf -p vcp-mobile bench_tail_ -- --nocapture --test-threads=1
//!
//! 注意：dev profile 下的绝对耗时无参考意义；务必看 perf profile 的输出，
//! 并参考文末的"发布版换算说明"。

use crate::vcp_modules::aurora_pipeline::{AuroraBuffer, TailFrame};
use crate::vcp_modules::chat::ast_diff::diff_ast;
use crate::vcp_modules::pre_renderer::code_highlighter::highlight_code_block;
use crate::vcp_modules::pre_renderer::{parse_markdown_to_ast_streaming, MarkdownNode};
use std::time::Instant;

/// 以中位数（取多轮最小值附近的稳定值）返回闭包耗时（毫秒）。
fn time_median<F: FnMut()>(iters: usize, mut f: F) -> f64 {
    // 预热，避免首次分配/缓存未命中污染
    f();
    f();
    let mut samples: Vec<f64> = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t = Instant::now();
        f();
        samples.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    samples[samples.len() / 2]
}

/// 读取 40k 测试 HTML 源（827 行）。兼容从 src-tauri/ 或仓库根运行。
fn load_genesis_html() -> String {
    let candidates = [
        "../scripts/tail-test/v1.1.0-aurora-genesis.html",
        "scripts/tail-test/v1.1.0-aurora-genesis.html",
        "g:\\VCPMobile\\scripts\\tail-test\\v1.1.0-aurora-genesis.html",
    ];
    for p in candidates {
        if let Ok(s) = std::fs::read_to_string(p) {
            return s;
        }
    }
    panic!("找不到 v1.1.0-aurora-genesis.html");
}

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

/// 将一段内容包装为未闭合的流式 html 代码围栏（模拟 AI 正在吐出 ```html 块）。
fn as_open_code_fence(content: &str) -> String {
    format!("```html\n{}", content)
}

/// 基准 1：单帧全链路开销 —— parse → hash → diff → serialize，外加 IPC 载荷字节数。
///
/// 这是决定 8KB 上限的关键数据：一帧（一次 process_queue）在 tail 达到某尺寸时的总开销。
/// 由于 CodeBlock 走整节点 Replace，每帧都会把整块重新 parse/hash/serialize。
#[test]
fn bench_tail_single_frame_pipeline() {
    let html = load_genesis_html();
    println!("\n====== 基准 1：单帧全链路开销（代码块路径，未闭合 ```html 围栏）======");
    println!(
        "{:>8} | {:>9} | {:>8} | {:>8} | {:>10} | {:>10} | {:>12}",
        "tier(B)", "parse", "hash", "diff", "serialize", "frame合计", "payload(B)"
    );
    println!("{}", "-".repeat(86));

    for &tier in TIERS.iter() {
        let content = truncate_on_char_boundary(&html, tier);
        let fenced = as_open_code_fence(content);

        // 上一帧：少一个 chunk（模拟刚追加了 ~40B）的同一代码块
        let prev_content = truncate_on_char_boundary(content, content.len().saturating_sub(40));
        let prev_fenced = as_open_code_fence(prev_content);

        let parse_ms = time_median(30, || {
            let _ = parse_markdown_to_ast_streaming(&fenced);
        });

        // parse 内部已算 hash，这里单独测一次纯 hash 增量（在已 parse 的节点上重算）
        let parsed = parse_markdown_to_ast_streaming(&fenced);
        let hash_ms = time_median(30, || {
            let mut nodes = parsed.clone();
            for n in &mut nodes {
                n.compute_hashes_recursively();
            }
        });

        let prev_ast = parse_markdown_to_ast_streaming(&prev_fenced);
        let diff_ms = time_median(30, || {
            let _ = diff_ast(&prev_ast, &parsed, "t");
        });

        // serialize：把 diff 产出的 mutations 封装为 TailFrame 再 JSON 序列化
        let mutations = diff_ast(&prev_ast, &parsed, "t");
        let frame = TailFrame {
            epoch: 1,
            revision: 1,
            frame_seq: 1,
            reset: false,
            snapshot: None,
            mutations,
        };
        let ser_ms = time_median(30, || {
            let _ = serde_json::to_string(&frame).unwrap();
        });
        let payload_bytes = serde_json::to_string(&frame).unwrap().len();

        let frame_total = parse_ms + diff_ms + ser_ms; // parse 已含 hash，不重复计入

        println!(
            "{:>8} | {:>7.3}ms | {:>6.3}ms | {:>6.3}ms | {:>8.3}ms | {:>8.3}ms | {:>12}",
            tier, parse_ms, hash_ms, diff_ms, ser_ms, frame_total, payload_bytes
        );
    }
    println!(
        "说明：frame合计 = parse + diff + serialize（parse 内部已包含 hash 计算，不重复累加）。"
    );
}

/// 基准 2：syntect 高亮开销（决定 4096 流式高亮阈值是否合理）。
#[test]
fn bench_tail_syntect_highlight() {
    let html = load_genesis_html();
    println!("\n====== 基准 2：syntect 高亮开销（highlight_code_block, lang=html）======");
    println!(
        "{:>8} | {:>10} | {:>14}",
        "tier(B)", "highlight", "输出html(B)"
    );
    println!("{}", "-".repeat(40));

    for &tier in TIERS.iter() {
        let content = truncate_on_char_boundary(&html, tier).to_string();
        let hl_ms = time_median(15, || {
            let _ = highlight_code_block(&content, "html");
        });
        let out_bytes = highlight_code_block(&content, "html")
            .map(|s| s.len())
            .unwrap_or(0);
        println!("{:>8} | {:>8.3}ms | {:>14}", tier, hl_ms, out_bytes);
    }
}

/// 基准 3：累计流式开销 —— 一个代码块从 0 增长到目标尺寸，逐帧 re-parse 的总和。
///
/// 模拟真实 SSE：固定 chunk 字节（约模拟一次 SSE delta），每追加一块就跑一次
/// 单帧全链路（parse+diff+serialize）。汇报总耗时、帧数、单帧均值/峰值。
/// 这是"30 帧缓冲累计开销"的直接量化。
#[test]
fn bench_tail_cumulative_stream() {
    let html = load_genesis_html();
    // 典型 SSE delta 约 20~80 字节；取 48B 作为代表（偏保守，帧数偏多 = 开销偏高）。
    const CHUNK_BYTES: usize = 48;

    println!("\n====== 基准 3：累计流式开销（块从 0 增长到目标尺寸，逐帧 re-parse）======");
    println!("每帧 chunk ≈ {}B（保守估计，帧数偏多）", CHUNK_BYTES);
    println!(
        "{:>8} | {:>6} | {:>11} | {:>11} | {:>11}",
        "目标(B)", "帧数", "累计耗时", "单帧均值", "末帧峰值"
    );
    println!("{}", "-".repeat(60));

    for &tier in TIERS.iter() {
        let full = truncate_on_char_boundary(&html, tier);

        // 构造增长边界（char 安全）
        let mut bounds: Vec<usize> = Vec::new();
        let mut b = CHUNK_BYTES;
        while b < full.len() {
            let mut e = b;
            while e < full.len() && !full.is_char_boundary(e) {
                e += 1;
            }
            bounds.push(e);
            b += CHUNK_BYTES;
        }
        bounds.push(full.len());

        let mut total_ms = 0.0_f64;
        let mut last_frame_ms = 0.0_f64;
        let mut prev_ast: Vec<MarkdownNode> = Vec::new();

        for &end in &bounds {
            let content = &full[..end];
            let fenced = as_open_code_fence(content);
            let t = Instant::now();
            let new_ast = parse_markdown_to_ast_streaming(&fenced);
            let mutations = diff_ast(&prev_ast, &new_ast, "t");
            let frame = TailFrame {
                epoch: 1,
                revision: 1,
                frame_seq: 1,
                reset: false,
                snapshot: None,
                mutations,
            };
            let _ = serde_json::to_string(&frame).unwrap();
            let dt = t.elapsed().as_secs_f64() * 1000.0;
            total_ms += dt;
            last_frame_ms = dt;
            prev_ast = new_ast;
        }

        let frames = bounds.len();
        println!(
            "{:>8} | {:>6} | {:>9.2}ms | {:>9.3}ms | {:>9.3}ms",
            tier,
            frames,
            total_ms,
            total_ms / frames as f64,
            last_frame_ms
        );
    }
    println!("说明：单帧均值/峰值用于判断在某尺寸下 30Hz(33ms)/10Hz(100ms)/5Hz(200ms) 帧预算是否被击穿。");
}

/// 基准 4：端到端 AuroraBuffer —— 用真实管道喂入增长的代码块，验证整链路（含
/// take_tail_frame 的 reset/snapshot 逻辑）的真实开销，而非孤立函数。
#[test]
fn bench_tail_end_to_end_aurora() {
    let html = load_genesis_html();
    const CHUNK_BYTES: usize = 48;

    println!("\n====== 基准 4：端到端 AuroraBuffer 累计开销（append_chunk + process_queue + take_tail_frame）======");
    println!(
        "{:>8} | {:>6} | {:>11} | {:>11} | {:>14}",
        "目标(B)", "帧数", "累计耗时", "单帧均值", "总payload(B)"
    );
    println!("{}", "-".repeat(64));

    for &tier in TIERS.iter() {
        let full = truncate_on_char_boundary(&html, tier);
        let fenced_full = as_open_code_fence(full);
        let chars_total = fenced_full.len();

        let mut buffer = AuroraBuffer::new();
        let mut sent = 0usize;
        let mut total_ms = 0.0_f64;
        let mut frames = 0usize;
        let mut total_payload = 0usize;

        while sent < chars_total {
            let mut end = (sent + CHUNK_BYTES).min(chars_total);
            while end < chars_total && !fenced_full.is_char_boundary(end) {
                end += 1;
            }
            let chunk = &fenced_full[sent..end];
            sent = end;

            let t = Instant::now();
            buffer.append_chunk(chunk);
            let _ = buffer.process_queue();
            let frame = buffer.take_tail_frame();
            total_ms += t.elapsed().as_secs_f64() * 1000.0;
            frames += 1;
            if let Some(f) = frame {
                total_payload += serde_json::to_string(&f).unwrap().len();
            }
        }

        println!(
            "{:>8} | {:>6} | {:>9.2}ms | {:>9.3}ms | {:>14}",
            tier,
            frames,
            total_ms,
            total_ms / frames as f64,
            total_payload
        );
    }
    println!("说明：此为最贴近线上的真实开销（含块解析器扫描、reset/snapshot 决策）。");
}
