//! OPC part discovery and relationships. Every target remains a ZIP item name;
//! no relationship is opened as a filesystem path or network address.

use super::xml::{self, Namespace};
use super::{MAX_BUFFER_BYTES, MAX_EXPANDED_BYTES, MAX_PARTS, MAX_XML_BYTES};
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::NamespaceResolver;
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use zip::ZipArchive;

const RELATIONSHIP_NAMESPACES: [&str; 2] = [
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/",
    "http://purl.oclc.org/ooxml/officeDocument/relationships/",
];
const RELATIONSHIPS_TYPE: &str = "application/vnd.openxmlformats-package.relationships+xml";
// A package may contain many resource parts per sheet/slide. Its directory has
// its own limit rather than sharing the per-manifest/per-relationship limit.
const MAX_PACKAGE_PARTS: usize = 65_536;
// Conservative per-entry allowance supplements retained string bytes. This is
// an application index budget, not a bound on ZipArchive's central-directory allocations.
const METADATA_ENTRY_BYTES: usize = 96;

pub(super) struct Package {
    archive: ZipArchive<File>,
    parts: HashMap<String, usize>,
    content_types_index: usize,
    content_types: ContentTypes,
    remaining: usize,
    metadata_bytes: usize,
}

#[derive(Default)]
struct ContentTypes {
    defaults: HashMap<String, String>,
    overrides: HashMap<String, String>,
}

pub(super) struct Relationships {
    entries: HashMap<String, Relationship>,
}

pub(super) struct Relationship {
    kind: String,
    target: String,
    external: bool,
}

