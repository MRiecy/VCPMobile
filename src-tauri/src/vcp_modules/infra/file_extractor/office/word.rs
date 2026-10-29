//! WordprocessingML body text. Structural wrappers retain their content context;
//! unrelated namespaces and property/extension subtrees never become body text.

use super::markup::{Alternatives, Start};
use super::package::Package;
use super::xml::{self, Namespace};
use super::{append, TextOutput, MAX_BUFFER_BYTES, MAX_COLUMNS};
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::NamespaceResolver;

#[derive(Clone, Copy)]
enum Content {
    Blocks,
    Runs,
    Rows,
    Cells,
}

#[derive(Clone, Copy)]
enum Node {
    Document,
    Body,
    Paragraph,
    ParagraphProperties,
    Run,
    RunProperties,
    Text,
    Math,
    VmlPicture,
    VmlGroup,
    VmlShape,
    VmlTextBox,
    Drawing,
    DrawingViewport,
    Graphic,
    GraphicData,
    DrawingGroup,
    DrawingShape,
    DrawingTextBox,
    TextBoxContent,
    Table,
    Row,
    Cell,
    Container(Content),
    ContentControl(Content),
    Heading,
    List,
    Bold,
    Tab,
    Break,
    Ignored,
}

impl Node {
    fn content(self) -> Option<Content> {
        match self {
            Self::Body | Self::Cell | Self::TextBoxContent => Some(Content::Blocks),
            Self::Paragraph => Some(Content::Runs),
            Self::Table => Some(Content::Rows),
            Self::Row => Some(Content::Cells),
            Self::Container(content) => Some(content),
            _ => None,
        }
    }
}

fn node(parent: Option<Node>, name: &str) -> Node {
    match (parent, name) {
        (None, "document") => return Node::Document,
        (Some(Node::Document), "body") => return Node::Body,
        (Some(Node::Paragraph), "pPr") => return Node::ParagraphProperties,
        (Some(Node::Run), "rPr") => return Node::RunProperties,
        (Some(Node::Run), "t") => return Node::Text,
        (Some(Node::Run), "tab") => return Node::Tab,
        (Some(Node::Run), "br" | "cr") => return Node::Break,
        (Some(Node::Run), "pict") => return Node::VmlPicture,
        (Some(Node::Run), "drawing") => return Node::Drawing,
        (Some(Node::VmlTextBox | Node::DrawingTextBox), "txbxContent") => {
            return Node::TextBoxContent
        }
        (Some(Node::ParagraphProperties), "pStyle") => return Node::Heading,
        (Some(Node::ParagraphProperties), "numPr") => return Node::List,
        (Some(Node::RunProperties), "b" | "bCs") => return Node::Bold,
        (Some(Node::ContentControl(content)), "sdtContent") => return Node::Container(content),
        _ => {}
    }
    let Some(content) = parent.and_then(Node::content) else {
        return Node::Ignored;
    };
    match (content, name) {
        (Content::Blocks, "p") => Node::Paragraph,
        (Content::Blocks, "tbl") => Node::Table,
        (Content::Runs, "r") => Node::Run,
        (Content::Runs, "hyperlink" | "fldSimple" | "smartTag") => Node::Container(content),
        (Content::Rows, "tr") => Node::Row,
        (Content::Cells, "tc") => Node::Cell,
        (_, "customXml" | "ins" | "moveTo") => Node::Container(content),
        (_, "sdt") => Node::ContentControl(content),
        _ => Node::Ignored,
    }
}

