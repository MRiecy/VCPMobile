//! Incremental Office text extraction. Budgets cover actual decompression and
//! parser/output buffers; original attachments remain available when extraction stops.

#[path = "office/markup.rs"]
mod markup;
#[path = "office/package.rs"]
mod package;
#[path = "office/presentation.rs"]
mod presentation;
#[path = "office/spreadsheet.rs"]
mod spreadsheet;
#[path = "office/word.rs"]
mod word;
#[path = "office/xml.rs"]
mod xml;

use std::path::Path;
use std::sync::Mutex;

const MAX_XML_BYTES: usize = 64 * 1024 * 1024;
const MAX_EXPANDED_BYTES: usize = 128 * 1024 * 1024;
const MAX_BUFFER_BYTES: usize = 8 * 1024 * 1024;
const MAX_SHARED_BYTES: usize = 16 * 1024 * 1024;
const MAX_SHARED_ITEMS: usize = 250_000;
const MAX_TEXT_CHARS: usize = 10_000_000;
const MAX_PARTS: usize = 4096;
const MAX_COLUMNS: usize = 16_384;
const TRUNCATED: &str = "……（文本过长已截断）";

// All callers already run extraction on blocking workers. Keep the permit in
// that worker, including when its async caller is cancelled or disappears.
static EXTRACTION_WORKER: Mutex<()> = Mutex::new(());

pub(super) fn extract(path: &Path, kind: &str) -> Option<String> {
    let _worker = EXTRACTION_WORKER.lock().unwrap_or_else(|e| e.into_inner());
    match extract_inner(path, kind) {
        Ok(text) => (!text.trim().is_empty()).then_some(text),
        Err(error) => {
            log::warn!(
                "[FileExtractor] Office extraction stopped: {:?}: {}",
                path,
                error
            );
            Some(format!("[Office 文本提取未完成：{error}；原附件已保留]"))
        }
    }
}

fn extract_inner(path: &Path, kind: &str) -> Result<String, String> {
    let mut package = package::Package::open(path)?;
    let main = package.main_part(kind)?;
    let mut output = TextOutput::new(MAX_TEXT_CHARS);
    match kind {
        "docx" => word::extract(&mut package, &main, &mut output)?,
        "pptx" => presentation::extract(&mut package, &main, &mut output)?,
        "xlsx" => spreadsheet::extract(&mut package, &main, &mut output)?,
        _ => return Err("不支持的 Office 格式".into()),
    }
    Ok(output.finish())
}

fn append(buffer: &mut String, value: &str) -> Result<(), String> {
    if value.len() > MAX_BUFFER_BYTES.saturating_sub(buffer.len()) {
        return Err("Office 段落、单元格或行超过提取预算".into());
    }
    buffer.push_str(value);
    Ok(())
}

struct TextOutput {
    text: String,
    remaining: usize,
    truncated: bool,
}

impl TextOutput {
    fn new(limit: usize) -> Self {
        Self {
            text: String::new(),
            remaining: limit,
            truncated: false,
        }
    }

    fn push(&mut self, value: &str) {
        if self.truncated {
            return;
        }
        let mut used = 0;
        let mut end = value.len();
        for (index, _) in value.char_indices() {
            if used == self.remaining {
                end = index;
                self.truncated = true;
                break;
            }
            used += 1;
        }
        self.text.push_str(&value[..end]);
        self.remaining -= used;
    }

    fn finish(mut self) -> String {
        if self.truncated {
            self.text.push_str(TRUNCATED);
        }
        self.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_stops_at_character_boundary_without_a_second_full_copy() {
        let mut output = TextOutput::new(3);
        output.push("中🙂a");
        output.push("b");
        assert_eq!(output.finish(), format!("中🙂a{TRUNCATED}"));
    }
}
