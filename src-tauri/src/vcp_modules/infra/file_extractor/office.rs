//! Incremental Office text extraction. Budgets cover actual decompression and
//! parser/output buffers; original attachments remain available when extraction stops.

use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{NamespaceResolver, ResolveResult};
use quick_xml::NsReader;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;
use std::sync::Mutex;
use zip::ZipArchive;

const MAX_XML_BYTES: usize = 64 * 1024 * 1024;
const MAX_EXPANDED_BYTES: usize = 128 * 1024 * 1024;
const MAX_BUFFER_BYTES: usize = 8 * 1024 * 1024;
const MAX_SHARED_BYTES: usize = 16 * 1024 * 1024;
const MAX_SHARED_ITEMS: usize = 250_000;
const MAX_TEXT_CHARS: usize = 10_000_000;
const MAX_PARTS: usize = 4096;
const MAX_COLUMNS: usize = 16_384;
const TRUNCATED: &str = "……（文本过长已截断）";
const OFFICE_RELATIONSHIPS: [&str; 2] = [
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    "http://purl.oclc.org/ooxml/officeDocument/relationships",
];

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
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|e| e.to_string())?;
    let mut remaining = MAX_EXPANDED_BYTES;
    let mut output = TextOutput::new(MAX_TEXT_CHARS);
    match kind {
        "docx" => docx(&mut archive, &mut remaining, &mut output)?,
        "pptx" => pptx(&mut archive, &mut remaining, &mut output)?,
        "xlsx" => xlsx(&mut archive, &mut remaining, &mut output)?,
        _ => return Err("不支持的 Office 格式".into()),
    }
    Ok(output.finish())
}

/// Reset `event_remaining` for each pull. The BufReader can retain at most
/// 8 KiB from the preceding event, so even an unterminated XML token is bounded.
struct BudgetReader<'a, R> {
    inner: R,
    remaining: &'a mut usize,
    part_remaining: usize,
    event_remaining: usize,
}

impl<R: Read> Read for BudgetReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let allowed = buffer
            .len()
            .min(*self.remaining)
            .min(self.part_remaining)
            .min(self.event_remaining);
        if allowed == 0 {
            // A single-byte probe distinguishes an exact budget-sized EOF from
            // more decompressed data; it never enters a growing parser buffer.
            let mut probe = [0];
            if self.inner.read(&mut probe)? == 0 {
                return Ok(0);
            }
            return Err(io::Error::other("Office 解压或 XML 事件超过提取预算"));
        }
        let read = self.inner.read(&mut buffer[..allowed])?;
        *self.remaining -= read;
        self.part_remaining -= read;
        self.event_remaining -= read;
        Ok(read)
    }
}

fn read_part(
    archive: &mut ZipArchive<File>,
    name: &str,
    remaining: &mut usize,
    mut consume: impl FnMut(Event<'_>, &NamespaceResolver) -> Result<bool, String>,
) -> Result<bool, String> {
    let file = match archive.by_name(name) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    if file.size() > MAX_XML_BYTES as u64 || file.size() > *remaining as u64 {
        return Err("Office XML 超过解压预算".into());
    }
    let source = BudgetReader {
        inner: file,
        remaining,
        part_remaining: MAX_XML_BYTES,
        event_remaining: MAX_BUFFER_BYTES,
    };
    let mut reader = NsReader::from_reader(BufReader::with_capacity(8192, source));
    reader.config_mut().expand_empty_elements = true;
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    loop {
        buffer.clear();
        reader.get_mut().get_mut().event_remaining = MAX_BUFFER_BYTES;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|e| e.to_string())?;
        match &event {
            Event::Start(_) => {
                depth += 1;
                if depth > 128 {
                    return Err("Office XML 嵌套层数超过提取预算".into());
                }
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::DocType(_) => return Err("Office XML 不支持 DTD".into()),
            Event::Eof => break,
            _ => {}
        }
        if !consume(event, reader.resolver())? {
            break;
        }
    }
    Ok(true)
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

fn attribute(start: &BytesStart<'_>, name: &str) -> Result<Option<String>, String> {
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|e| e.to_string())?;
        if attribute.key.local_name().as_ref() == name {
            return attribute
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map(|value| Some(value.into_owned()))
                .map_err(|e| e.to_string());
        }
    }
    Ok(None)
}

