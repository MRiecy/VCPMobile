//! file_extractor 集成测试。
//!
//! 从 `src-tauri/src/vcp_modules/infra/file_extractor.rs` 内联测试迁出。
//! 原测试 `test_extract_sample_files` 使用绝对路径 `g:\VCPMobile\scripts\test`
//! （该目录不存在 → CI 必跳过）且无任何断言，仅做人工诊断。
//!
//! 本集成测试使用仓库内固定的真实附件样本：
//! - `sample.docx`：蒙特卡罗模拟实验报告
//! - `sample.xlsx`：蒙特卡洛计算π值表格
//! - `sample.pdf`：扫描件/图片型 PDF 回退提示
//! - `sample.pptx`：蒙特卡罗模拟原理与方法演示文稿
//!
//! 另覆盖文本文件 BOM/编码解码路径与超大文件 OOM 防护路径。

use std::io::{Read, Write};
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

fn write_office(parts: &[(&str, &str)], extension: &str) -> NamedTempFile {
    let main = match extension {
        "docx" => "word/document.xml",
        "pptx" => "ppt/presentation.xml",
        "xlsx" => "xl/workbook.xml",
        _ => panic!("unsupported test package"),
    };
    write_office_package(parts, extension, main)
}

fn xml_root(xml: &str) -> String {
    let mut reader = quick_xml::Reader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(start) | quick_xml::events::Event::Empty(start)) => {
                return start.local_name().as_ref().to_string();
            }
            Ok(quick_xml::events::Event::Eof) | Err(_) => return String::new(),
            _ => {}
        }
    }
}

// Build a minimal OPC package, including the package relationship and content
// types. Format tests supply actual namespaces and relationships explicitly.
fn write_office_package(parts: &[(&str, &str)], extension: &str, main: &str) -> NamedTempFile {
    let root_relationships = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="main" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="{main}"/></Relationships>"#
    );
    let main_type = match extension {
        "docx" => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"
        }
        "pptx" => {
            "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"
        }
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
        _ => panic!("unsupported test package"),
    };
    let mut types = String::from(
        r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>"#,
    );
    for (name, xml) in parts
        .iter()
        .filter(|(name, _)| *name != "[Content_Types].xml" && !name.ends_with(".rels"))
    {
        let kind = if *name == main {
            main_type
        } else {
            match xml_root(xml).as_str() {
                "worksheet" => {
                    "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"
                }
                "chartsheet" => {
                    "application/vnd.openxmlformats-officedocument.spreadsheetml.chartsheet+xml"
                }
                "sst" => {
                    "application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"
                }
                "sld" => "application/vnd.openxmlformats-officedocument.presentationml.slide+xml",
                _ => "application/xml",
            }
        };
        types.push_str(&format!(
            r#"<Override PartName="/{name}" ContentType="{kind}"/>"#
        ));
    }
    types.push_str("</Types>");
    let file = tempfile::Builder::new()
        .suffix(&format!(".{extension}"))
        .tempfile_in("tests")
        .expect("create Office fixture");
    let mut archive = zip::ZipWriter::new(file.reopen().expect("open Office fixture"));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    if !parts.iter().any(|(name, _)| *name == "_rels/.rels") {
        archive
            .start_file("_rels/.rels", options)
            .expect("start package relationships");
        archive
            .write_all(root_relationships.as_bytes())
            .expect("write package relationships");
    }
    if !parts.iter().any(|(name, _)| *name == "[Content_Types].xml") {
        archive
            .start_file("[Content_Types].xml", options)
            .expect("start content types");
        archive
            .write_all(types.as_bytes())
            .expect("write content types");
    }
    for (name, content) in parts {
        archive.start_file(*name, options).expect("start XML part");
        archive
            .write_all(content.as_bytes())
            .expect("write XML part");
    }
    archive.finish().expect("finish Office fixture");
    file
}

const SINGLE_SHEET_WORKBOOK: (&str, &str) = (
    "xl/workbook.xml",
    r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet 1" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
);
const SINGLE_SHEET_RELATIONSHIPS: (&str, &str) = (
    "xl/_rels/workbook.xml.rels",
    r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
);
const SINGLE_SHEET_WITH_STRINGS_RELATIONSHIPS: (&str, &str) = (
    "xl/_rels/workbook.xml.rels",
    r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="strings" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/></Relationships>"#,
);