fn drawing_node(parent: Option<Node>, start: &BytesStart<'_>, ns: &NamespaceResolver) -> Node {
    let local = start.local_name();
    let name = local.as_ref();
    if xml::in_namespace(start.name(), ns, Namespace::Vml) {
        match (parent, name) {
            (Some(Node::VmlPicture | Node::VmlGroup), "group") => return Node::VmlGroup,
            (
                Some(Node::VmlPicture | Node::VmlGroup),
                "shape" | "rect" | "roundrect" | "oval" | "line" | "arc" | "curve" | "polyline"
                | "image",
            ) => return Node::VmlShape,
            (Some(Node::VmlShape), "textbox") => return Node::VmlTextBox,
            _ => {}
        }
    } else if xml::in_namespace(start.name(), ns, Namespace::WordDrawing) {
        if matches!(parent, Some(Node::Drawing)) && matches!(name, "inline" | "anchor") {
            return Node::DrawingViewport;
        }
    } else if xml::in_namespace(start.name(), ns, Namespace::Drawing) {
        match (parent, name) {
            (Some(Node::DrawingViewport), "graphic") => return Node::Graphic,
            (Some(Node::Graphic), "graphicData") => return Node::GraphicData,
            _ => {}
        }
    } else if xml::in_namespace(start.name(), ns, Namespace::WordGroup) {
        if matches!(parent, Some(Node::GraphicData | Node::DrawingGroup)) && name == "wgp" {
            return Node::DrawingGroup;
        }
    } else if xml::in_namespace(start.name(), ns, Namespace::WordShape) {
        match (parent, name) {
            (Some(Node::GraphicData | Node::DrawingGroup), "wsp") => return Node::DrawingShape,
            (Some(Node::DrawingShape), "txbx") => return Node::DrawingTextBox,
            _ => {}
        }
    }
    Node::Ignored
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

#[derive(Default)]
struct Paragraph {
    text: String,
    level: usize,
    list: bool,
    bold: bool,
    bold_open: bool,
    written: bool,
    boxed: bool,
}

#[derive(Default)]
struct Row {
    cells: Vec<String>,
    cell: Option<String>,
}

struct Table {
    first_row: bool,
    row: Option<Row>,
}

struct WordText<'a> {
    output: &'a mut TextOutput,
    nodes: Vec<(Node, bool)>,
    alternatives: Alternatives,
    paragraphs: Vec<Paragraph>,
    text_boxes: usize,
    tables: Vec<Table>,
    pending_bytes: usize,
}

fn append_pending(buffer: &mut String, value: &str, pending: &mut usize) -> Result<(), String> {
    if value.len() > MAX_BUFFER_BYTES.saturating_sub(*pending) {
        return Err("Office 段落或表格缓冲超过提取预算".into());
    }
    append(buffer, value)?;
    *pending += value.len();
    Ok(())
}

impl<'a> WordText<'a> {
    fn new(output: &'a mut TextOutput) -> Self {
        Self {
            output,
            nodes: Vec::new(),
            alternatives: Alternatives::default(),
            paragraphs: Vec::new(),
            text_boxes: 0,
            tables: Vec::new(),
            pending_bytes: 0,
        }
    }

    fn paragraph_mut(&mut self) -> Result<&mut Paragraph, String> {
        self.paragraphs
            .last_mut()
            .ok_or_else(|| "DOCX 段落结构无效".into())
    }

    fn append_paragraph(&mut self, value: &str) -> Result<(), String> {
        let paragraph = self.paragraphs.last_mut().ok_or("DOCX 段落结构无效")?;
        append_pending(&mut paragraph.text, value, &mut self.pending_bytes)
    }

    /// A nested table emits into its parent cell; top-level rows stream directly
    /// to output. All retained paragraph/cell strings share one bounded budget.
    fn emit(&mut self, value: &str, table: Option<usize>) -> Result<(), String> {
        if let Some(index) = table {
            let cell = self.tables[index]
                .row
                .as_mut()
                .and_then(|row| row.cell.as_mut())
                .ok_or("DOCX 表格单元格结构无效")?;
            append_pending(cell, value, &mut self.pending_bytes)
        } else {
            self.output.push(value);
            Ok(())
        }
    }

