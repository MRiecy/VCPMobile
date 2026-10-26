use std::collections::HashSet;
use std::fs;

lazy_static::lazy_static! {
    /// 统一维护的、可提取的纯文本/代码后缀列表
    pub static ref TEXT_AND_CODE_EXTENSIONS: HashSet<&'static str> = {
        let mut s = HashSet::new();
        s.insert("txt");
        s.insert("md");
        s.insert("csv");
        s.insert("json");
        s.insert("js");
        s.insert("mjs");
        s.insert("bat");
        s.insert("sh");
        s.insert("ts");
        s.insert("tsx");
        s.insert("jsx");
        s.insert("vue");
        s.insert("rs");
        s.insert("py");
        s.insert("java");
        s.insert("c");
        s.insert("cpp");
        s.insert("h");
        s.insert("hpp");
        s.insert("cs");
        s.insert("go");
        s.insert("rb");
        s.insert("php");
        s.insert("swift");
        s.insert("kt");
        s.insert("kts");
        s.insert("css");
        s.insert("html");
        s.insert("xml");
        s.insert("yaml");
        s.insert("yml");
        s.insert("toml");
        s.insert("ini");
        s.insert("sql");
        s.insert("log");
        s.insert("jsonc");
        s.insert("dart");
        s.insert("lua");
        s.insert("r");
        s.insert("pl");
        s.insert("ex");
        s.insert("exs");
        s.insert("zig");
        s.insert("hs");
        s.insert("scala");
        s.insert("groovy");
        s.insert("d");
        s.insert("nim");
        s.insert("cr");
        s
    };

    /// 统一维护的、可提取的结构化文档后缀列表
    pub static ref STRUCTURED_DOC_EXTENSIONS: HashSet<&'static str> = {
        let mut s = HashSet::new();
        s.insert("docx");
        s.insert("pdf");
        s.insert("xlsx");
        s.insert("pptx");
        s
    };
}

/// 判定后缀是否属于可提取的纯文本/代码类文件
pub fn is_text_or_code_extension(ext: &str) -> bool {
    TEXT_AND_CODE_EXTENSIONS.contains(ext)
}

/// 判定后缀是否属于可提取的结构化文档
pub fn is_structured_doc_extension(ext: &str) -> bool {
    STRUCTURED_DOC_EXTENSIONS.contains(ext)
}

/// 判定是否是任何可提取的文档/代码格式
pub fn is_extractable_extension(ext: &str) -> bool {
    is_text_or_code_extension(ext) || is_structured_doc_extension(ext)
}

/// 判定是否是任何受支持的附件后缀格式（多模态格式 或 文本提取格式）
pub fn is_supported_attachment_extension(ext: &str) -> bool {
    let ext_lower = ext.to_lowercase();
    let ext_str = ext_lower.as_str();

    // 1. 多模态媒体支持判定 (与 vcp_client 一致，支持主流媒体及硬件预转码后格式)
    let is_multimodal = matches!(
        ext_str,
        "png"
            | "jpg"
            | "jpeg"
            | "webp"
            | "gif"
            | "bmp"
            | "heic"
            | "heif"
            | "avif"
            | "mp3"
            | "wav"
            | "ogg"
            | "flac"
            | "aac"
            | "m4a"
            | "opus"
            | "amr"
            | "wma"
            | "aiff"
            | "mp4"
            | "webm"
            | "3gp"
            | "3g2"
            | "mov"
            | "mkv"
            | "avi"
            | "flv"
            | "wmv"
            | "ts"
    );

    is_multimodal || is_extractable_extension(ext_str)
}

/// =================================================================
/// vcp_modules/infra/file_extractor.rs - 多模态文件文本内容提取纯函数库
/// =================================================================
/// 内存映射读取文件，自动检测编码并转换为 UTF-8
/// 1. 优先 BOM 头检测（最可靠）
/// 2. 无 BOM 时使用 chardetng 统计检测（Firefox 同款）
fn read_text_with_mmap(path: &std::path::Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mmap = unsafe { memmap2::Mmap::map(&file).ok()? };

    // 1. BOM 检测
    if let Some((encoding, _bom_len)) = encoding_rs::Encoding::for_bom(&mmap) {
        let (text, _had_errors) = encoding.decode_with_bom_removal(&mmap);
        return Some(text.into_owned());
    }

    // 2. 统计检测（无 BOM）
    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(&mmap, true);
    let encoding = detector.guess(None, true);

    let (text, _had_errors) = encoding.decode_without_bom_handling(&mmap);
    Some(text.into_owned())
}

#[path = "file_extractor/office.rs"]
mod office;