const SHARED_CELL: (&str, &str) = (
    "xl/worksheets/sheet1.xml",
    r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row><c r="A1" t="s"><v>0</v></c></row></sheetData></worksheet>"#,
);

fn extracted(file: &NamedTempFile) -> String {
    try_extract_text(file.path(), "application/octet-stream").expect("Office extraction result")
}

fn assert_incomplete(file: &NamedTempFile) {
    let text = extracted(file);
    assert!(text.contains("Office 文本提取未完成"), "{text}");
    assert!(text.contains("原附件已保留"), "{text}");
}

// Relocate/replace parts of a real package while retaining every unrelated
// entry. This checks content equivalence independently of synthetic fixtures.
fn transform_package(
    bytes: &[u8],
    extension: &str,
    mut transform: impl FnMut(&str, Vec<u8>) -> (String, Vec<u8>),
) -> NamedTempFile {
    let file = tempfile::Builder::new()
        .suffix(&format!(".{extension}"))
        .tempfile_in("tests")
        .unwrap();
    let mut source = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut target = zip::ZipWriter::new(file.reopen().unwrap());
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for index in 0..source.len() {
        let mut part = source.by_index(index).unwrap();
        let mut content = Vec::new();
        part.read_to_end(&mut content).unwrap();
        let name = part.name().expect("valid ZIP part name");
        let (name, content) = transform(&name, content);
        target.start_file(name, options).unwrap();
        target.write_all(&content).unwrap();
    }
    target.finish().unwrap();
    file
}

#[test]
fn xlsx_relocated_real_shared_strings_preserve_all_extracted_text() {
    let original = include_bytes!("fixtures/file_extractor/sample.xlsx");
    let before = extracted(&write_fixture(original, "xlsx"));
    assert!(before.contains("蒙特卡罗方法计算 π 值"));
    for target in ["strings/unique.xml", "/xl/strings/unique.xml"] {
        let relocated = transform_package(original, "xlsx", |name, content| {
            let content = if name == "xl/_rels/workbook.xml.rels" {
                String::from_utf8(content)
                    .unwrap()
                    .replace(
                        "Target=\"sharedStrings.xml\"",
                        &format!("Target=\"{target}\""),
                    )
                    .into_bytes()
            } else if name == "[Content_Types].xml" {
                String::from_utf8(content)
                    .unwrap()
                    .replace("/xl/sharedStrings.xml", "/xl/strings/unique.xml")
                    .into_bytes()
            } else {
                content
            };
            let name = if name == "xl/sharedStrings.xml" {
                "xl/strings/unique.xml"
            } else {
                name
            };
            (name.to_string(), content)
        });
        assert_eq!(
            extracted(&relocated),
            before,
            "relationship target: {target}"
        );
    }
}

#[test]
fn office_main_parts_can_move_without_changing_text() {
    for (extension, bytes, old, new) in [
        (
            "docx",
            include_bytes!("fixtures/file_extractor/sample.docx").as_slice(),
            "word/document.xml",
            "word/main.xml",
        ),
        (
            "xlsx",
            include_bytes!("fixtures/file_extractor/sample.xlsx").as_slice(),
            "xl/workbook.xml",
            "xl/main.xml",
        ),
        (
            "pptx",
            include_bytes!("fixtures/file_extractor/sample.pptx").as_slice(),
            "ppt/presentation.xml",
            "ppt/main.xml",
        ),
    ] {
        let before = extracted(&write_fixture(bytes, extension));
        assert!(!before.contains("Office 文本提取未完成"));
        let directory = old.rsplit_once('/').unwrap().0;
        let old_rels = format!("{directory}/_rels/{}.rels", old.rsplit('/').next().unwrap());
        let new_rels = format!("{directory}/_rels/main.xml.rels");
        let moved = transform_package(bytes, extension, |name, content| {
            let content = if name == "_rels/.rels" || name == "[Content_Types].xml" {
                String::from_utf8(content)
                    .unwrap()
                    .replace(old, new)
                    .into_bytes()
            } else {
                content
            };
            let name = if name == old {
                new
            } else if name == old_rels {
                &new_rels
            } else {
                name
            };
            (name.to_string(), content)
        });
        assert_eq!(extracted(&moved), before, "{extension}");
    }
}

