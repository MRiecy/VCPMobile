//! SpreadsheetML text extraction through package relationships and structural XML paths.
//!
//! Values are the stored values (including formula caches), not recalculated or styled.
use super::package::Package;
use super::xml::{self, Namespace};
use super::{
    append, TextOutput, MAX_BUFFER_BYTES, MAX_COLUMNS, MAX_PARTS, MAX_SHARED_BYTES,
    MAX_SHARED_ITEMS,
};
use quick_xml::events::Event;
use quick_xml::name::NamespaceResolver;
use std::borrow::Cow;
use std::collections::HashSet;

struct Sheet {
    id: u32,
    title: String,
    relationship_id: String,
}

fn sheets(package: &mut Package, main: &str) -> Result<Vec<Sheet>, String> {
    let mut sheets = Vec::new();
    let mut in_sheets = false;
    let mut metadata_bytes = 0usize;
    let mut ids = HashSet::new();
    package.read_part(
        main,
        (Namespace::Spreadsheet, "workbook"),
        |event, namespaces, depth| {
            match event {
                Event::Start(start)
                    if xml::in_namespace(start.name(), namespaces, Namespace::Spreadsheet) =>
                {
                    match (depth, start.local_name().as_ref()) {
                        (2, "sheets") => in_sheets = true,
                        (3, "sheet") if in_sheets => {
                            if sheets.len() >= MAX_PARTS {
                                return Err("Office 工作表数量超过提取预算".into());
                            }
                            let relationship_id = xml::qualified_attribute(
                                &start,
                                namespaces,
                                Namespace::OfficeRelationships,
                                "id",
                            )?
                            .filter(|value| !value.is_empty())
                            .ok_or("XLSX 工作表缺少关系 ID")?;
                            let id = xml::attribute(&start, "sheetId")?
                                .ok_or("XLSX 工作表缺少 sheetId")?
                                .parse::<u32>()
                                .map_err(|_| "XLSX 工作表 sheetId 无效")?;
                            if !ids.insert(id) {
                                return Err("XLSX 工作表 sheetId 重复".into());
                            }
                            let raw_title =
                                xml::attribute(&start, "name")?.ok_or("XLSX 工作表缺少名称")?;
                            let title = decode_xstring(&raw_title)?.into_owned();
                            let added = relationship_id.len().saturating_add(title.len());
                            if added > MAX_BUFFER_BYTES.saturating_sub(metadata_bytes) {
                                return Err("Office 部件清单或关系表超过提取预算".into());
                            }
                            metadata_bytes += added;
                            sheets.push(Sheet {
                                id,
                                title,
                                relationship_id,
                            });
                        }
                        _ => {}
                    }
                }
                Event::End(_) if depth == 2 => in_sheets = false,
                _ => {}
            }
            Ok(true)
        },
    )?;
    package.reserve_metadata(metadata_bytes)?;
    Ok(sheets)
}

/// One `si` / `is` item may contain several runs, but phonetic annotations are
/// separate metadata. Decode each text node before joining runs: an escape-like
/// sequence split across two runs must not become a new escape after joining.
#[derive(Default)]
struct RichString {
    root_depth: Option<usize>,
    run_depth: Option<usize>,
    text_depth: Option<usize>,
    raw_text: String,
    value: String,
}

impl RichString {
    fn begin(&mut self, depth: usize) {
        self.root_depth = Some(depth);
        self.run_depth = None;
        self.text_depth = None;
        self.raw_text.clear();
        self.value.clear();
    }

    fn event(
        &mut self,
        event: &Event<'_>,
        namespaces: &NamespaceResolver,
        depth: usize,
    ) -> Result<Option<String>, String> {
        let Some(root) = self.root_depth else {
            return Ok(None);
        };
        match event {
            Event::Start(start)
                if xml::in_namespace(start.name(), namespaces, Namespace::Spreadsheet) =>
            {
                let local = start.local_name();
                if local.as_ref() == "r" && depth == root + 1 {
                    self.run_depth = Some(depth);
                } else if local.as_ref() == "t"
                    && (depth == root + 1
                        || (depth == root + 2 && self.run_depth == Some(root + 1)))
                {
                    self.text_depth = Some(depth);
                    self.raw_text.clear();
                }
            }
            Event::End(_) => {
                if self.text_depth == Some(depth) {
                    append(&mut self.value, &decode_xstring(&self.raw_text)?)?;
                    self.raw_text.clear();
                    self.text_depth = None;
                }
                if self.run_depth == Some(depth) {
                    self.run_depth = None;
                }
                if root == depth {
                    self.root_depth = None;
                    return Ok(Some(std::mem::take(&mut self.value)));
                }
            }
            _ if self.text_depth == Some(depth) => {
                if let Some(text) = xml::text(event)? {
                    append(&mut self.raw_text, &text)?;
                }
            }
            _ => {}
        }
        Ok(None)
    }
}