fn extract_pdf_text(path: &std::path::Path) -> Option<String> {
    use pdf_oxide::pipeline::converters::{MarkdownOutputConverter, OutputConverter};
    use pdf_oxide::pipeline::{
        ReadingOrderContext, ReadingOrderStrategyType, TextPipeline, TextPipelineConfig,
    };
    use pdf_oxide::PdfDocument;

    let doc = match PdfDocument::open(path) {
        Ok(d) => d,
        Err(e) => {
            log::warn!(
                "[FileExtractor] Failed to open PDF (pdf_oxide): {:?}, error: {:?}",
                path,
                e
            );
            return None;
        }
    };

    let mut full_markdown = String::new();
    let pages_count = match doc.page_count() {
        Ok(count) => count,
        Err(e) => {
            log::warn!(
                "[FileExtractor] Failed to get page count (pdf_oxide): {:?}, error: {:?}",
                path,
                e
            );
            return None;
        }
    };

    // 配置高性能 Pipeline：启用 XY-Cut (投影剖分算法) 识别多栏布局，并开启标题检测
    let mut config = TextPipelineConfig::default();
    config.reading_order.strategy = ReadingOrderStrategyType::XYCut;
    config.output.detect_headings = true;

    let pipeline = TextPipeline::with_config(config.clone());
    let converter = MarkdownOutputConverter::new();

    for i in 0..pages_count {
        // 1. 提取带坐标的原始文本跨度 (Spans)
        match doc.extract_spans(i) {
            Ok(spans) => {
                // 2. 通过 Pipeline 进行阅读顺序分析 (XY-Cut)
                let context = ReadingOrderContext::new().with_page(i as u32);
                match pipeline.process(spans, context) {
                    Ok(ordered_spans) => {
                        // 3. 转换为结构化的 Markdown
                        match converter.convert(&ordered_spans, &config) {
                            Ok(page_md) => {
                                full_markdown.push_str(&page_md);
                                full_markdown.push_str("\n\n");
                            }
                            Err(e) => {
                                log::warn!(
                                    "[FileExtractor] Markdown conversion failed for page {}: {:?}",
                                    i,
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!(
                            "[FileExtractor] Reading order analysis failed for page {}: {:?}",
                            i,
                            e
                        );
                    }
                }
            }
            Err(e) => {
                log::warn!(
                    "[FileExtractor] Span extraction failed for page {}: {:?}",
                    i,
                    e
                );
            }
        }
    }

    if full_markdown.trim().is_empty() {
        Some("[此文件可能为扫描件或图片型 PDF，暂不支持文字提取]".to_string())
    } else {
        Some(full_markdown)
    }
}
/// 物理文件多模态异步/同步提取文本主入口
pub fn try_extract_text(path: &std::path::Path, mime_type: &str) -> Option<String> {
    log::info!(
        "[FileExtractor] Starting extraction for path: {:?}, mime: {}",
        path,
        mime_type
    );

    // 硬上限：防止极端巨型文件载入内存导致 OOM（50MB 为安全阈值）
    const MAX_FILE_SIZE_BYTES: u64 = 50 * 1024 * 1024;
    if let Ok(meta) = fs::metadata(path) {
        if meta.len() > MAX_FILE_SIZE_BYTES {
            return Some(format!(
                "[文件过大（{:.2} MB），已跳过自动提取以保护内存]",
                (meta.len() as f64) / 1024.0 / 1024.0
            ));
        }
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    // 1. 识别是否属于“可直接读取”的文本/代码格式
    let is_text_type = mime_type.starts_with("text/")
        || mime_type == "application/json"
        || mime_type == "application/javascript"
        || mime_type == "application/x-javascript"
        || is_text_or_code_extension(&ext);

    if is_text_type {
        // mmap + 自动编码检测 → UTF-8
        let text = read_text_with_mmap(path)?;

        // 按提取文本长度截断（对齐 2M 上下文模型，约 8-10M 字符）
        const MAX_TEXT_CHARS: usize = 10_000_000;
        if text.chars().count() > MAX_TEXT_CHARS {
            let truncated: String = text.chars().take(MAX_TEXT_CHARS).collect();
            return Some(format!("{}……（文本过长已截断）", truncated));
        }

        return Some(text);
    }

    // 2. 结构化文档 (PDF, Docx, Xlsx, Pptx)
    let doc_text = match ext.as_str() {
        "docx" | "pptx" | "xlsx" => return office::extract(path, &ext),
        "pdf" => extract_pdf_text(path),
        _ => {
            log::warn!(
                "[FileExtractor] No specialized extractor for extension: {}",
                ext
            );
            None
        }
    };

    if let Some(text) = doc_text {
        log::info!(
            "[FileExtractor] Successfully extracted {} chars from structured doc: {:?}",
            text.chars().count(),
            path
        );
        const MAX_TEXT_CHARS: usize = 10_000_000;
        if text.chars().count() > MAX_TEXT_CHARS {
            let truncated: String = text.chars().take(MAX_TEXT_CHARS).collect();
            return Some(format!("{}……（文本过长已截断）", truncated));
        }
        return Some(text);
    }

    log::warn!(
        "[FileExtractor] Extraction failed or returned no content for path: {:?}",
        path
    );
    None
}