fn text<'a>(event: &'a Event<'_>) -> Result<Option<Cow<'a, str>>, String> {
    Ok(match event {
        Event::Text(value) => Some(value.xml10_content()),
        Event::CData(value) => Some(value.xml10_content()),
        Event::GeneralRef(value) => {
            if let Some(character) = value.resolve_char_ref().map_err(|e| e.to_string())? {
                Some(Cow::Owned(character.to_string()))
            } else {
                let value = match value.as_ref() {
                    "amp" => "&",
                    "lt" => "<",
                    "gt" => ">",
                    "apos" => "'",
                    "quot" => "\"",
                    _ => return Err("Office XML 包含未知实体".into()),
                };
                Some(Cow::Borrowed(value))
            }
        }
        _ => None,
    })
}

struct ManifestPart {
    relationship_id: String,
    sheet: Option<(u32, String)>,
}

struct PartRelationship {
    kind: String,
    target: String,
    external: bool,
}

impl PartRelationship {
    fn is_kind(&self, kind: &str) -> bool {
        OFFICE_RELATIONSHIPS.iter().any(|namespace| {
            self.kind
                .strip_prefix(namespace)
                .and_then(|suffix| suffix.strip_prefix('/'))
                == Some(kind)
        })
    }

    fn internal_target(&self, manifest: &str) -> Result<String, String> {
        if self.external {
            return Err("Office 工作表或幻灯片引用了外部目标".into());
        }
        // Resolve package URIs only. The result is used by ZipArchive::by_name,
        // never as a filesystem path or a network URL.
        if self.target.is_empty() || self.target.contains(['\\', ':', '?', '#']) {
            return Err("Office 关系目标不是有效的包内路径".into());
        }
        let (directory, _) = manifest.rsplit_once('/').ok_or("Office 主文档路径无效")?;
        let mut components: Vec<&str> = if self.target.starts_with('/') {
            Vec::new()
        } else {
            directory.split('/').collect()
        };
        for component in self
            .target
            .strip_prefix('/')
            .unwrap_or(&self.target)
            .split('/')
        {
            match component {
                "" => return Err("Office 关系目标不是有效的包内路径".into()),
                "." => {}
                ".." => {
                    components.pop().ok_or("Office 关系目标超出包内根目录")?;
                }
                _ => components.push(component),
            }
        }
        if components.is_empty() {
            return Err("Office 关系目标缺少部件名称".into());
        }
        Ok(components.join("/"))
    }
}

fn relationship_id(
    start: &BytesStart<'_>,
    namespaces: &NamespaceResolver,
) -> Result<String, String> {
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|e| e.to_string())?;
        let (namespace, name) = namespaces.resolve_attribute(attribute.key);
        if name.as_ref() == "id"
            && matches!(namespace, ResolveResult::Bound(namespace) if OFFICE_RELATIONSHIPS.contains(&namespace.as_ref()))
        {
            let id = attribute
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map_err(|e| e.to_string())?
                .into_owned();
            if !id.is_empty() {
                return Ok(id);
            }
        }
    }
    Err("Office 主文档部件缺少关系 ID".into())
}

fn reserve_metadata(bytes: &mut usize, added: usize) -> Result<(), String> {
    if added > MAX_BUFFER_BYTES.saturating_sub(*bytes) {
        return Err("Office 部件清单或关系表超过提取预算".into());
    }
    *bytes += added;
    Ok(())
}