fn shared_strings(package: &mut Package, name: &str) -> Result<Vec<String>, String> {
    let mut shared = Vec::new();
    let mut bytes = 0usize;
    let mut rich = RichString::default();
    package.read_part(
        name,
        (Namespace::Spreadsheet, "sst"),
        |event, namespaces, depth| {
            if let Event::Start(start) = &event {
                if depth == 2
                    && start.local_name().as_ref() == "si"
                    && xml::in_namespace(start.name(), namespaces, Namespace::Spreadsheet)
                {
                    rich.begin(depth);
                }
            }
            if let Some(value) = rich.event(&event, namespaces, depth)? {
                if shared.len() >= MAX_SHARED_ITEMS
                    || value.len() > MAX_SHARED_BYTES.saturating_sub(bytes)
                {
                    return Err("XLSX 共享字符串表超过提取预算".into());
                }
                bytes += value.len();
                shared.push(value);
            }
            Ok(true)
        },
    )?;
    Ok(shared)
}

#[derive(Default)]
struct Cell {
    column: usize,
    kind: String,
    value: String,
    present: bool,
    in_value: bool,
    inline: RichString,
}

impl Cell {
    fn finish(
        &mut self,
        shared: &[String],
        has_shared_table: bool,
    ) -> Result<Option<String>, String> {
        if !self.present {
            return Ok(None);
        }
        let value = match self.kind.as_str() {
            "s" => {
                if !has_shared_table {
                    return Err("XLSX 单元格引用了缺失的共享字符串表".into());
                }
                let index = self
                    .value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| "XLSX 共享字符串索引无效")?;
                shared.get(index).ok_or("XLSX 共享字符串索引越界")?.clone()
            }
            "inlineStr" => std::mem::take(&mut self.value),
            "str" => decode_xstring(&self.value)?.into_owned(),
            _ => self.value.trim().to_string(),
        };
        Ok(Some(value))
    }
}

fn worksheet(
    package: &mut Package,
    name: &str,
    sheet: &Sheet,
    shared: &[String],
    has_shared_table: bool,
    output: &mut TextOutput,
) -> Result<(), String> {
    let mut in_data = false;
    let mut in_row = false;
    let mut cell: Option<Cell> = None;
    let mut next_column = 0usize;
    let mut row = Vec::<(usize, String)>::new();
    let mut row_bytes = 0usize;
    let mut emitted_header = false;
    package.read_part(
        name,
        (Namespace::Spreadsheet, "worksheet"),
        |event, namespaces, depth| {
            if let Some(cell) = cell.as_mut() {
                if let Some(value) = cell.inline.event(&event, namespaces, depth)? {
                    cell.value = value;
                    cell.present = true;
                }
            }
            match &event {
                Event::Start(start)
                    if xml::in_namespace(start.name(), namespaces, Namespace::Spreadsheet) =>
                {
                    match (depth, start.local_name().as_ref()) {
                        (2, "sheetData") => in_data = true,
                        (3, "row") if in_data => {
                            in_row = true;
                            next_column = 0;
                            row.clear();
                            row_bytes = 0;
                        }
                        (4, "c") if in_row => {
                            let kind = xml::attribute(start, "t")?.unwrap_or_default();
                            let column = match xml::attribute(start, "r")? {
                                Some(reference) => column(&reference)?,
                                None => next_column,
                            };
                            if column >= MAX_COLUMNS {
                                return Err("XLSX 列坐标超出范围".into());
                            }
                            next_column = column + 1;
                            cell = Some(Cell {
                                column,
                                kind,
                                ..Cell::default()
                            });
                        }
                        (5, "v") => {
                            if let Some(cell) =
                                cell.as_mut().filter(|cell| cell.kind != "inlineStr")
                            {
                                cell.present = true;
                                cell.in_value = true;
                            }
                        }
                        (5, "is") => {
                            if let Some(cell) =
                                cell.as_mut().filter(|cell| cell.kind == "inlineStr")
                            {
                                cell.inline.begin(depth);
                            }
                        }
                        _ => {}
                    }
                }
                Event::End(end)
                    if xml::in_namespace(end.name(), namespaces, Namespace::Spreadsheet) =>
                {
                    match (depth, end.local_name().as_ref()) {
                        (5, "v") => {
                            if let Some(cell) = cell.as_mut() {
                                cell.in_value = false;
                            }
                        }
                        (4, "c") if in_row => {
                            if let Some(mut cell) = cell.take() {
                                if let Some(value) = cell.finish(shared, has_shared_table)? {
                                    if row.len() >= MAX_COLUMNS
                                        || value.len() > MAX_BUFFER_BYTES.saturating_sub(row_bytes)
                                    {
                                        return Err("XLSX 行超过提取预算".into());
                                    }
                                    row_bytes += value.len();
                                    row.push((cell.column, value));
                                }
                            }
                        }
                        (3, "row") if in_row => {
                            if !row.is_empty() {
                                if !emitted_header {
                                    output.push(&format!(
                                        "\n--- Sheet {}: {} ---\n",
                                        sheet.id, sheet.title
                                    ));
                                    emitted_header = true;
                                }
                                row.sort_by_key(|(column, _)| *column);
                                let mut previous = None;
                                for (column, value) in &row {
                                    if previous == Some(*column) {
                                        return Err("XLSX 行包含重复列坐标".into());
                                    }
                                    let tabs =
                                        previous.map_or(*column, |previous| column - previous);
                                    for _ in 0..tabs {
                                        output.push("\t");
                                    }
                                    output.push(value);
                                    previous = Some(*column);
                                }
                                output.push("\n");
                            }
                            in_row = false;
                            row.clear();
                        }
                        (2, "sheetData") => in_data = false,
                        _ => {}
                    }
                }
                _ if depth == 5 => {
                    if let Some(cell) = cell.as_mut().filter(|cell| cell.in_value) {
                        if let Some(text) = xml::text(&event)? {
                            append(&mut cell.value, &text)?;
                        }
                    }
                }
                _ => {}
            }
            Ok(!output.truncated)
        },
    )
}

