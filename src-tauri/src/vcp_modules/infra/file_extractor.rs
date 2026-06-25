use std::collections::HashSet;
use std::fs;
use std::io::Read;

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

fn simple_xml_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

fn col_name_to_index(r: &str) -> usize {
    let letters: String = r.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let mut index = 0;
    for c in letters.chars() {
        let val = (c.to_ascii_uppercase() as usize) - ('A' as usize) + 1;
        index = index * 26 + val;
    }
    if index > 0 {
        index - 1
    } else {
        0
    }
}

fn extract_docx_text(path: &std::path::Path) -> Option<String> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            log::warn!(
                "[FileExtractor] Failed to open DOCX: {:?}, error: {}",
                path,
                e
            );
            return None;
        }
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(e) => {
            log::warn!(
                "[FileExtractor] Failed to read DOCX zip archive: {:?}, error: {}",
                path,
                e
            );
            return None;
        }
    };
    let mut doc_file = match archive.by_name("word/document.xml") {
        Ok(f) => f,
        Err(e) => {
            log::warn!(
                "[FileExtractor] Failed to find word/document.xml in DOCX: {:?}, error: {}",
                path,
                e
            );
            return None;
        }
    };

    let mut content = String::new();
    if let Err(e) = doc_file.read_to_string(&mut content) {
        log::warn!(
            "[FileExtractor] Failed to read document.xml content: {:?}, error: {}",
            path,
            e
        );
        return None;
    }

    let mut result = String::new();
    let mut in_tag = false;
    let mut is_collecting = false;
    let mut tag_buffer = String::new();

    // 表格提取状态
    let mut in_tc = false;
    let mut row_count = 0;
    let mut cell_texts: Vec<String> = Vec::new();
    let mut current_cell_text = String::new();

    // 段落属性追踪状态
    let mut in_p_pr = false;
    let mut p_heading_level = 0; // 0=普通段落, 1=#, 2=##, 3=###...
    let mut p_is_list = false;
    let mut current_p_text = String::new();

    // 字符加粗状态追踪
    let mut in_r_pr = false;
    let mut r_is_bold = false;

    for c in content.chars() {
        if c == '<' {
            in_tag = true;
            tag_buffer.clear();
        } else if c == '>' {
            in_tag = false;
            let tag_content = tag_buffer.trim();

            if let Some(name) = tag_content.strip_prefix('/') {
                if name == "w:t" {
                    is_collecting = false;
                } else if name == "w:pPr" {
                    in_p_pr = false;
                } else if name == "w:rPr" {
                    in_r_pr = false;
                } else if name == "w:p" {
                    let cleaned_p = simple_xml_unescape(&current_p_text).trim().to_string();
                    if !cleaned_p.is_empty() {
                        let mut prefix = String::new();
                        if p_heading_level > 0 {
                            prefix.push('\n');
                            for _ in 0..p_heading_level {
                                prefix.push('#');
                            }
                            prefix.push(' ');
                        } else if p_is_list {
                            prefix.push_str("- ");
                        }

                        if in_tc {
                            current_cell_text.push_str(&prefix);
                            current_cell_text.push_str(&cleaned_p);
                        } else {
                            result.push_str(&prefix);
                            result.push_str(&cleaned_p);
                            result.push('\n');
                        }
                    } else if !in_tc {
                        result.push('\n');
                    }
                    current_p_text.clear();
                    p_heading_level = 0;
                    p_is_list = false;
                } else if name == "w:tc" {
                    in_tc = false;
                    cell_texts.push(current_cell_text.trim().to_string());
                    current_cell_text.clear();
                } else if name == "w:tr" {
                    if !cell_texts.is_empty() {
                        let row_str = format!("| {} |\n", cell_texts.join(" | "));
                        result.push_str(&row_str);
                        if row_count == 0 {
                            let separators = vec!["---"; cell_texts.len()];
                            let sep_str = format!("| {} |\n", separators.join(" | "));
                            result.push_str(&sep_str);
                        }
                        row_count += 1;
                        cell_texts.clear();
                    }
                } else if name == "w:tbl" {
                    result.push('\n');
                }
            } else {
                let is_self_closing = tag_content.ends_with('/');
                let mut clean_content = tag_content;
                if is_self_closing {
                    clean_content = tag_content[..tag_content.len() - 1].trim_end();
                }
                let name = clean_content.split_whitespace().next().unwrap_or("");

                if name == "w:t" {
                    if !is_self_closing {
                        is_collecting = true;
                    }
                } else if name == "w:pPr" {
                    in_p_pr = true;
                } else if name == "w:rPr" {
                    in_r_pr = true;
                    r_is_bold = false; // 进入新 Run 重置加粗
                } else if name == "w:b" || name == "w:bCs" {
                    if in_r_pr {
                        r_is_bold = true; // 开启加粗
                    }
                } else if name == "w:pStyle" {
                    if in_p_pr {
                        let mut val = "";
                        if let Some(val_pos) = clean_content.find("w:val=\"") {
                            let sub = &clean_content[val_pos + 7..];
                            if let Some(end_pos) = sub.find('"') {
                                val = &sub[..end_pos];
                            }
                        } else if let Some(val_pos) = clean_content.find("w:val='") {
                            let sub = &clean_content[val_pos + 7..];
                            if let Some(end_pos) = sub.find('\'') {
                                val = &sub[..end_pos];
                            }
                        }

                        let lower_val = val.to_lowercase();
                        // 1. 精确防误匹配（如 ListParagraph3 不判定为 H3）
                        if lower_val.starts_with("heading") || lower_val.contains("heading") {
                            if lower_val.contains("1") {
                                p_heading_level = 1;
                            } else if lower_val.contains("2") {
                                p_heading_level = 2;
                            } else if lower_val.contains("3") {
                                p_heading_level = 3;
                            } else if lower_val.contains("4") {
                                p_heading_level = 4;
                            }
                        } else if lower_val == "1"
                            || lower_val == "heading1"
                            || lower_val == "标题 1"
                        {
                            p_heading_level = 1;
                        } else if lower_val == "2"
                            || lower_val == "heading2"
                            || lower_val == "标题 2"
                        {
                            p_heading_level = 2;
                        } else if lower_val == "3"
                            || lower_val == "heading3"
                            || lower_val == "标题 3"
                        {
                            p_heading_level = 3;
                        } else if lower_val.contains("title") {
                            p_heading_level = 1;
                        }
                    }
                } else if name == "w:numPr" {
                    if in_p_pr {
                        p_is_list = true;
                    }
                } else if name == "w:tbl" {
                    row_count = 0;
                    result.push('\n');
                } else if name == "w:tc" {
                    in_tc = true;
                    current_cell_text.clear();
                } else if name == "w:tab" {
                    if in_tc {
                        current_cell_text.push('\t');
                    } else {
                        result.push('\t');
                    }
                } else if name == "w:br" || name == "w:cr" {
                    if in_tc {
                        current_cell_text.push('\n');
                    } else {
                        result.push('\n');
                    }
                }
            }
            tag_buffer.clear();
        } else if in_tag {
            tag_buffer.push(c);
        } else if is_collecting {
            if r_is_bold {
                current_p_text.push_str(&format!("**{}**", c));
            } else {
                current_p_text.push(c);
            }
        }
    }

    // 加粗熔接：把 "**学****号**" 优化为 "**学号**"
    let cleaned_result = result.replace("****", "");
    if cleaned_result.trim().is_empty() {
        None
    } else {
        Some(cleaned_result)
    }
}