    fn emit_paragraph(
        &mut self,
        paragraph: &mut Paragraph,
        boundary: bool,
        finish: bool,
    ) -> Result<(), String> {
        if paragraph.bold_open {
            append_pending(&mut paragraph.text, "**", &mut self.pending_bytes)?;
            paragraph.bold_open = false;
        }
        self.pending_bytes -= paragraph.text.len();
        let retained = std::mem::take(&mut paragraph.text);
        let text = retained.trim();
        let table = self.tables.len().checked_sub(1);
        if !text.is_empty() {
            if paragraph.level > 0 && !paragraph.written {
                self.emit(&format!("\n{} ", "#".repeat(paragraph.level)), table)?;
            } else if paragraph.list && !paragraph.written {
                self.emit("- ", table)?;
            }
            self.emit(text, table)?;
            if table.is_none() || paragraph.boxed || boundary {
                self.emit("\n", table)?;
            }
            paragraph.written = true;
        } else if finish && !paragraph.written && table.is_none() {
            self.output.push("\n");
        }
        Ok(())
    }

    fn finish_paragraph(&mut self) -> Result<(), String> {
        let mut paragraph = self.paragraphs.pop().ok_or("DOCX 段落结构无效")?;
        self.emit_paragraph(&mut paragraph, false, true)
    }

    fn begin_text_box(&mut self) -> Result<(), String> {
        let mut outer = self.paragraphs.pop().ok_or("DOCX 文本框缺少外层段落")?;
        // Preserve source order and the enclosing run/property state while each
        // inner paragraph owns its own buffer. Never overwrite the outer text.
        self.emit_paragraph(&mut outer, true, false)?;
        self.paragraphs.push(outer);
        self.text_boxes += 1;
        Ok(())
    }

    fn finish_row(&mut self) -> Result<(), String> {
        let table = self.tables.last_mut().ok_or("DOCX 表格行结构无效")?;
        let row = table.row.take().ok_or("DOCX 表格行结构无效")?;
        if row.cells.is_empty() {
            return Ok(());
        }
        let first_row = table.first_row;
        table.first_row = false;
        self.pending_bytes -= row.cells.iter().map(String::len).sum::<usize>();
        let target = self.tables.len().checked_sub(2);
        self.emit("| ", target)?;
        for (index, cell) in row.cells.iter().enumerate() {
            if index > 0 {
                self.emit(" | ", target)?;
            }
            self.emit(cell.trim(), target)?;
        }
        self.emit(" |\n", target)?;
        if first_row {
            self.emit("| ", target)?;
            for index in 0..row.cells.len() {
                if index > 0 {
                    self.emit(" | ", target)?;
                }
                self.emit("---", target)?;
            }
            self.emit(" |\n", target)?;
        }
        Ok(())
    }

    fn start(
        &mut self,
        node: Node,
        start: &BytesStart<'_>,
        ns: &NamespaceResolver,
    ) -> Result<(), String> {
        match node {
            Node::Paragraph => self.paragraphs.push(Paragraph {
                boxed: self.text_boxes > 0,
                ..Paragraph::default()
            }),
            Node::TextBoxContent => self.begin_text_box()?,
            Node::Run | Node::Math => self.paragraph_mut()?.bold = false,
            Node::Heading => {
                self.paragraph_mut()?.level = heading(
                    &xml::qualified_attribute(start, ns, Namespace::Word, "val")?
                        .unwrap_or_default(),
                );
            }
            Node::List => self.paragraph_mut()?.list = true,
            Node::Bold => {
                self.paragraph_mut()?.bold = !matches!(
                    xml::qualified_attribute(start, ns, Namespace::Word, "val")?.as_deref(),
                    Some("0" | "false" | "off")
                );
            }
            Node::Tab => self.append_paragraph("\t")?,
            Node::Break => self.append_paragraph("\n")?,
            Node::Table => {
                self.emit("\n", self.tables.len().checked_sub(1))?;
                self.tables.push(Table {
                    first_row: true,
                    row: None,
                });
            }
            Node::Row => {
                self.tables.last_mut().ok_or("DOCX 表格行结构无效")?.row = Some(Row::default());
            }
            Node::Cell => {
                let row = self
                    .tables
                    .last_mut()
                    .and_then(|table| table.row.as_mut())
                    .ok_or("DOCX 表格单元格结构无效")?;
                if row.cells.len() >= MAX_COLUMNS {
                    return Err("Office 表格行超过提取预算".into());
                }
                row.cell = Some(String::new());
            }
            _ => {}
        }
        Ok(())
    }