#[test]
fn xlsx_root_level_and_cross_directory_main_parts_resolve_relationships() {
    for (main, rels, sheet_target, strings_target) in [
        (
            "book.xml",
            "_rels/book.xml.rels",
            "sheets/data.xml",
            "strings/values.xml",
        ),
        (
            "documents/book.xml",
            "documents/_rels/book.xml.rels",
            "../sheets/data.xml",
            "../strings/values.xml",
        ),
    ] {
        let relationships = format!(
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="{sheet_target}"/><Relationship Id="strings" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="{strings_target}"/></Relationships>"#
        );
        let file = write_office_package(
            &[
                (main, SINGLE_SHEET_WORKBOOK.1),
                (rels, &relationships),
                ("sheets/data.xml", SHARED_CELL.1),
                (
                    "strings/values.xml",
                    r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>Moved together</t></si></sst>"#,
                ),
            ],
            "xlsx",
            main,
        );
        assert_eq!(
            extracted(&file),
            "\n--- Sheet 1: Sheet 1 ---\nMoved together\n",
            "{main}"
        );
    }
}

#[test]
fn xlsx_standard_utf16_shared_strings_match_utf8_text() {
    let xml = r#"<?xml version="1.0" encoding="UTF-16"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>中文 🙂 &amp; café</t></si></sst>"#;
    let file = write_office(
        &[
            SINGLE_SHEET_WORKBOOK,
            SINGLE_SHEET_WITH_STRINGS_RELATIONSHIPS,
            SHARED_CELL,
            ("xl/sharedStrings.xml", xml),
        ],
        "xlsx",
    );
    let original = std::fs::read(file.path()).unwrap();
    for little_endian in [true, false] {
        let encoded = transform_package(&original, "xlsx", |name, content| {
            if name != "xl/sharedStrings.xml" {
                return (name.to_string(), content);
            }
            let mut data = if little_endian {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for unit in xml.encode_utf16() {
                data.extend_from_slice(&if little_endian {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            (name.to_string(), data)
        });
        assert_eq!(
            extracted(&encoded),
            "\n--- Sheet 1: Sheet 1 ---\n中文 🙂 & café\n"
        );
    }
}

#[test]
fn xlsx_strict_namespaces_and_arbitrary_prefixes_preserve_text() {
    let file = write_office(
        &[
            (
                "_rels/.rels",
                r#"<pkg:Relationships xmlns:pkg="http://schemas.openxmlformats.org/package/2006/relationships"><pkg:Relationship Id="main" Type="http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument" Target="xl/workbook.xml"/></pkg:Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<s:workbook xmlns:s="http://purl.oclc.org/ooxml/spreadsheetml/main" xmlns:link="http://purl.oclc.org/ooxml/officeDocument/relationships"><s:sheets><s:sheet name="Strict" sheetId="7" link:id="sheet"/></s:sheets></s:workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="sheet" Type="http://purl.oclc.org/ooxml/officeDocument/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="strings" Type="http://purl.oclc.org/ooxml/officeDocument/relationships/sharedStrings" Target="sharedStrings.xml"/></Relationships>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<tab:worksheet xmlns:tab="http://purl.oclc.org/ooxml/spreadsheetml/main"><tab:sheetData><tab:row><tab:c r="B1" t="s"><tab:v>0</tab:v></tab:c></tab:row></tab:sheetData></tab:worksheet>"#,
            ),
            (
                "xl/sharedStrings.xml",
                r#"<s:sst xmlns:s="http://purl.oclc.org/ooxml/spreadsheetml/main"><s:si><s:t>Prefix independent</s:t></s:si></s:sst>"#,
            ),
        ],
        "xlsx",
    );
    assert_eq!(
        extracted(&file),
        "\n--- Sheet 7: Strict ---\n\tPrefix independent\n"
    );
}

#[test]
fn xlsx_decodes_office_string_escapes_once_after_xml_entities() {
    let file = write_office(
        &[
            (
                "xl/workbook.xml",
                r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet_x0020_1" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            ),
            SINGLE_SHEET_WITH_STRINGS_RELATIONSHIPS,
            (
                "xl/sharedStrings.xml",
                r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>_x0041_ _x005F_x0041_ _xD83D__xDE42_ &amp;</t></si></sst>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row><c r="A1" t="s"><v>0</v></c><c r="B1" t="inlineStr"><is><r><t>_x0042_</t></r><r><t xml:space="preserve"> _x005F_x0042_</t></r></is></c><c r="C1" t="inlineStr"><is><r><t>_x00</t></r><r><t>41_</t></r></is></c></row></sheetData></worksheet>"#,
            ),
        ],
        "xlsx",
    );
    assert_eq!(
        extracted(&file),
        "\n--- Sheet 1: Sheet 1 ---\nA _x0041_ 🙂 &\tB _x0042_\t_x0041_\n"
    );
}

#[test]
fn xlsx_preserves_cached_values_without_evaluating_formulas_or_styles() {
    let file = write_office(
        &[
            SINGLE_SHEET_WORKBOOK,
            SINGLE_SHEET_RELATIONSHIPS,
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>12.50</v></c><c r="B1" t="b"><v>1</v></c><c r="C1" t="e"><v>#DIV/0!</v></c><c r="D1"><f>SUM(A1,5)</f><v>17.5</v></c><c r="E1" t="str"><f>CONCAT("A","B")</f><v>Cached_x0020_text</v></c><c r="F1"><f>NOW()</f></c></row></sheetData></worksheet>"#,
            ),
        ],
        "xlsx",
    );
    assert_eq!(
        extracted(&file),
        "\n--- Sheet 1: Sheet 1 ---\n12.50\t1\t#DIV/0!\t17.5\tCached text\n"
    );
}

#[test]
fn xlsx_missing_external_or_duplicate_shared_string_relationships_are_not_guessed() {
    for extra in [
        "", // A physical default-named table is not a relationship.
        r#"<Relationship Id="strings" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="missing.xml"/>"#,
        r#"<Relationship Id="strings" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="https://example.com/sst.xml" TargetMode="External"/>"#,
        r#"<Relationship Id="strings" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/><Relationship Id="other" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="other.xml"/>"#,
        r#"<Relationship Id="strings" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings"/>"#,
    ] {
        let relationships = format!(
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>{extra}</Relationships>"#
        );
        let file = write_office(
            &[
                SINGLE_SHEET_WORKBOOK,
                ("xl/_rels/workbook.xml.rels", &relationships),
                SHARED_CELL,
                (
                    "xl/sharedStrings.xml",
                    r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>Unbound text</t></si></sst>"#,
                ),
                (
                    "xl/other.xml",
                    r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>Ambiguous</t></si></sst>"#,
                ),
            ],
            "xlsx",
        );
        assert_incomplete(&file);
        assert!(!extracted(&file).contains("Unbound text"));
    }
}

#[test]
fn office_rejects_incomplete_or_ambiguous_package_entrypoints() {
    for root_relationships in [
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>"#,
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="a" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/><Relationship Id="b" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/other.xml"/></Relationships>"#,
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="a" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="https://example.com/book.xml" TargetMode="External"/></Relationships>"#,
        r#"<Relationships xmlns="urn:unrelated"><Relationship Id="a" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
    ] {
        let file = write_office(
            &[
                ("_rels/.rels", root_relationships),
                SINGLE_SHEET_WORKBOOK,
                SINGLE_SHEET_RELATIONSHIPS,
            ],
            "xlsx",
        );
        assert_incomplete(&file);
    }
}

#[test]
fn office_rejects_truncated_multiple_root_and_dtd_xml() {
    for xml in [
        r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>truncated</t></si>"#,
        r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>first</t></si></sst><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"/>"#,
        r#"<!DOCTYPE sst [<!ENTITY text "expanded">]><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>&text;</t></si></sst>"#,
        r#"<sst xmlns="urn:unrelated"><si><t>wrong vocabulary</t></si></sst>"#,
    ] {
        let file = write_office(
            &[
                SINGLE_SHEET_WORKBOOK,
                SINGLE_SHEET_WITH_STRINGS_RELATIONSHIPS,
                SHARED_CELL,
                ("xl/sharedStrings.xml", xml),
            ],
            "xlsx",
        );
        assert_incomplete(&file);
    }
}

#[test]
fn xlsx_foreign_extensions_do_not_add_sheets_cells_or_string_text() {
    let file = write_office(
        &[
            (
                "xl/workbook.xml",
                r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:x="urn:extension"><sheets><x:sheet name="Foreign" sheetId="99" r:id="missing"/><sheet name="Sheet 1" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            ),
            SINGLE_SHEET_WITH_STRINGS_RELATIONSHIPS,
            (
                "xl/sharedStrings.xml",
                r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:x="urn:extension"><x:si><x:t>not an item</x:t></x:si><si><r><t>Actual</t></r><x:t>foreign text</x:t><rPh><t>pronunciation</t></rPh><r><t xml:space="preserve"> value</t></r></si></sst>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:x="urn:extension"><sheetData><row><x:c r="B1" t="inlineStr"><x:is><x:t>foreign cell</x:t></x:is></x:c><c r="A1" t="s"><v>0</v></c></row></sheetData></worksheet>"#,
            ),
        ],
        "xlsx",
    );
    assert_eq!(
        extracted(&file),
        "\n--- Sheet 1: Sheet 1 ---\nActual value\n"
    );
}

#[test]
fn docx_foreign_extension_subtrees_are_not_document_paragraphs() {
    let file = write_office(
        &[(
            "word/document.xml",
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:x="urn:extension"><w:body><x:p><x:t>Foreign paragraph</x:t></x:p><w:p><w:r><w:t>Actual paragraph</w:t><x:t>Foreign text</x:t></w:r></w:p></w:body></w:document>"#,
        )],
        "docx",
    );
    assert_eq!(extracted(&file), "Actual paragraph\n");
}

#[test]
fn xlsx_inline_strings_keep_labels_and_sparse_columns() {
    let file = write_office(
        &[
            SINGLE_SHEET_WORKBOOK,
            SINGLE_SHEET_RELATIONSHIPS,
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row>
        <c r="A1" t="inlineStr"><is><r><t xml:space="preserve">Invoice </t></r><r><t>total</t></r></is></c>
        <c r="C1"><v>123</v></c>
        </row></sheetData></worksheet>"#,
            ),
        ],
        "xlsx",
    );
    let text = try_extract_text(
        file.path(),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    )
    .unwrap();
    assert!(text.contains("Invoice total\t\t123\n"), "{text}");
}

#[test]
fn xlsx_shared_rich_text_is_one_item_including_empty_items() {
    let file = write_office(
        &[
            SINGLE_SHEET_WORKBOOK,
            SINGLE_SHEET_WITH_STRINGS_RELATIONSHIPS,
            (
                "xl/sharedStrings.xml",
                r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
            <si><r><t xml:space="preserve">Invoice </t></r><r><t>total</t></r><rPh><t>annotation</t></rPh></si>
            <si/><si><t>Second cell &amp; &#x4E2D;</t></si>
            </sst>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row>
            <c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c><c r="C1" t="s"><v>2</v></c>
            </row></sheetData></worksheet>"#,
            ),
        ],
        "xlsx",
    );
    let text = try_extract_text(
        file.path(),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    )
    .unwrap();
    assert!(
        text.contains("Invoice total\t\tSecond cell & 中\n"),
        "{text}"
    );
    assert!(!text.contains("annotation"));
}

#[test]
fn xlsx_invalid_shared_index_reports_incomplete_extraction() {
    let file = write_office(
        &[
            SINGLE_SHEET_WORKBOOK,
            SINGLE_SHEET_RELATIONSHIPS,
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row><c r="A1" t="s"><v>123</v></c></row></sheetData></worksheet>"#,
            ),
        ],
        "xlsx",
    );
    let text = try_extract_text(
        file.path(),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    )
    .unwrap();
    assert!(
        text.contains("Office 文本提取未完成") && text.contains("共享字符串"),
        "{text}"
    );
    assert!(!text.contains("--- Sheet"));
}

#[test]
fn docx_preserves_headings_lists_bold_and_tables() {
    let file = write_office(
        &[(
            "word/document.xml",
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
        <w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>Heading</w:t></w:r></w:p>
        <w:p><w:pPr><w:numPr/></w:pPr><w:r><w:rPr><w:b/></w:rPr><w:t>Bold</w:t></w:r><w:r><w:t xml:space="preserve"> plain</w:t></w:r></w:p>
        <w:tbl><w:tr><w:tc><w:p><w:r><w:t>First</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Second</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
        </w:body></w:document>"#,
        )],
        "docx",
    );
    let text = try_extract_text(
        file.path(),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    )
    .unwrap();
    assert!(text.contains("## Heading\n"), "{text}");
    assert!(text.contains("- **Bold** plain\n"), "{text}");
    assert!(
        text.contains("| First | Second |\n| --- | --- |\n"),
        "{text}"
    );
}

#[test]
fn pptx_reads_manifest_order_through_relationships() {
    let file = write_office(
        &[
            (
                "ppt/presentation.xml",
                r#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:rel="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="300" rel:id="lastCreated"/><p:sldId id="200" rel:id="firstCreated"/></p:sldIdLst></p:presentation>"#,
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="firstCreated" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="/ppt/slides/slide2.xml"/><Relationship Id="lastCreated" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide10.xml"/><Relationship Id="website" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/" TargetMode="External"/></Relationships>"#,
            ),
            (
                "ppt/slides/slide10.xml",
                r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:x="urn:extension"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>Tenth</a:t><x:t>Foreign text</x:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#,
            ),
            (
                "ppt/slides/slide2.xml",
                r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>Second</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#,
            ),
        ],
        "pptx",
    );
    let text = try_extract_text(
        file.path(),
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    )
    .unwrap();
    assert_eq!(
        text,
        "\n--- Slide 1 ---\nTenth\n\n--- Slide 2 ---\nSecond\n"
    );
}

#[test]
fn xlsx_binds_sheet_names_and_order_to_relationships_not_filenames() {
    let file = write_office(
        &[
            (
                "xl/workbook.xml",
                r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Expenses" sheetId="42" r:id="expensePart"/><sheet name="Chart" sheetId="43" r:id="chartPart"/><sheet name="Income" sheetId="7" r:id="incomePart"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="incomePart" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="expensePart" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="/xl/worksheets/expenses.xml"/><Relationship Id="chartPart" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chartsheet" Target="chartsheets/sheet1.xml"/></Relationships>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row><c><v>900</v></c></row></sheetData></worksheet>"#,
            ),
            (
                "xl/worksheets/expenses.xml",
                r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row><c><v>250</v></c></row></sheetData></worksheet>"#,
            ),
            (
                "xl/chartsheets/sheet1.xml",
                r#"<chartsheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"/>"#,
            ),
            (
                "xl/worksheets/sheet999.xml",
                r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row><c><v>Unreferenced</v></c></row></sheetData></worksheet>"#,
            ),
        ],
        "xlsx",
    );
    let text = try_extract_text(
        file.path(),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    )
    .unwrap();
    assert_eq!(
        text,
        "\n--- Sheet 42: Expenses ---\n250\n\n--- Sheet 7: Income ---\n900\n"
    );
}

#[test]
fn office_missing_manifest_does_not_guess_parts_from_filenames() {
    for (extension, name, xml) in [
        ("xlsx", "xl/worksheets/sheet1.xml", "<worksheet/>"),
        (
            "pptx",
            "ppt/slides/slide1.xml",
            "<slide><p><t>Orphan</t></p></slide>",
        ),
    ] {
        let file = write_office(&[(name, xml)], extension);
        let text = try_extract_text(file.path(), "application/octet-stream").unwrap();
        assert!(text.contains("Office 文本提取未完成"), "{text}");
        assert!(text.contains("原附件已保留"), "{text}");
    }
}

#[test]
fn xlsx_invalid_references_report_incomplete_extraction() {
    for relationships in [
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>"#,
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/missing.xml"/></Relationships>"#,
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="https://example.com/sheet.xml" TargetMode="External"/></Relationships>"#,
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="../../sheet.xml"/></Relationships>"#,
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/></Relationships>"#,
    ] {
        let file = write_office(
            &[
                SINGLE_SHEET_WORKBOOK,
                ("xl/_rels/workbook.xml.rels", relationships),
            ],
            "xlsx",
        );
        let text = try_extract_text(
            file.path(),
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        )
        .unwrap();
        assert!(text.contains("Office 文本提取未完成"), "{text}");
        assert!(text.contains("原附件已保留"), "{text}");
    }
}

#[test]
fn compressed_docx_with_oversized_xml_event_stops_and_next_document_still_works() {
    let xml = format!(
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>{}</w:t></w:r></w:p></w:body></w:document>"#,
        "x".repeat(9 * 1024 * 1024)
    );
    let file = write_office(&[("word/document.xml", &xml)], "docx");
    assert!(file.as_file().metadata().unwrap().len() < 100_000);
    let text = try_extract_text(
        file.path(),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    )
    .unwrap();
    assert!(text.contains("超过提取预算"), "{text}");
    let normal = write_office(
        &[(
            "word/document.xml",
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Still readable</w:t></w:r></w:p></w:body></w:document>"#,
        )],
        "docx",
    );
    assert_eq!(
        try_extract_text(
            normal.path(),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        )
        .as_deref(),
        Some("Still readable\n")
    );
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