fn extract_pptx_text(path: &std::path::Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut text = String::new();

    let mut slide_names: Vec<String> = archive
        .file_names()
        .filter(|name| name.starts_with("ppt/slides/slide") && name.ends_with(".xml"))
        .map(|name| name.to_string())
        .collect();

    slide_names.sort_by_key(|name| {
        let num_str: String = name.chars().filter(|c| c.is_ascii_digit()).collect();
        num_str.parse::<usize>().unwrap_or(0)
    });

    for (idx, name) in slide_names.iter().enumerate() {
        if let Ok(mut slide_file) = archive.by_name(name) {
            let mut content = String::new();
            if slide_file.read_to_string(&mut content).is_ok() {
                let mut slide_text = String::new();
                let mut in_tag = false;
                let mut is_collecting = false;
                let mut tag_buffer = String::new();

                for c in content.chars() {
                    if c == '<' {
                        in_tag = true;
                        tag_buffer.clear();
                    } else if c == '>' {
                        in_tag = false;
                        let tag_content = tag_buffer.trim();
                        if let Some(name) = tag_content.strip_prefix('/') {
                            if name == "a:t" {
                                is_collecting = false;
                            } else if name == "a:p" {
                                slide_text.push('\n');
                            }
                        } else {
                            let is_self_closing = tag_content.ends_with('/');
                            let mut clean_content = tag_content;
                            if is_self_closing {
                                clean_content = tag_content[..tag_content.len() - 1].trim_end();
                            }
                            let name = clean_content.split_whitespace().next().unwrap_or("");
                            if name == "a:t" {
                                if !is_self_closing {
                                    is_collecting = true;
                                }
                            } else if name == "a:br" {
                                slide_text.push('\n');
                            }
                        }
                        tag_buffer.clear();
                    } else if in_tag {
                        tag_buffer.push(c);
                    } else if is_collecting {
                        slide_text.push(c);
                    }
                }

                let cleaned = simple_xml_unescape(&slide_text);
                if !cleaned.trim().is_empty() {
                    text.push_str(&format!("\n--- Slide {} ---\n", idx + 1));
                    text.push_str(&cleaned);
                }
            }
        }
    }

    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn extract_xlsx_text(path: &std::path::Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;

    // 1. 从 xl/workbook.xml 提取每个 sheet 的真实名称，建立 ID 到名字的映射
    let mut sheet_names_map = std::collections::HashMap::new();
    if let Ok(mut wb_file) = archive.by_name("xl/workbook.xml") {
        let mut wb_content = String::new();
        if wb_file.read_to_string(&mut wb_content).is_ok() {
            let mut in_tag = false;
            let mut tag_buffer = String::new();
            for c in wb_content.chars() {
                if c == '<' {
                    in_tag = true;
                    tag_buffer.clear();
                } else if c == '>' {
                    in_tag = false;
                    let tag_content = tag_buffer.trim();
                    let name = tag_content.split_whitespace().next().unwrap_or("");
                    if name == "sheet" {
                        let mut s_name = String::new();
                        let mut s_id = 0;

                        if let Some(n_pos) = tag_content.find("name=\"") {
                            let sub = &tag_content[n_pos + 6..];
                            if let Some(end_pos) = sub.find('"') {
                                s_name = simple_xml_unescape(&sub[..end_pos]);
                            }
                        } else if let Some(n_pos) = tag_content.find("name='") {
                            let sub = &tag_content[n_pos + 6..];
                            if let Some(end_pos) = sub.find('\'') {
                                s_name = simple_xml_unescape(&sub[..end_pos]);
                            }
                        }

                        let mut id_str = "";
                        if let Some(id_pos) = tag_content.find("sheetId=\"") {
                            let sub = &tag_content[id_pos + 9..];
                            if let Some(end_pos) = sub.find('"') {
                                id_str = &sub[..end_pos];
                            }
                        } else if let Some(id_pos) = tag_content.find("sheetId='") {
                            let sub = &tag_content[id_pos + 9..];
                            if let Some(end_pos) = sub.find('\'') {
                                id_str = &sub[..end_pos];
                            }
                        }
                        if let Ok(parsed_id) = id_str.parse::<usize>() {
                            s_id = parsed_id;
                        }

                        if s_id > 0 && !s_name.is_empty() {
                            sheet_names_map.insert(s_id, s_name);
                        }
                    }
                    tag_buffer.clear();
                } else if in_tag {
                    tag_buffer.push(c);
                }
            }
        }
    }

    // 2. 获取共享字符串表
    let mut shared_strings = Vec::new();
    if let Ok(mut ss_file) = archive.by_name("xl/sharedStrings.xml") {
        let mut content = String::new();
        if ss_file.read_to_string(&mut content).is_ok() {
            let mut in_tag = false;
            let mut is_collecting = false;
            let mut tag_buffer = String::new();
            let mut current_str = String::new();

            for c in content.chars() {
                if c == '<' {
                    in_tag = true;
                    tag_buffer.clear();
                } else if c == '>' {
                    in_tag = false;
                    let tag_content = tag_buffer.trim();
                    if let Some(name) = tag_content.strip_prefix('/') {
                        if name == "t" {
                            is_collecting = false;
                            shared_strings.push(simple_xml_unescape(&current_str));
                            current_str.clear();
                        }
                    } else {
                        let name = tag_content.split_whitespace().next().unwrap_or("");
                        if name == "t" {
                            is_collecting = true;
                        }
                    }
                    tag_buffer.clear();
                } else if in_tag {
                    tag_buffer.push(c);
                } else if is_collecting {
                    current_str.push(c);
                }
            }
        }
    }

    // 3. 扫描并收集所有的 worksheets/sheet*.xml 并按照编号排序
    let mut sheet_names: Vec<String> = archive
        .file_names()
        .filter(|name| name.starts_with("xl/worksheets/sheet") && name.ends_with(".xml"))
        .map(|name| name.to_string())
        .collect();

    sheet_names.sort_by_key(|name| {
        let num_str: String = name.chars().filter(|c| c.is_ascii_digit()).collect();
        num_str.parse::<usize>().unwrap_or(0)
    });

    let mut final_text = String::new();

    for name in sheet_names {
        // 从名称中提取数字，建立 ID 到名字的映射
        let num_str: String = name.chars().filter(|c| c.is_ascii_digit()).collect();
        let sheet_id = num_str.parse::<usize>().unwrap_or(0);

        let sheet_display_name = sheet_names_map
            .get(&sheet_id)
            .cloned()
            .unwrap_or_else(|| format!("Sheet {}", sheet_id));

        if let Ok(mut sheet_file) = archive.by_name(&name) {
            let mut content = String::new();
            if sheet_file.read_to_string(&mut content).is_ok() {
                let mut sheet_text = String::new();
                let mut in_tag = false;
                let mut tag_buffer = String::new();
                let mut row_cells: Vec<(usize, String)> = Vec::new();
                let mut current_col_idx = 0;
                let mut is_shared_string = false;
                let mut in_val = false;
                let mut val_buffer = String::new();

                for c in content.chars() {
                    if c == '<' {
                        in_tag = true;
                        tag_buffer.clear();
                    } else if c == '>' {
                        in_tag = false;
                        let tag_content = tag_buffer.trim();
                        if let Some(name) = tag_content.strip_prefix('/') {
                            if name == "v" {
                                in_val = false;
                                let val_str = val_buffer.trim().to_string();
                                let final_val = if is_shared_string {
                                    if let Ok(idx) = val_str.parse::<usize>() {
                                        shared_strings.get(idx).cloned().unwrap_or(val_str)
                                    } else {
                                        val_str
                                    }
                                } else {
                                    val_str
                                };
                                row_cells.push((current_col_idx, final_val));
                            } else if name == "row" && !row_cells.is_empty() {
                                row_cells.sort_by_key(|&(idx, _)| idx);
                                let mut last_idx = 0;
                                let mut line_str = String::new();
                                for &(idx, ref val) in &row_cells {
                                    while last_idx < idx {
                                        line_str.push('\t');
                                        last_idx += 1;
                                    }
                                    if last_idx > 0 {
                                        line_str.push('\t');
                                    }
                                    line_str.push_str(val);
                                    last_idx = idx + 1;
                                }
                                sheet_text.push_str(&line_str);
                                sheet_text.push('\n');
                                row_cells.clear();
                            }
                        } else {
                            let name = tag_content.split_whitespace().next().unwrap_or("");
                            if name == "c" {
                                is_shared_string = tag_content.contains("t=\"s\"")
                                    || tag_content.contains("t='s'");
                                let mut r_val = "";
                                if let Some(r_pos) = tag_content.find("r=\"") {
                                    let sub = &tag_content[r_pos + 3..];
                                    if let Some(end_pos) = sub.find('"') {
                                        r_val = &sub[..end_pos];
                                    }
                                } else if let Some(r_pos) = tag_content.find("r='") {
                                    let sub = &tag_content[r_pos + 3..];
                                    if let Some(end_pos) = sub.find('\'') {
                                        r_val = &sub[..end_pos];
                                    }
                                }
                                current_col_idx = if !r_val.is_empty() {
                                    col_name_to_index(r_val)
                                } else {
                                    current_col_idx + 1
                                };
                            } else if name == "v" {
                                in_val = true;
                                val_buffer.clear();
                            } else if name == "row" {
                                row_cells.clear();
                            }
                        }
                        tag_buffer.clear();
                    } else if in_tag {
                        tag_buffer.push(c);
                    } else if in_val {
                        val_buffer.push(c);
                    }
                }

                if !sheet_text.trim().is_empty() {
                    final_text.push_str(&format!(
                        "\n--- Sheet {}: {} ---\n",
                        sheet_id, sheet_display_name
                    ));
                    final_text.push_str(&sheet_text);
                }
            }
        }
    }

    if final_text.trim().is_empty() {
        None
    } else {
        Some(final_text)
    }
}

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
        "docx" => extract_docx_text(path),
        "pptx" => extract_pptx_text(path),
        "xlsx" => extract_xlsx_text(path),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::path::Path;

    #[test]
    fn test_extract_sample_files() {
        let test_dir = Path::new("g:\\VCPMobile\\scripts\\test");
        if !test_dir.exists() {
            println!("测试目录不存在: {:?}", test_dir);
            return;
        }

        let files = vec![
            (
                "上机实验一：蒙特卡罗模拟原理与方法.pptx",
                "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            ),
            ("模拟模型——仓库卸货问题.pdf", "application/pdf"),
            (
                "蒙特卡洛计算π值.xlsx",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            ),
            (
                "蒙特卡罗模拟实验报告.docx",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            ),
            (
                "计划书【四稿】new.docx",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            ),
            ("计划书【四稿】new.pdf", "application/pdf"),
            ("银杏科技.pdf", "application/pdf"),
        ];

        for (filename, mime) in files {
            let file_path = test_dir.join(filename);
            if !file_path.exists() {
                println!("警告: 测试文件不存在 {:?}", file_path);
                continue;
            }

            // 如果是 PDF，我们进行详细诊断
            if filename.ends_with(".pdf") {
                use pdf_oxide::PdfDocument;
                match PdfDocument::open(&file_path) {
                    Ok(doc) => {
                        let pages_count = doc.page_count().unwrap_or(0);
                        println!("PDF 成功载入! 总页数: {}", pages_count);
                        for i in 0..pages_count {
                            match doc.extract_text(i) {
                                Ok(page_text) => {
                                    println!(
                                        "  - 页码 {}: 成功提取 {} 字符",
                                        i + 1,
                                        page_text.chars().count()
                                    );
                                }
                                Err(e) => {
                                    println!("  - 页码 {}: 提取文本失败: {:?}", i + 1, e);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        println!("PDF 载入失败: {:?}", e);
                    }
                }
            }

            let start_time = std::time::Instant::now();
            let extracted_text = try_extract_text(&file_path, mime);
            let duration = start_time.elapsed();

            if let Some(text) = extracted_text {
                println!(
                    "【成功】提取文件: {}, 大小: {} 字节, 字符数: {}, 耗时: {:?}",
                    filename,
                    fs::metadata(&file_path).unwrap().len(),
                    text.chars().count(),
                    duration
                );

                // 1. 保存为 .extracted.md，以供人工直观走查 Markdown 渲染效果
                let md_path = test_dir.join(format!("{}.extracted.md", filename));
                fs::write(&md_path, &text).expect("写入 md 文件失败");

                // 2. 保存为包含元数据的结构化 .extracted.json
                let json_meta = json!({
                    "file_name": filename,
                    "file_size_bytes": fs::metadata(&file_path).unwrap().len(),
                    "extracted_at": chrono::Utc::now().to_rfc3339(),
                    "extraction_duration_ms": duration.as_millis(),
                    "char_count": text.chars().count(),
                    "content": text
                });
                let json_path = test_dir.join(format!("{}.extracted.json", filename));
                fs::write(
                    &json_path,
                    serde_json::to_string_pretty(&json_meta).unwrap(),
                )
                .expect("写入 json 文件失败");
            } else {
                println!("【失败】提取文件: {}，结果为 None", filename);
            }
        }
    }
}