impl Package {
    pub(super) fn open(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|error| error.to_string())?;
        let mut archive = ZipArchive::new(file).map_err(|error| error.to_string())?;
        let mut parts = HashMap::new();
        let mut metadata_bytes = 0;
        let mut content_types_index = None;
        for index in 0..archive.len() {
            let entry = archive
                .by_index_raw(index)
                .map_err(|error| error.to_string())?;
            if entry.is_dir() {
                continue;
            }
            let zip_name = entry.name().map_err(|error| error.to_string())?;
            if zip_name.eq_ignore_ascii_case("[Content_Types].xml") {
                if content_types_index.replace(index).is_some() {
                    return Err("Office 包包含重复的内容类型清单".into());
                }
                continue;
            }
            // OPC permits unrelated ZIP trash items. Invalid item names cannot
            // become targets, and do not prevent extraction of valid parts.
            let Ok(name) = part_name(&zip_name) else {
                continue;
            };
            // ZIP physical names use the escaped URI spelling. Unicode/space
            // conversion is for relationship IRIs, never an alternate ZIP lookup.
            if !name.eq_ignore_ascii_case(&zip_name) {
                continue;
            }
            if parts.len() >= MAX_PACKAGE_PARTS {
                return Err("Office 包部件数量超过提取预算".into());
            }
            reserve(&mut metadata_bytes, name.len() + METADATA_ENTRY_BYTES)?;
            if parts.insert(name.to_ascii_lowercase(), index).is_some() {
                return Err(format!("Office 包含等价或重复的部件名称：{name}"));
            }
        }
        let content_types_index = content_types_index.ok_or("Office 缺少 [Content_Types].xml")?;
        let mut package = Self {
            archive,
            parts,
            content_types_index,
            content_types: ContentTypes::default(),
            remaining: MAX_EXPANDED_BYTES,
            metadata_bytes,
        };
        package.load_content_types()?;
        Ok(package)
    }

    pub(super) fn main_part(&mut self, kind: &str) -> Result<String, String> {
        let expected_type = match kind {
            "docx" => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"
            }
            "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
            "pptx" => {
                "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"
            }
            _ => return Err("不支持的 Office 格式".into()),
        };
        let relationships = self.relationships("")?;
        let main = relationships
            .optional_unique("officeDocument")?
            .ok_or("Office 缺少主文档关系 officeDocument")?
            .target("")?;
        self.require_type(&main, expected_type)?;
        self.part_index(&main)?;
        Ok(main)
    }

    pub(super) fn reserve_metadata(&mut self, bytes: usize) -> Result<(), String> {
        reserve(&mut self.metadata_bytes, bytes)
    }

    pub(super) fn read_part(
        &mut self,
        name: &str,
        expected_root: (Namespace, &str),
        consume: impl FnMut(Event<'_>, &NamespaceResolver, usize) -> Result<bool, String>,
    ) -> Result<(), String> {
        if let Some(expected) = content_type_for_root(expected_root) {
            self.require_type(name, expected)?;
        }
        self.read_xml(name, MAX_XML_BYTES, expected_root, consume)
    }

    pub(super) fn relationships(&mut self, source: &str) -> Result<Relationships, String> {
        let name = relationship_part(source)?;
        self.require_type(&name, RELATIONSHIPS_TYPE)?;
        let mut entries = HashMap::new();
        let mut metadata_bytes = self.metadata_bytes;
        self.read_xml(
            &name,
            MAX_BUFFER_BYTES,
            (Namespace::PackageRelationships, "Relationships"),
            |event, namespaces, depth| {
                let Event::Start(start) = event else {
                    return Ok(true);
                };
                if depth != 2
                    || !xml::in_namespace(start.name(), namespaces, Namespace::PackageRelationships)
                    || start.local_name().as_ref() != "Relationship"
                {
                    return Ok(true);
                }
                if entries.len() >= MAX_PARTS {
                    return Err("Office 关系数量超过提取预算".into());
                }
                let id = required_attribute(&start, "Id")?;
                let kind = required_attribute(&start, "Type")?;
                let target =
                    xml::attribute(&start, "Target")?.ok_or("Office 包元数据缺少 Target")?;
                let external = match xml::attribute(&start, "TargetMode")?.as_deref() {
                    None | Some("Internal") => false,
                    Some("External") => true,
                    _ => return Err("Office 关系 TargetMode 无效".into()),
                };
                reserve(
                    &mut metadata_bytes,
                    id.len() + kind.len() + target.len() + METADATA_ENTRY_BYTES,
                )?;
                if entries
                    .insert(
                        id,
                        Relationship {
                            kind,
                            target,
                            external,
                        },
                    )
                    .is_some()
                {
                    return Err("Office 关系 ID 重复".into());
                }
                Ok(true)
            },
        )?;
        self.metadata_bytes = metadata_bytes;
        Ok(Relationships { entries })
    }

    fn part_index(&self, name: &str) -> Result<usize, String> {
        if name == "[Content_Types].xml" {
            return Ok(self.content_types_index);
        }
        let key = part_name(name)?.to_ascii_lowercase();
        self.parts
            .get(&key)
            .copied()
            .ok_or_else(|| format!("Office 缺少部件 {name}"))
    }

    fn read_xml(
        &mut self,
        name: &str,
        limit: usize,
        expected_root: (Namespace, &str),
        consume: impl FnMut(Event<'_>, &NamespaceResolver, usize) -> Result<bool, String>,
    ) -> Result<(), String> {
        let index = self.part_index(name)?;
        let file = self
            .archive
            .by_index(index)
            .map_err(|error| error.to_string())?;
        if file.size() > limit as u64 || file.size() > self.remaining as u64 {
            return Err(format!("Office XML 部件 {name} 超过解压预算"));
        }
        xml::read(file, &mut self.remaining, limit, expected_root, consume)
            .map_err(|error| format!("{name}: {error}"))
    }

    fn require_type(&self, name: &str, expected: &str) -> Result<(), String> {
        let key = part_name(name)?.to_ascii_lowercase();
        let content_type = self
            .content_types
            .overrides
            .get(&key)
            .or_else(|| {
                let leaf = key.rsplit('/').next()?;
                let (_, extension) = leaf.rsplit_once('.')?;
                self.content_types.defaults.get(extension)
            })
            .ok_or_else(|| format!("Office 部件缺少内容类型：{name}"))?;
        if !content_type.eq_ignore_ascii_case(expected) {
            return Err(format!("Office 部件内容类型不匹配：{name}"));
        }
        Ok(())
    }

    fn load_content_types(&mut self) -> Result<(), String> {
        let mut types = ContentTypes::default();
        let mut metadata_bytes = self.metadata_bytes;
        let mut count = 0;
        self.read_xml(
            "[Content_Types].xml",
            MAX_BUFFER_BYTES,
            (Namespace::ContentTypes, "Types"),
            |event, namespaces, depth| {
                let Event::Start(start) = event else {
                    return Ok(true);
                };
                if depth != 2
                    || !xml::in_namespace(start.name(), namespaces, Namespace::ContentTypes)
                {
                    return Ok(true);
                }
                let local = start.local_name();
                if !matches!(local.as_ref(), "Default" | "Override") {
                    return Ok(true);
                }
                if count >= MAX_PACKAGE_PARTS {
                    return Err("Office 内容类型数量超过提取预算".into());
                }
                count += 1;
                let content_type = required_attribute(&start, "ContentType")?;
                let (key, destination) = if local.as_ref() == "Override" {
                    let name = required_attribute(&start, "PartName")?;
                    let name = name
                        .strip_prefix('/')
                        .ok_or("Office 内容类型部件名必须从包根开始")?;
                    (part_name(name)?.to_ascii_lowercase(), &mut types.overrides)
                } else {
                    let extension = required_attribute(&start, "Extension")?;
                    if extension.contains(['/', '\\', '.', ':', '?', '#', '%'])
                        || extension.chars().any(char::is_whitespace)
                    {
                        return Err("Office 默认内容类型扩展名无效".into());
                    }
                    (extension.to_ascii_lowercase(), &mut types.defaults)
                };
                reserve(
                    &mut metadata_bytes,
                    key.len() + content_type.len() + METADATA_ENTRY_BYTES,
                )?;
                if destination.insert(key, content_type).is_some() {
                    return Err("Office 内容类型声明重复".into());
                }
                Ok(true)
            },
        )?;
        self.metadata_bytes = metadata_bytes;
        self.content_types = types;
        Ok(())
    }
}

impl Relationships {
    pub(super) fn get(&self, id: &str) -> Result<&Relationship, String> {
        self.entries
            .get(id)
            .ok_or_else(|| format!("Office 部件关系缺失：{id}"))
    }

    pub(super) fn by_id(&self, id: &str, kind: &str) -> Result<&Relationship, String> {
        let relationship = self.get(id)?;
        if !relationship.is_kind(kind) {
            return Err(format!("Office 关系 {id} 的类型不是 {kind}"));
        }
        Ok(relationship)
    }

    pub(super) fn optional_unique(&self, kind: &str) -> Result<Option<&Relationship>, String> {
        let mut matches = self
            .entries
            .values()
            .filter(|relationship| relationship.is_kind(kind));
        let first = matches.next();
        if matches.next().is_some() {
            return Err(format!("Office 包含重复的 {kind} 关系"));
        }
        Ok(first)
    }
}

impl Relationship {
    pub(super) fn is_kind(&self, kind: &str) -> bool {
        RELATIONSHIP_NAMESPACES
            .iter()
            .any(|namespace| self.kind.strip_prefix(namespace) == Some(kind))
    }

    pub(super) fn target(&self, source: &str) -> Result<String, String> {
        if self.external {
            return Err("Office 正文部件引用了外部目标".into());
        }
        resolve_target(source, &self.target)
    }
}

fn content_type_for_root(root: (Namespace, &str)) -> Option<&'static str> {
    match root {
        (Namespace::Word, "document") => {
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml")
        }
        (Namespace::Spreadsheet, "workbook") => {
            Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml")
        }
        (Namespace::Spreadsheet, "worksheet") => {
            Some("application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml")
        }
        (Namespace::Spreadsheet, "sst") => {
            Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml")
        }
        (Namespace::Presentation, "presentation") => Some(
            "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml",
        ),
        (Namespace::Presentation, "sld") => {
            Some("application/vnd.openxmlformats-officedocument.presentationml.slide+xml")
        }
        _ => None,
    }
}