pub(super) fn extract(
    package: &mut Package,
    main: &str,
    output: &mut TextOutput,
) -> Result<(), String> {
    let sheets = sheets(package, main)?;
    let relationships = package.relationships(main)?;
    let shared_path = relationships
        .optional_unique("sharedStrings")?
        .map(|relationship| relationship.target(main))
        .transpose()?;
    let shared = match &shared_path {
        Some(path) => shared_strings(package, path)?,
        None => Vec::new(),
    };
    for sheet in sheets {
        let relationship = relationships.get(&sheet.relationship_id)?;
        // Chart/dialog sheets have no worksheet cells to extract.
        if relationship.is_kind("chartsheet") || relationship.is_kind("dialogsheet") {
            continue;
        }
        let relationship = relationships.by_id(&sheet.relationship_id, "worksheet")?;
        let name = relationship.target(main)?;
        worksheet(
            package,
            &name,
            &sheet,
            &shared,
            shared_path.is_some(),
            output,
        )?;
        if output.truncated {
            break;
        }
    }
    Ok(())
}

fn column(reference: &str) -> Result<usize, String> {
    let letters = reference
        .bytes()
        .take_while(u8::is_ascii_alphabetic)
        .count();
    if letters == 0 || letters == reference.len() {
        return Err("XLSX 单元格坐标无效".into());
    }
    let row = &reference[letters..];
    if !row.bytes().all(|byte| byte.is_ascii_digit())
        || !row
            .parse::<u32>()
            .is_ok_and(|row| (1..=1_048_576).contains(&row))
    {
        return Err("XLSX 单元格行坐标无效".into());
    }
    let mut column = 0usize;
    for character in reference[..letters].bytes() {
        column = column
            .checked_mul(26)
            .and_then(|value| {
                value.checked_add(usize::from(character.to_ascii_uppercase() - b'A') + 1)
            })
            .filter(|value| *value <= MAX_COLUMNS)
            .ok_or("XLSX 列坐标超出范围")?;
    }
    Ok(column - 1)
}

