//! file_extractor 集成测试。
//!
//! 从 `src-tauri/src/vcp_modules/infra/file_extractor.rs` 内联测试迁出。
//! 原测试 `test_extract_sample_files` 使用绝对路径 `g:\VCPMobile\scripts\test`
//! （该目录不存在 → CI 必跳过）且无任何断言，仅做人工诊断。
//!
//! 本集成测试使用从 `scripts/attach-test/` 复制整理来的真实附件样本：
//! - `sample.docx`：蒙特卡罗模拟实验报告
//! - `sample.xlsx`：蒙特卡洛计算π值表格
//! - `sample.pdf`：扫描件/图片型 PDF 回退提示
//! - `sample.pptx`：蒙特卡罗模拟原理与方法演示文稿
//! 另覆盖文本文件 BOM/编码解码路径与超大文件 OOM 防护路径。

use std::io::Write;
use tempfile::NamedTempFile;

#[allow(dead_code)]
#[path = "../src/vcp_modules/infra/file_extractor.rs"]
mod file_extractor;
use file_extractor::try_extract_text;

/// 将内嵌的 fixture 字节写入临时文件，返回其路径。
fn write_fixture(bytes: &[u8], ext: &str) -> NamedTempFile {
    let mut file = NamedTempFile::with_suffix(format!(".{ext}")).expect("创建临时文件失败");
    file.write_all(bytes).expect("写入临时文件失败");
    file
}

#[test]
fn test_extract_docx_returns_expected_text() {
    let docx_bytes = include_bytes!("fixtures/file_extractor/sample.docx");
    let file = write_fixture(docx_bytes, "docx");
    let text = try_extract_text(
        file.path(),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    )
    .expect("DOCX 提取不应返回 None");

    assert!(
        text.contains("管理学院上机实验报告") || text.contains("管 理 学 院 上 机 实 验 报 告"),
        "DOCX 提取结果应包含实验报告标题，实际: {text:?}"
    );
    assert!(
        text.contains("蒙特卡罗模拟原理与方法"),
        "DOCX 提取结果应包含实验内容关键词，实际: {text:?}"
    );
}

#[test]
fn test_extract_xlsx_returns_expected_text() {
    let xlsx_bytes = include_bytes!("fixtures/file_extractor/sample.xlsx");
    let file = write_fixture(xlsx_bytes, "xlsx");
    let text = try_extract_text(
        file.path(),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    )
    .expect("XLSX 提取不应返回 None");

    assert!(
        text.contains("蒙特卡罗方法计算 π 值"),
        "XLSX 提取结果应包含表格标题，实际: {text:?}"
    );
    assert!(
        text.contains("--- Sheet 1: 参数设置 ---"),
        "XLSX 提取结果应包含 Sheet 名称，实际: {text:?}"
    );
}

#[test]
fn test_extract_pdf_scanned_fallback_message() {
    let pdf_bytes = include_bytes!("fixtures/file_extractor/sample.pdf");
    let file = write_fixture(pdf_bytes, "pdf");
    let text = try_extract_text(file.path(), "application/pdf").expect("PDF 提取不应返回 None");

    assert_eq!(
        text, "[此文件可能为扫描件或图片型 PDF，暂不支持文字提取]",
        "扫描 PDF 应稳定返回回退提示"
    );
}

#[test]
fn test_extract_pptx_returns_expected_text() {
    let pptx_bytes = include_bytes!("fixtures/file_extractor/sample.pptx");
    let file = write_fixture(pptx_bytes, "pptx");
    let text = try_extract_text(
        file.path(),
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    )
    .expect("PPTX 提取不应返回 None");

    assert!(
        text.contains("实验1：解析模型示例") || text.contains("线性预测模型"),
        "PPTX 提取结果应包含演示文稿标题，实际: {text:?}"
    );
}

#[test]
fn test_extract_text_file_with_bom() {
    // UTF-8 BOM 头的纯文本应能被 read_text_with_mmap 正确解码
    let bom_text = "\u{FEFF}Hello, 文本提取测试";
    let mut file = NamedTempFile::with_suffix(".txt").expect("创建临时文件失败");
    file.write_all(bom_text.as_bytes())
        .expect("写入临时文件失败");
    let text = try_extract_text(file.path(), "text/plain").expect("文本文件提取不应返回 None");
    assert!(
        text.contains("Hello") && text.contains("文本提取测试"),
        "BOM 文本应被正确解码，实际: {text:?}"
    );
}

#[test]
fn test_extract_oversized_file_returns_warning() {
    // 构造一个超过 50MB 硬上限的稀疏文件，验证 OOM 防护返回提示而非崩溃
    let file = NamedTempFile::with_suffix(".txt").expect("创建临时文件失败");
    // 用 seek + write 末尾字节构造稀疏大文件，不实际占用磁盘空间
    use std::io::Seek;
    let oversized = 50 * 1024 * 1024 + 1; // 50MB + 1B
    let mut f = file.reopen().expect("reopen 失败");
    f.seek(std::io::SeekFrom::Start(oversized - 1))
        .expect("seek 失败");
    f.write_all(&[0u8]).expect("写入末尾字节失败");
    drop(f);

    let text = try_extract_text(file.path(), "text/plain")
        .expect("超大文件应返回 Some(提示文本) 而非 None");
    assert!(
        text.contains("文件过大") && text.contains("跳过自动提取"),
        "超大文件应返回 OOM 防护提示，实际: {text:?}"
    );
}