fn reserve(bytes: &mut usize, added: usize) -> Result<(), String> {
    if added > MAX_BUFFER_BYTES.saturating_sub(*bytes) {
        return Err("Office 包目录、部件清单或关系表超过提取预算".into());
    }
    *bytes += added;
    Ok(())
}

fn required_attribute(start: &BytesStart<'_>, name: &str) -> Result<String, String> {
    xml::attribute(start, name)?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Office 包元数据缺少 {name}"))
}

fn relationship_part(source: &str) -> Result<String, String> {
    if source.is_empty() {
        return Ok("_rels/.rels".into());
    }
    let source = part_name(source)?;
    Ok(match source.rsplit_once('/') {
        Some((directory, name)) => format!("{directory}/_rels/{name}.rels"),
        None => format!("_rels/{source}.rels"),
    })
}

/// OPC compares part names ASCII case-insensitively, but preserves escaped
/// octets in ZIP names. Only URI-unreserved octets are decoded; %2F and %5C can
/// never turn into path separators. This is URI resolution, not OS path cleanup.
fn escaped_path(value: &str) -> Result<String, String> {
    if value.contains(['\\', '?', '#']) || value.chars().any(char::is_control) {
        return Err("Office 关系目标不是有效的包内 URI".into());
    }
    let mut result = String::with_capacity(value.len());
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = bytes
                .next()
                .and_then(hex_digit)
                .ok_or("Office 部件 URI 包含无效百分号编码")?;
            let low = bytes
                .next()
                .and_then(hex_digit)
                .ok_or("Office 部件 URI 包含无效百分号编码")?;
            let decoded = high * 16 + low;
            if matches!(decoded, b'/' | b'\\') {
                return Err("Office 部件 URI 包含编码的路径分隔符".into());
            }
            if unreserved(decoded) {
                result.push(char::from(decoded));
            } else {
                push_escape(&mut result, decoded);
            }
        } else if unreserved(byte) || b"/!$&'()*+,;=:@".contains(&byte) {
            result.push(char::from(byte));
        } else {
            // The URI spelling of a Unicode IRI is UTF-8 percent-encoded.
            push_escape(&mut result, byte);
        }
        if result.len() > MAX_BUFFER_BYTES {
            return Err("Office 部件 URI 超过提取预算".into());
        }
    }
    Ok(result)
}