fn manifest_parts(
    archive: &mut ZipArchive<File>,
    remaining: &mut usize,
    metadata_bytes: &mut usize,
    manifest: &str,
    list_name: &str,
    part_name: &str,
) -> Result<Vec<ManifestPart>, String> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut in_list = false;
    if !read_part(archive, manifest, remaining, |event, namespaces| {
        match event {
            Event::Start(start) => {
                depth += 1;
                if depth == 2 && start.local_name().as_ref() == list_name {
                    in_list = true;
                } else if depth == 3 && in_list && start.local_name().as_ref() == part_name {
                    if parts.len() >= MAX_PARTS {
                        return Err("Office 工作表或幻灯片数量超过提取预算".into());
                    }
                    let relationship_id = relationship_id(&start, namespaces)?;
                    reserve_metadata(metadata_bytes, relationship_id.len())?;
                    let sheet = if part_name == "sheet" {
                        let id = attribute(&start, "sheetId")?
                            .ok_or("XLSX 工作表缺少 sheetId")?
                            .parse::<u32>()
                            .map_err(|_| "XLSX 工作表 sheetId 无效")?;
                        let title = attribute(&start, "name")?.ok_or("XLSX 工作表缺少名称")?;
                        reserve_metadata(metadata_bytes, title.len())?;
                        Some((id, title))
                    } else {
                        None
                    };
                    parts.push(ManifestPart {
                        relationship_id,
                        sheet,
                    });
                }
            }
            Event::End(_) => {
                if depth == 2 {
                    in_list = false;
                }
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
        Ok(true)
    })? {
        return Err(format!("Office 缺少主文档 {manifest}"));
    }
    Ok(parts)
}

fn relationships(
    archive: &mut ZipArchive<File>,
    remaining: &mut usize,
    metadata_bytes: &mut usize,
    manifest: &str,
) -> Result<HashMap<String, PartRelationship>, String> {
    let (directory, filename) = manifest.rsplit_once('/').ok_or("Office 主文档路径无效")?;
    let path = format!("{directory}/_rels/{filename}.rels");
    let mut relationships = HashMap::new();
    if !read_part(archive, &path, remaining, |event, _| {
        if let Event::Start(start) = event {
            if start.local_name().as_ref() == "Relationship" {
                if relationships.len() >= MAX_PARTS {
                    return Err("Office 关系数量超过提取预算".into());
                }
                let id = attribute(&start, "Id")?.ok_or("Office 关系缺少 Id")?;
                let kind = attribute(&start, "Type")?.ok_or("Office 关系缺少 Type")?;
                let target = attribute(&start, "Target")?.ok_or("Office 关系缺少 Target")?;
                let external = match attribute(&start, "TargetMode")?.as_deref() {
                    None | Some("Internal") => false,
                    Some("External") => true,
                    _ => return Err("Office 关系 TargetMode 无效".into()),
                };
                reserve_metadata(metadata_bytes, id.len() + kind.len() + target.len())?;
                if relationships
                    .insert(
                        id,
                        PartRelationship {
                            kind,
                            target,
                            external,
                        },
                    )
                    .is_some()
                {
                    return Err("Office 关系 ID 重复".into());
                }
            }
        }
        Ok(true)
    })? {
        return Err(format!("Office 缺少关系表 {path}"));
    }
    Ok(relationships)
}

fn heading(value: &str) -> usize {
    let value = value.to_lowercase();
    if value.contains("heading") {
        return (1..=4)
            .find(|level| value.contains(&level.to_string()))
            .unwrap_or(0);
    }
    if value.contains("title") {
        return 1;
    }
    (1..=3)
        .find(|level| value == level.to_string() || value == format!("标题 {level}"))
        .unwrap_or(0)
}

fn docx(
    archive: &mut ZipArchive<File>,
    remaining: &mut usize,
    output: &mut TextOutput,
) -> Result<(), String> {
    let mut paragraph = String::new();
    let mut cell = String::new();
    let mut cells = Vec::<String>::new();
    let mut row_bytes = 0;
    let mut first_row = true;
    let mut in_cell = false;
    let mut in_text = false;
    let mut in_properties = false;
    let mut bold = false;
    let mut bold_open = false;
    let mut level = 0;
    let mut list = false;
    if !read_part(archive, "word/document.xml", remaining, |event, _| {
        match &event {
            Event::Start(start) => match start.local_name().as_ref() {
                "t" => in_text = true,
                "pPr" => in_properties = true,
                "pStyle" if in_properties => {
                    level = heading(&attribute(start, "val")?.unwrap_or_default())
                }
                "numPr" if in_properties => list = true,
                "r" => bold = false,
                "b" | "bCs" => {
                    bold = !matches!(
                        attribute(start, "val")?.as_deref(),
                        Some("0" | "false" | "off")
                    )
                }
                "tbl" => {
                    first_row = true;
                    output.push("\n");
                }
                "tc" => {
                    in_cell = true;
                    cell.clear();
                }
                "tab" => append(&mut paragraph, "\t")?,
                "br" | "cr" => append(&mut paragraph, "\n")?,
                _ => {}
            },
            Event::End(end) => match end.local_name().as_ref() {
                "t" => in_text = false,
                "pPr" => in_properties = false,
                "p" => {
                    if bold_open {
                        append(&mut paragraph, "**")?;
                        bold_open = false;
                    }
                    let paragraph_text = paragraph.trim();
                    if !paragraph_text.is_empty() {
                        let prefix = if level > 0 {
                            format!("\n{} ", "#".repeat(level))
                        } else if list {
                            "- ".to_string()
                        } else {
                            String::new()
                        };
                        if in_cell {
                            append(&mut cell, &prefix)?;
                            append(&mut cell, paragraph_text)?;
                        } else {
                            output.push(&prefix);
                            output.push(paragraph_text);
                            output.push("\n");
                        }
                    } else if !in_cell {
                        output.push("\n");
                    }
                    paragraph.clear();
                    level = 0;
                    list = false;
                }
                "tc" => {
                    in_cell = false;
                    row_bytes += cell.len();
                    if row_bytes > MAX_BUFFER_BYTES || cells.len() >= MAX_COLUMNS {
                        return Err("Office 表格行超过提取预算".into());
                    }
                    cells.push(std::mem::take(&mut cell));
                }
                "tr" => {
                    if !cells.is_empty() {
                        output.push("| ");
                        for (index, cell) in cells.iter().enumerate() {
                            if index > 0 {
                                output.push(" | ");
                            }
                            output.push(cell.trim());
                        }
                        output.push(" |\n");
                        if first_row {
                            output.push("| ");
                            for index in 0..cells.len() {
                                if index > 0 {
                                    output.push(" | ");
                                }
                                output.push("---");
                            }
                            output.push(" |\n");
                        }
                        first_row = false;
                        cells.clear();
                        row_bytes = 0;
                    }
                }
                "tbl" => output.push("\n"),
                _ => {}
            },
            _ => {
                if in_text {
                    if let Some(value) = text(&event)? {
                        if bold_open != bold {
                            append(&mut paragraph, "**")?;
                            bold_open = bold;
                        }
                        append(&mut paragraph, &value)?;
                    }
                }
            }
        }
        Ok(!output.truncated)
    })? {
        return Err("DOCX 缺少 word/document.xml".into());
    }
    Ok(())
}

fn pptx(
    archive: &mut ZipArchive<File>,
    remaining: &mut usize,
    output: &mut TextOutput,
) -> Result<(), String> {
    let manifest = "ppt/presentation.xml";
    let mut metadata_bytes = 0;
    let slides = manifest_parts(
        archive,
        remaining,
        &mut metadata_bytes,
        manifest,
        "sldIdLst",
        "sldId",
    )?;
    let relationships = relationships(archive, remaining, &mut metadata_bytes, manifest)?;
    for (index, part) in slides.into_iter().enumerate() {
        let relationship = relationships
            .get(&part.relationship_id)
            .ok_or("PPTX 幻灯片关系缺失")?;
        if !relationship.is_kind("slide") {
            return Err("PPTX 幻灯片关系类型无效".into());
        }
        let name = relationship.internal_target(manifest)?;
        let mut slide = String::new();
        let mut in_text = false;
        if !read_part(archive, &name, remaining, |event, _| {
            match &event {
                Event::Start(start) => match start.local_name().as_ref() {
                    "t" => in_text = true,
                    "br" => append(&mut slide, "\n")?,
                    _ => {}
                },
                Event::End(end) => match end.local_name().as_ref() {
                    "t" => in_text = false,
                    "p" => append(&mut slide, "\n")?,
                    _ => {}
                },
                _ => {
                    if in_text {
                        if let Some(value) = text(&event)? {
                            append(&mut slide, &value)?;
                        }
                    }
                }
            }
            Ok(true)
        })? {
            return Err(format!("PPTX 缺少幻灯片部件 {name}"));
        }
        if !slide.trim().is_empty() {
            output.push(&format!("\n--- Slide {} ---\n", index + 1));
            output.push(&slide);
        }
        if output.truncated {
            break;
        }
    }
    Ok(())
}

/// A rich string is one logical item (`si` or `is`), regardless of run count.
/// Only direct text and r/t contribute, excluding phonetic annotation text.
#[derive(Default)]
struct RichString {
    depth: usize,
    run_depth: Option<usize>,
    text_depth: Option<usize>,
    value: String,
}

impl RichString {
    fn event(&mut self, event: &Event<'_>, root: &str) -> Result<Option<String>, String> {
        match event {
            Event::Start(start) => {
                let name = start.local_name();
                if self.depth == 0 {
                    if name.as_ref() == root {
                        self.depth = 1;
                        self.value.clear();
                    }
                } else {
                    self.depth += 1;
                    if name.as_ref() == "r" && self.depth == 2 {
                        self.run_depth = Some(2);
                    }
                    if name.as_ref() == "t"
                        && (self.depth == 2 || (self.depth == 3 && self.run_depth == Some(2)))
                    {
                        self.text_depth = Some(self.depth);
                    }
                }
            }
            Event::End(_) if self.depth > 0 => {
                if self.text_depth == Some(self.depth) {
                    self.text_depth = None;
                }
                if self.run_depth == Some(self.depth) {
                    self.run_depth = None;
                }
                self.depth -= 1;
                if self.depth == 0 {
                    return Ok(Some(std::mem::take(&mut self.value)));
                }
            }
            _ if self.text_depth.is_some() => {
                if let Some(value) = text(event)? {
                    append(&mut self.value, &value)?;
                }
            }
            _ => {}
        }
        Ok(None)
    }
}

fn column(reference: &str) -> Result<usize, String> {
    let mut index = 0usize;
    for character in reference.chars().take_while(|c| c.is_ascii_alphabetic()) {
        index = index
            .checked_mul(26)
            .and_then(|v| v.checked_add(character.to_ascii_uppercase() as usize - 'A' as usize + 1))
            .filter(|value| *value <= MAX_COLUMNS)
            .ok_or("XLSX 列坐标超出范围")?;
    }
    index
        .checked_sub(1)
        .ok_or_else(|| "XLSX 单元格缺少列坐标".into())
}

fn xlsx(
    archive: &mut ZipArchive<File>,
    remaining: &mut usize,
    output: &mut TextOutput,
) -> Result<(), String> {
    let manifest = "xl/workbook.xml";
    let mut metadata_bytes = 0;
    let sheets = manifest_parts(
        archive,
        remaining,
        &mut metadata_bytes,
        manifest,
        "sheets",
        "sheet",
    )?;
    let relationships = relationships(archive, remaining, &mut metadata_bytes, manifest)?;
    let mut shared = Vec::new();
    let mut shared_bytes = 0usize;
    let mut rich = RichString::default();
    read_part(archive, "xl/sharedStrings.xml", remaining, |event, _| {
        if let Some(value) = rich.event(&event, "si")? {
            if shared.len() >= MAX_SHARED_ITEMS
                || value.len() > MAX_SHARED_BYTES.saturating_sub(shared_bytes)
            {
                return Err("XLSX 共享字符串表超过提取预算".into());
            }
            shared_bytes += value.len();
            shared.push(value);
        }
        Ok(true)
    })?;
    for part in sheets {
        let relationship = relationships
            .get(&part.relationship_id)
            .ok_or("XLSX 工作表关系缺失")?;
        // Chart/dialog sheets have no worksheet cells; retain the existing scope
        // of text extraction without interpreting their XML as a worksheet.
        if relationship.is_kind("chartsheet") || relationship.is_kind("dialogsheet") {
            continue;
        }
        if !relationship.is_kind("worksheet") {
            return Err("XLSX 工作表关系类型无效".into());
        }
        let name = relationship.internal_target(manifest)?;
        let (id, title) = part.sheet.ok_or("XLSX 工作表描述缺失")?;
        let mut emitted_header = false;
        let mut row = Vec::<(usize, String)>::new();
        let mut row_bytes = 0usize;
        let mut next_column = 0usize;
        let mut cell_column = 0usize;
        let mut cell_type = String::new();
        let mut cell_value = String::new();
        let mut value_present = false;
        let mut in_value = false;
        let mut inline = RichString::default();
        if !read_part(archive, &name, remaining, |event, _| {
            if let Some(value) = inline.event(&event, "is")? {
                cell_value = value;
                value_present = true;
            }
            match &event {
                Event::Start(start) => match start.local_name().as_ref() {
                    "row" => {
                        row.clear();
                        row_bytes = 0;
                        next_column = 0;
                    }
                    "c" => {
                        cell_type = attribute(start, "t")?.unwrap_or_default();
                        cell_column = match attribute(start, "r")? {
                            Some(reference) => column(&reference)?,
                            None => next_column,
                        };
                        if cell_column >= MAX_COLUMNS {
                            return Err("XLSX 列坐标超出范围".into());
                        }
                        next_column = cell_column + 1;
                        cell_value.clear();
                        value_present = false;
                    }
                    "v" => {
                        in_value = true;
                        value_present = true;
                    }
                    _ => {}
                },
                Event::End(end) => match end.local_name().as_ref() {
                    "v" => in_value = false,
                    "c" if value_present => {
                        let value = if cell_type == "s" {
                            let index = cell_value
                                .trim()
                                .parse::<usize>()
                                .map_err(|_| "XLSX 共享字符串索引无效")?;
                            shared.get(index).ok_or("XLSX 共享字符串索引越界")?.clone()
                        } else if cell_type == "inlineStr" {
                            std::mem::take(&mut cell_value)
                        } else {
                            cell_value.trim().to_string()
                        };
                        if value.len() > MAX_BUFFER_BYTES.saturating_sub(row_bytes)
                            || row.len() >= MAX_COLUMNS
                        {
                            return Err("XLSX 行超过提取预算".into());
                        }
                        row_bytes += value.len();
                        row.push((cell_column, value));
                    }
                    "row" if !row.is_empty() => {
                        if !emitted_header {
                            output.push(&format!("\n--- Sheet {id}: {title} ---\n"));
                            emitted_header = true;
                        }
                        row.sort_by_key(|(column, _)| *column);
                        let mut previous = None;
                        for (column, value) in &row {
                            let tabs = previous
                                .map_or(*column, |previous| column.saturating_sub(previous));
                            for _ in 0..tabs {
                                output.push("\t");
                            }
                            output.push(value);
                            previous = Some(*column);
                        }
                        output.push("\n");
                        row.clear();
                        row_bytes = 0;
                    }
                    _ => {}
                },
                _ if in_value => {
                    if let Some(value) = text(&event)? {
                        append(&mut cell_value, &value)?;
                    }
                }
                _ => {}
            }
            Ok(!output.truncated)
        })? {
            return Err(format!("XLSX 缺少工作表部件 {name}"));
        }
        if output.truncated {
            break;
        }
    }
    Ok(())
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

    #[test]
    fn actual_read_budget_stops_before_collecting_expanded_input() {
        let mut remaining = 32;
        let mut reader = BudgetReader {
            inner: io::Cursor::new(vec![b'x'; 1024]),
            remaining: &mut remaining,
            part_remaining: 32,
            event_remaining: 32,
        };
        let mut output = Vec::new();
        assert!(reader.read_to_end(&mut output).is_err());
        assert_eq!(output.len(), 32);
    }

    #[test]
    fn exact_budget_eof_is_valid() {
        let mut remaining = 32;
        let mut reader = BudgetReader {
            inner: io::Cursor::new(vec![b'x'; 32]),
            remaining: &mut remaining,
            part_remaining: 32,
            event_remaining: 32,
        };
        let mut output = Vec::new();
        reader.read_to_end(&mut output).unwrap();
        assert_eq!(output.len(), 32);
    }
}