    fn event(
        &mut self,
        event: Event<'_>,
        ns: &NamespaceResolver,
        depth: usize,
    ) -> Result<bool, String> {
        match &event {
            Event::Start(start) => {
                let parent = self.nodes.last().map(|(node, _)| *node);
                let name = start.local_name();
                let (node, owned) = match self.alternatives.start(
                    start,
                    ns,
                    depth,
                    !matches!(parent, Some(Node::Ignored)),
                    &[
                        Namespace::Word,
                        Namespace::Math,
                        Namespace::Vml,
                        Namespace::WordDrawing,
                        Namespace::Drawing,
                        Namespace::WordShape,
                        Namespace::WordGroup,
                    ],
                )? {
                    Start::Transparent => (parent.unwrap_or(Node::Ignored), false),
                    Start::Ignored => (Node::Ignored, false),
                    Start::Ordinary => {
                        let node = if xml::in_namespace(start.name(), ns, Namespace::Word) {
                            node(parent, name.as_ref())
                        } else if xml::in_namespace(start.name(), ns, Namespace::Math) {
                            match (parent, name.as_ref()) {
                                (Some(Node::Math), "t") => Node::Text,
                                (Some(Node::Math), _) => Node::Math,
                                (_, "oMath" | "oMathPara")
                                    if matches!(
                                        parent.and_then(Node::content),
                                        Some(Content::Runs)
                                    ) =>
                                {
                                    Node::Math
                                }
                                _ => Node::Ignored,
                            }
                        } else {
                            drawing_node(parent, start, ns)
                        };
                        (node, true)
                    }
                };
                if owned {
                    self.start(node, start, ns)?;
                }
                self.nodes.push((node, owned));
            }
            Event::End(_) => {
                self.alternatives.end(depth);
                let (node, owned) = self.nodes.pop().ok_or("DOCX 元素结构无效")?;
                match if owned { node } else { Node::Ignored } {
                    Node::Paragraph => self.finish_paragraph()?,
                    Node::TextBoxContent => self.text_boxes -= 1,
                    Node::Cell => {
                        let row = self
                            .tables
                            .last_mut()
                            .and_then(|table| table.row.as_mut())
                            .ok_or("DOCX 表格单元格结构无效")?;
                        row.cells
                            .push(row.cell.take().ok_or("DOCX 表格单元格结构无效")?);
                    }
                    Node::Row => self.finish_row()?,
                    Node::Table => {
                        self.tables.pop();
                        self.emit("\n", self.tables.len().checked_sub(1))?;
                    }
                    _ => {}
                }
            }
            _ if matches!(self.nodes.last(), Some((Node::Text, _))) => {
                if let Some(value) = xml::text(&event)? {
                    let paragraph = self.paragraph_mut()?;
                    if paragraph.bold_open != paragraph.bold {
                        paragraph.bold_open = paragraph.bold;
                        self.append_paragraph("**")?;
                    }
                    self.append_paragraph(&value)?;
                }
            }
            _ => {}
        }
        Ok(!self.output.truncated)
    }
}

