//! Namespace-aware, bounded XML input shared by the Office package and text readers.
//! Raw ZIP expansion and decoded UTF-8 tokens have separate limits: UTF-16 text
//! can grow during decoding, and neither representation is collected as a document.

use quick_xml::encoding::DecodingReader;
use quick_xml::events::{BytesDecl, BytesStart, Event};
use quick_xml::name::{NamespaceResolver, QName, ResolveResult};
use quick_xml::NsReader;
use std::borrow::Cow;
use std::collections::HashSet;
use std::io::{self, BufReader, Read};

use super::{MAX_BUFFER_BYTES, MAX_XML_BYTES};

const MAX_DEPTH: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Namespace {
    Word,
    WordDrawing,
    WordShape,
    WordGroup,
    Vml,
    Spreadsheet,
    Presentation,
    Drawing,
    Drawing2010,
    Math,
    MarkupCompatibility,
    OfficeRelationships,
    PackageRelationships,
    ContentTypes,
}

impl Namespace {
    fn matches(self, uri: &str) -> bool {
        match self {
            Self::Word => matches!(
                uri,
                "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
                    | "http://purl.oclc.org/ooxml/wordprocessingml/main"
            ),
            Self::WordDrawing => matches!(
                uri,
                "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
                    | "http://purl.oclc.org/ooxml/drawingml/wordprocessingDrawing"
            ),
            Self::WordShape => {
                uri == "http://schemas.microsoft.com/office/word/2010/wordprocessingShape"
            }
            Self::WordGroup => {
                uri == "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup"
            }
            Self::Vml => uri == "urn:schemas-microsoft-com:vml",
            Self::Spreadsheet => matches!(
                uri,
                "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
                    | "http://purl.oclc.org/ooxml/spreadsheetml/main"
            ),
            Self::Presentation => matches!(
                uri,
                "http://schemas.openxmlformats.org/presentationml/2006/main"
                    | "http://purl.oclc.org/ooxml/presentationml/main"
            ),
            Self::Drawing => matches!(
                uri,
                "http://schemas.openxmlformats.org/drawingml/2006/main"
                    | "http://purl.oclc.org/ooxml/drawingml/main"
            ),
            Self::Drawing2010 => uri == "http://schemas.microsoft.com/office/drawing/2010/main",
            Self::Math => matches!(
                uri,
                "http://schemas.openxmlformats.org/officeDocument/2006/math"
                    | "http://purl.oclc.org/ooxml/officeDocument/math"
            ),
            Self::MarkupCompatibility => {
                uri == "http://schemas.openxmlformats.org/markup-compatibility/2006"
            }
            Self::OfficeRelationships => matches!(
                uri,
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
                    | "http://purl.oclc.org/ooxml/officeDocument/relationships"
            ),
            Self::PackageRelationships => {
                uri == "http://schemas.openxmlformats.org/package/2006/relationships"
            }
            Self::ContentTypes => {
                uri == "http://schemas.openxmlformats.org/package/2006/content-types"
            }
        }
    }
}

pub(super) fn in_namespace(
    name: QName<'_>,
    namespaces: &NamespaceResolver,
    expected: Namespace,
) -> bool {
    matches!(namespaces.resolve_element(name).0,
        ResolveResult::Bound(namespace) if expected.matches(namespace.as_ref()))
}

/// Office's unqualified attributes are distinct from extension attributes that
/// happen to use the same local name. The XML reader validates all attributes.
pub(super) fn attribute(start: &BytesStart<'_>, name: &str) -> Result<Option<String>, String> {
    for attribute in start.attributes().with_checks(false) {
        let attribute = attribute.map_err(|e| e.to_string())?;
        if attribute.key.as_ref() == name {
            return attribute
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map(|value| Some(value.into_owned()))
                .map_err(|e| e.to_string());
        }
    }
    Ok(None)
}

pub(super) fn qualified_attribute(
    start: &BytesStart<'_>,
    namespaces: &NamespaceResolver,
    expected: Namespace,
    name: &str,
) -> Result<Option<String>, String> {
    for attribute in start.attributes().with_checks(false) {
        let attribute = attribute.map_err(|e| e.to_string())?;
        let (namespace, local) = namespaces.resolve_attribute(attribute.key);
        if local.as_ref() == name
            && matches!(namespace, ResolveResult::Bound(namespace) if expected.matches(namespace.as_ref()))
        {
            return attribute
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map(|value| Some(value.into_owned()))
                .map_err(|e| e.to_string());
        }
    }
    Ok(None)
}