/// ST_Xstring uses UTF-16 code units. A literal escape is encoded by escaping
/// its leading underscore; scanning once preserves it instead of recursively decoding.
fn decode_xstring(value: &str) -> Result<Cow<'_, str>, String> {
    fn unit(bytes: &[u8]) -> Option<u16> {
        let escape = bytes.get(..7)?;
        if &escape[..2] != b"_x" || escape[6] != b'_' {
            return None;
        }
        let mut unit = 0u16;
        for digit in &escape[2..6] {
            unit = unit.checked_mul(16)?
                + match digit {
                    b'0'..=b'9' => u16::from(digit - b'0'),
                    b'a'..=b'f' => u16::from(digit - b'a') + 10,
                    b'A'..=b'F' => u16::from(digit - b'A') + 10,
                    _ => return None,
                };
        }
        Some(unit)
    }
    if !value.contains("_x") {
        return Ok(Cow::Borrowed(value));
    }
    let mut result = String::with_capacity(value.len());
    let mut offset = 0usize;
    while offset < value.len() {
        if let Some(first) = unit(&value.as_bytes()[offset..]) {
            offset += 7;
            let scalar = if (0xd800..=0xdbff).contains(&first) {
                let second = unit(&value.as_bytes()[offset..])
                    .filter(|unit| (0xdc00..=0xdfff).contains(unit))
                    .ok_or("XLSX 字符串包含未配对的 UTF-16 代理项")?;
                offset += 7;
                0x10000 + ((u32::from(first) - 0xd800) << 10) + u32::from(second) - 0xdc00
            } else if (0xdc00..=0xdfff).contains(&first) {
                return Err("XLSX 字符串包含未配对的 UTF-16 代理项".into());
            } else {
                u32::from(first)
            };
            result.push(char::from_u32(scalar).ok_or("XLSX 字符串包含无效 Unicode 字符")?);
        } else {
            let character = value[offset..].chars().next().ok_or("XLSX 字符串无效")?;
            result.push(character);
            offset += character.len_utf8();
        }
    }
    Ok(Cow::Owned(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_rich_items(source: &str) -> Vec<String> {
        let mut reader = quick_xml::NsReader::from_reader(source.as_bytes());
        reader.config_mut().expand_empty_elements = true;
        let mut depth = 0usize;
        let mut buffer = Vec::new();
        let mut rich = RichString::default();
        let mut values = Vec::new();
        loop {
            let event = reader.read_event_into(&mut buffer).unwrap();
            if matches!(event, Event::Start(_)) {
                depth += 1;
            }
            if let Event::Start(start) = &event {
                if depth == 2
                    && start.local_name().as_ref() == "si"
                    && xml::in_namespace(start.name(), reader.resolver(), Namespace::Spreadsheet)
                {
                    rich.begin(depth);
                }
            }
            if let Some(value) = rich.event(&event, reader.resolver(), depth).unwrap() {
                values.push(value);
            }
            if matches!(event, Event::End(_)) {
                depth -= 1;
            }
            if matches!(event, Event::Eof) {
                break;
            }
            buffer.clear();
        }
        values
    }

    #[test]
    fn escaped_strings_decode_once_and_preserve_literals() {
        assert_eq!(decode_xstring("中_x000D_文").unwrap(), "中\r文");
        assert_eq!(decode_xstring("_x005F_x0041_").unwrap(), "_x0041_");
        assert_eq!(decode_xstring("_x005f_x000D_").unwrap(), "_x000D_");
        assert_eq!(decode_xstring("_xD83D__xDE00_").unwrap(), "😀");
        assert_eq!(decode_xstring("x_xQQQQ__x12_").unwrap(), "x_xQQQQ__x12_");
        assert!(matches!(
            decode_xstring("plain中文").unwrap(),
            Cow::Borrowed(_)
        ));
        assert!(decode_xstring("_xD800_a").is_err());
        assert!(decode_xstring("_xDC00_").is_err());
    }

    #[test]
    fn coordinate_limits_and_complete_references_are_checked() {
        assert_eq!(column("A1").unwrap(), 0);
        assert_eq!(column("XFD1048576").unwrap(), MAX_COLUMNS - 1);
        assert!(column("XFE1").is_err());
        assert!(column("A0").is_err());
        assert!(column("A1048577").is_err());
        assert!(column("A1tail").is_err());
        assert!(column("A").is_err());
    }

    #[test]
    fn cached_string_values_preserve_whitespace_and_decode_escapes() {
        let mut cell = Cell {
            kind: "str".into(),
            value: "  x_x000D_  ".into(),
            present: true,
            ..Cell::default()
        };
        assert_eq!(cell.finish(&[], false).unwrap().unwrap(), "  x\r  ");
        cell.kind = "s".into();
        cell.value = "0".into();
        assert!(cell
            .finish(&[], false)
            .unwrap_err()
            .contains("缺失的共享字符串表"));
        assert_eq!(
            cell.finish(&[" shared ".into()], true).unwrap().unwrap(),
            " shared "
        );
    }

    #[test]
    fn rich_text_uses_only_main_text_and_decodes_before_joining_runs() {
        let values = read_rich_items(
            r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:x="urn:extension">
            <si><r><t> A </t></r><rPh><t>phonetic</t></rPh><x:r><x:t>foreign</x:t></x:r><r><x:t>foreign text</x:t></r></si>
            <si/><si><r><t>_x00</t></r><r><t>41_</t></r></si>
            <si><t>_x00&#x34;1_</t></si>
        </sst>"#,
        );
        assert_eq!(values, [" A ", "", "_x0041_", "A"]);
    }
}