pub(super) fn extract(
    package: &mut Package,
    main: &str,
    output: &mut TextOutput,
) -> Result<(), String> {
    let mut body = WordText::new(output);
    package.read_part(main, (Namespace::Word, "document"), |event, ns, depth| {
        body.event(event, ns, depth)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn read(body: &str) -> String {
        let xml = format!(
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:v="urn:schemas-microsoft-com:vml" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:wpg="http://schemas.microsoft.com/office/word/2010/wordprocessingGroup" xmlns:x="urn:other"><w:body>{body}</w:body></w:document>"#
        );
        let mut output = TextOutput::new(10000);
        let mut body = WordText::new(&mut output);
        xml::read(
            Cursor::new(xml.as_bytes()),
            &mut 100000,
            100000,
            (Namespace::Word, "document"),
            |event, ns, depth| body.event(event, ns, depth),
        )
        .unwrap();
        output.finish()
    }

    #[test]
    fn wrappers_keep_body_text_without_reading_extension_subtrees() {
        let text = read(
            r#"<w:sdt><w:sdtPr><w:p><w:r><w:t>Properties</w:t></w:r></w:p></w:sdtPr><w:sdtContent><w:p><w:hyperlink><w:r><w:t>Link</w:t></w:r></w:hyperlink><w:ins><w:r><w:t> inserted</w:t></w:r></w:ins><x:r><w:r><w:t>Hidden</w:t></w:r></x:r></w:p></w:sdtContent></w:sdt>"#,
        );
        assert_eq!(text, "Link inserted\n");
    }

    #[test]
    fn nested_table_finishes_inside_its_parent_cell() {
        let text = read(
            r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Before</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>Nested</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p><w:r><w:t>After</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Sibling</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
        );
        assert!(
            text.contains("Before\n| Nested |\n| --- |\n\nAfter | Sibling"),
            "{text}"
        );
    }

    #[test]
    fn math_text_and_one_compatibility_branch_remain_in_body_order() {
        let text = read(
            r#"<mc:AlternateContent><mc:Choice Requires="m"><w:p><m:oMathPara><m:oMath><m:f><m:num><m:r><m:t>a</m:t></m:r></m:num><m:den><m:r><m:t>b</m:t></m:r></m:den></m:f></m:oMath></m:oMathPara></w:p></mc:Choice><mc:Fallback><w:p><w:r><w:t>Duplicate fallback</w:t></w:r></w:p></mc:Fallback></mc:AlternateContent><mc:AlternateContent><mc:Choice Requires="x"><w:p><w:r><w:t>Unsupported choice</w:t></w:r></w:p></mc:Choice><mc:Fallback><w:p><w:r><w:t>Fallback</w:t></w:r></w:p></mc:Fallback></mc:AlternateContent>"#,
        );
        assert_eq!(text, "ab\nFallback\n");
    }

    #[test]
    fn vml_text_box_keeps_inner_paragraphs_and_surrounding_run_state() {
        let text = read(
            r#"<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Outer before </w:t><w:pict><v:shape><v:textbox><w:txbxContent><w:p><w:r><w:t>Textbox first</w:t></w:r></w:p><w:p><w:r><w:t>Textbox second</w:t></w:r></w:p></w:txbxContent></v:textbox></v:shape></w:pict><w:t>Outer after</w:t></w:r></w:p>"#,
        );
        assert_eq!(
            text,
            "**Outer before **\nTextbox first\nTextbox second\n**Outer after**\n"
        );
    }

    #[test]
    fn drawing_text_box_stays_in_its_table_cell_and_chooses_one_representation() {
        let text = read(
            r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Before</w:t><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing><wp:inline><a:graphic><a:graphicData><wpg:wgp><wps:wsp><wps:txbx><w:txbxContent><w:p><w:r><w:t>Box first</w:t></w:r></w:p><w:p><w:r><w:t>Box second</w:t></w:r></w:p></w:txbxContent></wps:txbx></wps:wsp></wpg:wgp></a:graphicData></a:graphic></wp:inline></w:drawing></mc:Choice><mc:Fallback><w:t>Duplicate fallback</w:t></mc:Fallback></mc:AlternateContent><w:t>After</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Sibling</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
        );
        assert!(
            text.contains("Before\nBox first\nBox second\nAfter | Sibling"),
            "{text}"
        );
        assert!(!text.contains("Duplicate"));
    }
}