fn xml_character(character: char) -> bool {
    matches!(character, '\u{9}' | '\u{A}' | '\u{D}' | '\u{20}'..='\u{D7FF}'
        | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

fn validate_characters(value: &str) -> Result<(), String> {
    if value.chars().all(xml_character) {
        Ok(())
    } else {
        Err("Office XML 包含 XML 1.0 不允许的字符".into())
    }
}

pub(super) fn text<'a>(event: &'a Event<'_>) -> Result<Option<Cow<'a, str>>, String> {
    Ok(match event {
        Event::Text(value) => Some(value.xml10_content()),
        Event::CData(value) => Some(value.xml10_content()),
        Event::GeneralRef(value) => {
            if let Some(character) = value.resolve_char_ref().map_err(|e| e.to_string())? {
                if !xml_character(character) {
                    return Err("Office XML 字符引用不是有效的 XML 1.0 字符".into());
                }
                Some(Cow::Owned(character.to_string()))
            } else {
                Some(Cow::Borrowed(match value.as_ref() {
                    "amp" => "&",
                    "lt" => "<",
                    "gt" => ">",
                    "apos" => "'",
                    "quot" => "\"",
                    _ => return Err("Office XML 包含未知实体".into()),
                }))
            }
        }
        _ => None,
    })
}

/// A single extra-byte probe permits exact-limit EOF without allocating beyond
/// the budget. Applied once before decoding, then again to decoded UTF-8.
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

fn validate_attributes(
    start: &BytesStart<'_>,
    namespaces: &NamespaceResolver,
) -> Result<(), String> {
    // Most Office elements have only a few attributes. Keep their expanded names
    // on the stack; longer tags promote once to a set instead of the parser's
    // unbounded quadratic duplicate scan.
    let mut small_names: [(Option<&str>, &str); 8] = [(None, ""); 8];
    let mut small_count = 0;
    let mut names: Option<HashSet<_>> = None;
    for attribute in start.attributes().with_checks(false) {
        let attribute = attribute.map_err(|e| e.to_string())?;
        let (namespace, name) = namespaces.resolve_attribute(attribute.key);
        let namespace = match namespace {
            ResolveResult::Bound(namespace) => Some(namespace.into_inner()),
            ResolveResult::Unbound => None,
            ResolveResult::Unknown(prefix) => {
                return Err(format!("Office XML 属性使用未声明的前缀 {prefix}"))
            }
        };
        let expanded_name = (namespace, name.into_inner());
        let unique = if let Some(names) = names.as_mut() {
            names.insert(expanded_name)
        } else if small_names[..small_count].contains(&expanded_name) {
            false
        } else if small_count < small_names.len() {
            small_names[small_count] = expanded_name;
            small_count += 1;
            true
        } else {
            let mut promoted = HashSet::with_capacity(small_names.len() * 2);
            promoted.extend(small_names);
            promoted.insert(expanded_name);
            names = Some(promoted);
            true
        };
        if !unique {
            return Err("Office XML 包含重复属性".into());
        }
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|e| e.to_string())?;
        validate_characters(&value)?;
    }
    Ok(())
}

fn validate_declaration(
    declaration: &BytesDecl<'_>,
    encoding: &'static encoding_rs::Encoding,
) -> Result<(), String> {
    let start = BytesStart::from_content(declaration.as_ref(), 3);
    let mut version = false;
    let mut declared_encoding = false;
    let mut standalone = false;
    for attribute in start.attributes().with_checks(false) {
        let attribute = attribute.map_err(|e| e.to_string())?;
        match attribute.key.as_ref() {
            "version" if !version && !declared_encoding && !standalone => {
                if attribute.value != "1.0" {
                    return Err("Office 仅支持 XML 1.0".into());
                }
                version = true;
            }
            "encoding" if version && !declared_encoding && !standalone => {
                let declared = attribute.value.as_ref();
                let matches = if declared.eq_ignore_ascii_case("UTF-8") {
                    encoding == encoding_rs::UTF_8
                } else if declared.eq_ignore_ascii_case("UTF-16") {
                    encoding == encoding_rs::UTF_16LE || encoding == encoding_rs::UTF_16BE
                } else if declared.eq_ignore_ascii_case("UTF-16LE") {
                    encoding == encoding_rs::UTF_16LE
                } else if declared.eq_ignore_ascii_case("UTF-16BE") {
                    encoding == encoding_rs::UTF_16BE
                } else {
                    return Err("Office XML 仅支持 UTF-8 或 UTF-16 编码".into());
                };
                if !matches {
                    return Err("Office XML 编码声明与实际字节编码不一致".into());
                }
                declared_encoding = true;
            }
            "standalone" if version && !standalone => {
                if !matches!(attribute.value.as_ref(), "yes" | "no") {
                    return Err("Office XML standalone 声明无效".into());
                }
                standalone = true;
            }
            _ => return Err("Office XML 声明无效".into()),
        }
    }
    if !version {
        return Err("Office XML 声明缺少版本".into());
    }
    Ok(())
}

/// Start and End callbacks receive the same element depth (the root is 1).
/// `false` deliberately ends extraction early, e.g. after the text output cap;
/// only a fully consumed part is checked for a complete closing root.
pub(super) fn read<R: Read>(
    source: R,
    remaining: &mut usize,
    part_limit: usize,
    expected_root: (Namespace, &str),
    mut consume: impl FnMut(Event<'_>, &NamespaceResolver, usize) -> Result<bool, String>,
) -> Result<(), String> {
    let limit = part_limit.min(MAX_XML_BYTES);
    let raw = BudgetReader {
        inner: source,
        remaining,
        part_remaining: limit,
        event_remaining: usize::MAX,
    };
    let decoded = DecodingReader::new(BufReader::with_capacity(8192, raw));
    let mut utf8_remaining = limit;
    let utf8 = BudgetReader {
        inner: decoded,
        remaining: &mut utf8_remaining,
        part_remaining: limit,
        event_remaining: MAX_BUFFER_BYTES,
    };
    let mut reader = NsReader::from_reader(BufReader::with_capacity(8192, utf8));
    reader.config_mut().expand_empty_elements = true;
    reader.config_mut().enable_all_checks(true);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut seen_root = false;
    let mut first_event = true;
    loop {
        buffer.clear();
        let buffered = reader.get_ref().buffer().len();
        reader.get_mut().get_mut().event_remaining = MAX_BUFFER_BYTES.saturating_sub(buffered);
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|e| e.to_string())?;
        let is_end = matches!(&event, Event::End(_));
        match &event {
            Event::Start(start) => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err("Office XML 嵌套层数超过提取预算".into());
                }
                if matches!(
                    reader.resolver().resolve_element(start.name()).0,
                    ResolveResult::Unknown(_)
                ) {
                    return Err("Office XML 元素使用未声明的命名空间前缀".into());
                }
                validate_attributes(start, reader.resolver())?;
                if depth == 1 {
                    if seen_root {
                        return Err("Office XML 包含多个根元素".into());
                    }
                    if start.local_name().as_ref() != expected_root.1
                        || !in_namespace(start.name(), reader.resolver(), expected_root.0)
                    {
                        return Err(format!("Office XML 根元素不是预期的 {}", expected_root.1));
                    }
                    seen_root = true;
                }
            }
            Event::End(_) if depth == 0 => return Err("Office XML 结束标签没有对应元素".into()),
            Event::DocType(_) => return Err("Office XML 不支持 DTD".into()),
            Event::Decl(declaration) => {
                if !first_event {
                    return Err("Office XML 声明必须位于文档开头".into());
                }
                validate_declaration(declaration, reader.get_ref().get_ref().inner.encoding())?;
            }
            Event::Text(value) => {
                validate_characters(value.as_ref())?;
                if depth == 0
                    && !value
                        .as_ref()
                        .bytes()
                        .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
                {
                    return Err("Office XML 根元素外包含文本".into());
                }
            }
            Event::CData(value) => {
                validate_characters(value.as_ref())?;
                if depth == 0 {
                    return Err("Office XML 根元素外包含 CDATA".into());
                }
            }
            Event::GeneralRef(_) => {
                text(&event)?;
                if depth == 0 {
                    return Err("Office XML 根元素外包含实体".into());
                }
            }
            Event::Comment(value) => validate_characters(value.as_ref())?,
            Event::PI(value) => validate_characters(value.as_ref())?,
            Event::Eof => {
                if !seen_root || depth != 0 {
                    return Err("Office XML 文档缺少完整根元素".into());
                }
                break;
            }
            _ => {}
        }
        first_event = false;
        if !consume(event, reader.resolver(), depth)? {
            break;
        }
        if is_end {
            depth -= 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";

    fn document(content: &str) -> String {
        format!("<sst xmlns=\"{NS}\">{content}</sst>")
    }

    fn collect(source: impl Read, limit: usize) -> Result<String, String> {
        let mut remaining = limit;
        let mut result = String::new();
        read(
            source,
            &mut remaining,
            limit,
            (Namespace::Spreadsheet, "sst"),
            |event, _, _| {
                if let Some(value) = text(&event)? {
                    result.push_str(&value);
                }
                Ok(true)
            },
        )?;
        Ok(result)
    }

    #[test]
    fn namespace_identity_and_start_end_depth_do_not_depend_on_prefix() {
        for ns in [NS, "http://purl.oclc.org/ooxml/spreadsheetml/main"] {
            let xml = format!("<x:sst xmlns:x='{ns}' xmlns:r='http://purl.oclc.org/ooxml/officeDocument/relationships' xmlns:e='urn:extension'><x:si e:id='wrong' id='plain' r:id='linked'/></x:sst>");
            let mut depths = Vec::new();
            let mut remaining = MAX_XML_BYTES;
            read(
                xml.as_bytes(),
                &mut remaining,
                MAX_XML_BYTES,
                (Namespace::Spreadsheet, "sst"),
                |event, namespaces, depth| {
                    match event {
                        Event::Start(start) => {
                            assert!(in_namespace(
                                start.name(),
                                namespaces,
                                Namespace::Spreadsheet
                            ));
                            if start.local_name().as_ref() == "si" {
                                assert_eq!(attribute(&start, "id")?.as_deref(), Some("plain"));
                                assert_eq!(
                                    qualified_attribute(
                                        &start,
                                        namespaces,
                                        Namespace::OfficeRelationships,
                                        "id"
                                    )?
                                    .as_deref(),
                                    Some("linked")
                                );
                            }
                            depths.push((true, depth));
                        }
                        Event::End(_) => depths.push((false, depth)),
                        _ => {}
                    }
                    Ok(true)
                },
            )
            .unwrap();
            assert_eq!(depths, [(true, 1), (true, 2), (false, 2), (false, 1)]);
        }
    }

    #[test]
    fn text_preserves_whitespace_and_resolves_only_xml_entities() {
        let xml = document("<si><t> a\r\nb &amp; &#x4e2d; &#128578; <![CDATA[<&>]]></t></si>");
        assert_eq!(
            collect(xml.as_bytes(), MAX_XML_BYTES).unwrap(),
            " a\nb & 中 🙂 <&>"
        );
    }

    #[test]
    fn rejects_incomplete_documents_entities_and_namespace_spoofing() {
        let cases = [
            "".to_string(),
            "<sst/>".to_string(),
            format!("<sst xmlns='{NS}'>"),
            format!("{}{}", document(""), document("")),
            format!("text{}", document("")),
            format!("{}<![CDATA[ ]]>", document("")),
            format!("<!DOCTYPE sst [<!ENTITY x 'text'>]>{}", document("&x;")),
            document("&undefined;"),
            document("&#0;"),
            document("\u{1}"),
            document("<missing:si/>"),
            document("<si missing:value='x'/>"),
            document("<si value='x' value='y'/>"),
            document("<si xmlns:p='urn:x' xmlns:q='urn:x' p:value='x' q:value='y'/>"),
            document("<si value='&undefined;'/>"),
            format!("<?xml version='1.1'?>{}", document("")),
            format!(" <?xml version='1.0'?>{}", document("")),
            format!("<?xml version='1.0' encoding='UTF-16'?>{}", document("")),
            format!(
                "<?xml version='1.0' encoding='ISO-8859-1'?>{}",
                document("")
            ),
        ];
        for xml in cases {
            assert!(
                collect(xml.as_bytes(), MAX_XML_BYTES).is_err(),
                "accepted {xml}"
            );
        }
    }

    #[test]
    fn duplicate_attributes_remain_rejected_after_small_storage_promotes() {
        let attributes: String = (0..12).map(|index| format!("a{index}='value' ")).collect();
        assert_eq!(
            collect(
                document(&format!("<si {attributes}/>")).as_bytes(),
                MAX_XML_BYTES
            )
            .unwrap(),
            ""
        );
        for duplicate in ["a0", "a7", "a8", "a11"] {
            let xml = document(&format!("<si {attributes} {duplicate}='duplicate'/>"));
            let error = collect(xml.as_bytes(), MAX_XML_BYTES).unwrap_err();
            assert!(error.contains("重复属性"), "{error}");
        }
        // A different prefix still denotes the same expanded name across the
        // stack-to-set boundary (six plain attributes plus two declarations).
        let attributes: String = (0..6).map(|index| format!("a{index}='value' ")).collect();
        let xml = document(&format!(
            "<si {attributes} xmlns:p='urn:extension' xmlns:q='urn:extension' p:value='first' q:value='duplicate'/>"
        ));
        let error = collect(xml.as_bytes(), MAX_XML_BYTES).unwrap_err();
        assert!(error.contains("重复属性"), "{error}");
    }

    fn utf16(xml: &str, little_endian: bool) -> Vec<u8> {
        let mut bytes = if little_endian {
            vec![0xff, 0xfe]
        } else {
            vec![0xfe, 0xff]
        };
        for unit in xml.encode_utf16() {
            bytes.extend_from_slice(&if little_endian {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        bytes
    }

    struct ByteAtATime<'a>(&'a [u8]);
    impl Read for ByteAtATime<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if buffer.is_empty() || self.0.is_empty() {
                return Ok(0);
            }
            buffer[0] = self.0[0];
            self.0 = &self.0[1..];
            Ok(1)
        }
    }

    #[test]
    fn utf16_both_endiannesses_stream_across_surrogate_boundaries() {
        let xml = format!(
            "<?xml version='1.0' encoding='UTF-16'?>{}",
            document("<si><t>中🙂 &amp; 文</t></si>")
        );
        for little_endian in [false, true] {
            let bytes = utf16(&xml, little_endian);
            assert_eq!(
                collect(ByteAtATime(&bytes), MAX_XML_BYTES).unwrap(),
                "中🙂 & 文"
            );
            assert!(collect(ByteAtATime(&bytes[..bytes.len() - 1]), MAX_XML_BYTES).is_err());
        }
        let mut malformed = utf16(&format!("<sst xmlns='{NS}'><si><t>"), true);
        // An unpaired high surrogate is an encoding error, never replacement text.
        malformed.extend_from_slice(&0xd800u16.to_le_bytes());
        for unit in "</t></si></sst>".encode_utf16() {
            malformed.extend_from_slice(&unit.to_le_bytes());
        }
        let error = collect(ByteAtATime(&malformed), MAX_XML_BYTES).unwrap_err();
        assert!(error.contains("decode"), "{error}");
    }

    #[test]
    fn utf16_raw_expansion_and_decoded_utf8_have_independent_caps() {
        let ascii = document("<si><t>ascii</t></si>");
        let bytes = utf16(&ascii, true);
        assert!(bytes.len() > ascii.len());
        assert!(collect(bytes.as_slice(), ascii.len()).is_err());
        let chinese = document(&format!("<si><t>{}</t></si>", "中".repeat(1000)));
        let bytes = utf16(&chinese, true);
        assert!(chinese.len() > bytes.len());
        assert!(collect(bytes.as_slice(), bytes.len()).is_err());
        assert_eq!(
            collect(bytes.as_slice(), chinese.len()).unwrap(),
            "中".repeat(1000)
        );
    }

    #[test]
    fn exact_byte_budget_allows_eof_but_not_one_more_byte() {
        let xml = document("");
        assert_eq!(collect(xml.as_bytes(), xml.len()).unwrap(), "");
        assert!(collect(format!("{xml} ").as_bytes(), xml.len()).is_err());
        let mut remaining = 32;
        let mut reader = BudgetReader {
            inner: io::Cursor::new(vec![b'x'; 1024]),
            remaining: &mut remaining,
            part_remaining: 32,
            event_remaining: 32,
        };
        let mut result = Vec::new();
        assert!(reader.read_to_end(&mut result).is_err());
        assert_eq!(result.len(), 32);
    }

    #[test]
    fn depth_limit_and_intentional_early_stop_are_bounded() {
        let xml = document(&format!(
            "{}{}",
            "<si>".repeat(MAX_DEPTH),
            "</si>".repeat(MAX_DEPTH)
        ));
        assert!(collect(xml.as_bytes(), MAX_XML_BYTES).is_err());
        let mut remaining = MAX_XML_BYTES;
        read(
            document("<si/>").as_bytes(),
            &mut remaining,
            MAX_XML_BYTES,
            (Namespace::Spreadsheet, "sst"),
            |_, _, _| Ok(false),
        )
        .unwrap();
    }

    #[test]
    fn oversized_xml_token_never_reaches_the_consumer() {
        let xml = document(&"x".repeat(MAX_BUFFER_BYTES + 1));
        let mut remaining = MAX_XML_BYTES;
        let mut text_events = 0;
        let result = read(
            xml.as_bytes(),
            &mut remaining,
            MAX_XML_BYTES,
            (Namespace::Spreadsheet, "sst"),
            |event, _, _| {
                if matches!(event, Event::Text(_)) {
                    text_events += 1;
                }
                Ok(true)
            },
        );
        assert!(result.is_err());
        assert_eq!(text_events, 0);
        assert_eq!(
            collect(document("<si><t>next</t></si>").as_bytes(), MAX_XML_BYTES).unwrap(),
            "next"
        );
    }
}