fn unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn push_escape(result: &mut String, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    result.push('%');
    result.push(char::from(HEX[usize::from(byte >> 4)]));
    result.push(char::from(HEX[usize::from(byte & 15)]));
}

fn part_name(name: &str) -> Result<String, String> {
    let name = escaped_path(name)?;
    if name.is_empty()
        || name
            .split('/')
            .any(|part| part.is_empty() || part.ends_with('.'))
    {
        return Err("Office 部件名称无效".into());
    }
    Ok(name)
}

fn resolve_target(source: &str, target: &str) -> Result<String, String> {
    if target.is_empty() || target.starts_with("//") {
        return Err("Office 关系目标不是有效的包内路径".into());
    }
    let target = escaped_path(target)?;
    if !target.starts_with('/')
        && target
            .split('/')
            .next()
            .is_some_and(|first| first.contains(':'))
    {
        return Err("Office 关系目标不能包含 URI scheme".into());
    }
    let source = if source.is_empty() {
        String::new()
    } else {
        part_name(source)?
    };
    let mut components: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        source
            .rsplit_once('/')
            .map_or_else(Vec::new, |(directory, _)| directory.split('/').collect())
    };
    let target = target.strip_prefix('/').unwrap_or(&target);
    if target.ends_with('/') || matches!(target.rsplit('/').next(), Some("." | "..")) {
        return Err("Office 关系目标缺少部件名称".into());
    }
    for component in target.split('/') {
        match component {
            "" => return Err("Office 关系目标包含空路径段".into()),
            "." => {}
            ".." => {
                components.pop().ok_or("Office 关系目标超出包内根目录")?;
            }
            _ => components.push(component),
        }
    }
    part_name(&components.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn archive(parts: &[(&str, &str)]) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut zip = zip::ZipWriter::new(file.reopen().unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (name, content) in parts {
            zip.start_file(*name, options).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        file
    }

    const TYPES: &str = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/xl/book.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/></Types>"#;
    const ROOT_RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="main" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/book.xml"/></Relationships>"#;
    const BOOK: &str = r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheets/></workbook>"#;

    #[test]
    fn package_part_lookup_uses_opc_equivalence_and_content_type_overrides() {
        let file = archive(&[
            ("[Content_Types].xml", TYPES),
            ("_rels/.rels", ROOT_RELS),
            ("XL/BOOK.XML", BOOK),
        ]);
        let mut package = Package::open(file.path()).unwrap();
        let main = package.main_part("xlsx").unwrap();
        let mut elements = 0;
        package
            .read_part(
                &main,
                (Namespace::Spreadsheet, "workbook"),
                |event, _, _| {
                    if matches!(event, Event::Start(_)) {
                        elements += 1;
                    }
                    Ok(true)
                },
            )
            .unwrap();
        assert_eq!(elements, 2);
        assert!(package
            .main_part("docx")
            .unwrap_err()
            .contains("内容类型不匹配"));
    }

    #[test]
    fn package_index_rejects_equivalent_duplicates() {
        let file = archive(&[
            ("[Content_Types].xml", TYPES),
            ("xl/book.xml", BOOK),
            ("XL/BOOK.XML", BOOK),
        ]);
        assert!(matches!(Package::open(file.path()), Err(error) if error.contains("重复")));
    }

    #[test]
    fn content_types_default_is_used_only_without_an_override() {
        let defaults = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="RELS" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="XML" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/></Types>"#;
        let file = archive(&[
            ("[Content_Types].xml", defaults),
            ("_rels/.rels", ROOT_RELS),
            ("xl/book.xml", BOOK),
        ]);
        assert_eq!(
            Package::open(file.path())
                .unwrap()
                .main_part("xlsx")
                .unwrap(),
            "xl/book.xml"
        );
        let conflicting = defaults.replace(
            "</Types>",
            "<Override PartName=\"/xl/book.xml\" ContentType=\"application/xml\"/></Types>",
        );
        let file = archive(&[
            ("[Content_Types].xml", &conflicting),
            ("_rels/.rels", ROOT_RELS),
            ("xl/book.xml", BOOK),
        ]);
        assert!(Package::open(file.path())
            .unwrap()
            .main_part("xlsx")
            .unwrap_err()
            .contains("内容类型不匹配"));
    }

    #[test]
    fn unconsumed_external_relationships_do_not_become_io() {
        let relationships = ROOT_RELS.replace("</Relationships>", "<Relationship Id=\"link\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink\" Target=\"https://example.com/\" TargetMode=\"External\"/></Relationships>");
        let file = archive(&[
            ("[Content_Types].xml", TYPES),
            ("_rels/.rels", &relationships),
            ("xl/book.xml", BOOK),
        ]);
        let mut package = Package::open(file.path()).unwrap();
        assert_eq!(package.main_part("xlsx").unwrap(), "xl/book.xml");
        assert!(package
            .relationships("")
            .unwrap()
            .get("link")
            .unwrap()
            .target("")
            .unwrap_err()
            .contains("外部目标"));
    }

    #[test]
    fn escaped_part_names_keep_the_zip_mapping() {
        let types = TYPES.replace("/xl/book.xml", "/xl/%E4%B8%AD%20book.xml");
        let rels = ROOT_RELS.replace("xl/book.xml", "xl/中 book.xml");
        let file = archive(&[
            ("[Content_Types].xml", &types),
            ("_rels/.rels", &rels),
            ("xl/%E4%B8%AD%20book.xml", BOOK),
        ]);
        assert_eq!(
            Package::open(file.path())
                .unwrap()
                .main_part("xlsx")
                .unwrap(),
            "xl/%E4%B8%AD%20book.xml"
        );
        // A non-OPC ZIP name is not an implicit alternative for that URI.
        let file = archive(&[
            ("[Content_Types].xml", &types),
            ("_rels/.rels", &rels),
            ("xl/中 book.xml", BOOK),
        ]);
        assert!(Package::open(file.path())
            .unwrap()
            .main_part("xlsx")
            .is_err());
    }

    #[test]
    fn target_resolution_is_relative_to_the_owning_part() {
        assert_eq!(
            resolve_target("xl/books/main.xml", "../strings/table.xml").unwrap(),
            "xl/strings/table.xml"
        );
        assert_eq!(
            resolve_target("book.xml", "sheets/a.xml").unwrap(),
            "sheets/a.xml"
        );
        assert_eq!(
            resolve_target("xl/book.xml", "/data/a.xml").unwrap(),
            "data/a.xml"
        );
        assert_eq!(
            resolve_target("", "office/main.xml").unwrap(),
            "office/main.xml"
        );
        assert_eq!(relationship_part("").unwrap(), "_rels/.rels");
        assert_eq!(
            relationship_part("main.xml").unwrap(),
            "_rels/main.xml.rels"
        );
        assert_eq!(
            relationship_part("xl/main.xml").unwrap(),
            "xl/_rels/main.xml.rels"
        );
    }

    #[test]
    fn uri_escaping_preserves_zip_names_and_never_creates_separators() {
        assert_eq!(
            resolve_target("xl/book.xml", "../strings/中 文.xml").unwrap(),
            "strings/%E4%B8%AD%20%E6%96%87.xml"
        );
        assert_eq!(
            resolve_target("xl/book.xml", "../strings/%e4%b8%ad%20name.xml").unwrap(),
            "strings/%E4%B8%AD%20name.xml"
        );
        assert_eq!(
            resolve_target("xl/book.xml", "sheets/%61.xml").unwrap(),
            "xl/sheets/a.xml"
        );
        for target in [
            "../../a.xml",
            "%2e%2e/%2e%2e/a.xml",
            "//server/a.xml",
            "https://example.com/a.xml",
            "a%2fb.xml",
            "a%5Cb.xml",
            "a%Q0.xml",
            "a.xml#fragment",
            "a.xml?q=1",
            "dir/",
            "dir/..",
            "a\\b.xml",
        ] {
            assert!(resolve_target("xl/book.xml", target).is_err(), "{target}");
        }
    }

    #[test]
    fn relationship_lookup_distinguishes_optional_unique_and_explicit_ids() {
        let relationship = |kind: &str| Relationship {
            kind: format!("{}{kind}", RELATIONSHIP_NAMESPACES[0]),
            target: "part.xml".into(),
            external: false,
        };
        let mut entries = HashMap::new();
        entries.insert("s1".into(), relationship("worksheet"));
        let mut relationships = Relationships { entries };
        assert!(relationships
            .optional_unique("sharedStrings")
            .unwrap()
            .is_none());
        assert!(relationships.by_id("s1", "worksheet").is_ok());
        assert!(relationships.by_id("s1", "slide").is_err());
        relationships
            .entries
            .insert("strings".into(), relationship("sharedStrings"));
        assert!(relationships
            .optional_unique("sharedStrings")
            .unwrap()
            .is_some());
        relationships
            .entries
            .insert("other".into(), relationship("sharedStrings"));
        assert!(relationships.optional_unique("sharedStrings").is_err());
    }

    #[test]
    fn metadata_budget_includes_record_overhead() {
        let mut bytes = MAX_BUFFER_BYTES - METADATA_ENTRY_BYTES;
        reserve(&mut bytes, METADATA_ENTRY_BYTES).unwrap();
        assert!(reserve(&mut bytes, 1).is_err());
    }
}
