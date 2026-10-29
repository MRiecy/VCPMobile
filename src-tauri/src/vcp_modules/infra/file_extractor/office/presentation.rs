//! Presentation order comes from relationships; only slide text bodies supply text.

use super::markup::{Alternatives, Start};
use super::package::Package;
use super::xml::{self, Namespace};
use super::{append, TextOutput, MAX_BUFFER_BYTES, MAX_PARTS};
use quick_xml::events::Event;
use quick_xml::name::NamespaceResolver;

fn slides(package: &mut Package, main: &str) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    let mut metadata_bytes = 0;
    let mut in_list = false;
    package.read_part(
        main,
        (Namespace::Presentation, "presentation"),
        |event, ns, depth| {
            match event {
                Event::Start(start)
                    if xml::in_namespace(start.name(), ns, Namespace::Presentation) =>
                {
                    if depth == 2 && start.local_name().as_ref() == "sldIdLst" {
                        in_list = true;
                    } else if depth == 3 && in_list && start.local_name().as_ref() == "sldId" {
                        if ids.len() >= MAX_PARTS {
                            return Err("Office 幻灯片数量超过提取预算".into());
                        }
                        let id = xml::qualified_attribute(
                            &start,
                            ns,
                            Namespace::OfficeRelationships,
                            "id",
                        )?
                        .filter(|id| !id.is_empty())
                        .ok_or("PPTX 幻灯片缺少关系 ID")?;
                        if id.len() > MAX_BUFFER_BYTES.saturating_sub(metadata_bytes) {
                            return Err("Office 幻灯片清单超过提取预算".into());
                        }
                        metadata_bytes += id.len();
                        ids.push(id);
                    }
                }
                Event::End(_) if depth == 2 => in_list = false,
                _ => {}
            }
            Ok(true)
        },
    )?;
    package.reserve_metadata(metadata_bytes)?;
    Ok(ids)
}

#[derive(Clone, Copy)]
enum Node {
    Slide,
    Common,
    Shapes,
    Group,
    Shape,
    GraphicFrame,
    Graphic,
    GraphicData,
    Table,
    Row,
    Cell,
    TextBody,
    Paragraph,
    Run,
    Text,
    MathWrapper,
    Math,
    Break,
    Ignored,
}

fn node(parent: Option<Node>, name: &str, presentation: bool, drawing: bool) -> Node {
    if presentation {
        match (parent, name) {
            (None, "sld") => return Node::Slide,
            (Some(Node::Slide), "cSld") => return Node::Common,
            (Some(Node::Common), "spTree") => return Node::Shapes,
            (Some(Node::Shapes | Node::Group), "grpSp") => return Node::Group,
            (Some(Node::Shapes | Node::Group), "sp") => return Node::Shape,
            (Some(Node::Shapes | Node::Group), "graphicFrame") => return Node::GraphicFrame,
            (Some(Node::Shape), "txBody") => return Node::TextBody,
            _ => {}
        }
    }
    if drawing {
        match (parent, name) {
            (Some(Node::GraphicFrame), "graphic") => return Node::Graphic,
            (Some(Node::Graphic), "graphicData") => return Node::GraphicData,
            (Some(Node::GraphicData), "tbl") => return Node::Table,
            (Some(Node::Table), "tr") => return Node::Row,
            (Some(Node::Row), "tc") => return Node::Cell,
            (Some(Node::Cell), "txBody") => return Node::TextBody,
            (Some(Node::TextBody), "p") => return Node::Paragraph,
            (Some(Node::Paragraph), "r" | "fld") => return Node::Run,
            (Some(Node::Paragraph), "br") => return Node::Break,
            (Some(Node::Run), "t") => return Node::Text,
            _ => {}
        }
    }
    Node::Ignored
}

#[derive(Default)]
struct SlideText {
    nodes: Vec<(Node, bool)>,
    alternatives: Alternatives,
    text: String,
}

impl SlideText {
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
                        Namespace::Presentation,
                        Namespace::Drawing,
                        Namespace::Math,
                        Namespace::Drawing2010,
                    ],
                )? {
                    Start::Transparent => (parent.unwrap_or(Node::Ignored), false),
                    Start::Ignored => (Node::Ignored, false),
                    Start::Ordinary => {
                        let node = if xml::in_namespace(start.name(), ns, Namespace::Math) {
                            match (parent, name.as_ref()) {
                                (Some(Node::Math), "t") => Node::Text,
                                (Some(Node::Math), _) => Node::Math,
                                (
                                    Some(Node::MathWrapper | Node::Paragraph),
                                    "oMath" | "oMathPara",
                                ) => Node::Math,
                                _ => Node::Ignored,
                            }
                        } else if matches!(parent, Some(Node::Paragraph))
                            && xml::in_namespace(start.name(), ns, Namespace::Drawing2010)
                            && name.as_ref() == "m"
                        {
                            Node::MathWrapper
                        } else {
                            node(
                                parent,
                                name.as_ref(),
                                xml::in_namespace(start.name(), ns, Namespace::Presentation),
                                xml::in_namespace(start.name(), ns, Namespace::Drawing),
                            )
                        };
                        (node, true)
                    }
                };
                if owned && matches!(node, Node::Break) {
                    append(&mut self.text, "\n")?;
                }
                self.nodes.push((node, owned));
            }
            Event::End(_) => {
                self.alternatives.end(depth);
                if matches!(self.nodes.pop(), Some((Node::Paragraph, true))) {
                    append(&mut self.text, "\n")?;
                }
            }
            _ if matches!(self.nodes.last(), Some((Node::Text, _))) => {
                if let Some(value) = xml::text(&event)? {
                    append(&mut self.text, &value)?;
                }
            }
            _ => {}
        }
        Ok(true)
    }
}

pub(super) fn extract(
    package: &mut Package,
    main: &str,
    output: &mut TextOutput,
) -> Result<(), String> {
    let ids = slides(package, main)?;
    let relationships = package.relationships(main)?;
    for (index, id) in ids.iter().enumerate() {
        let name = relationships.by_id(id, "slide")?.target(main)?;
        let mut slide = SlideText::default();
        package.read_part(
            &name,
            (Namespace::Presentation, "sld"),
            |event, ns, depth| slide.event(event, ns, depth),
        )?;
        if !slide.text.trim().is_empty() {
            output.push(&format!("\n--- Slide {} ---\n", index + 1));
            output.push(&slide.text);
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
    use std::io::Cursor;

    #[test]
    fn shapes_and_tables_keep_text_without_extension_lookalikes() {
        let source = r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:x="urn:other"><p:cSld><p:spTree><p:grpSp><p:sp><p:txBody><a:p><a:r><a:t>Shape</a:t></a:r><a:br/><a:fld><a:t>Field</a:t></a:fld><x:r><a:r><a:t>Hidden</a:t></a:r></x:r></a:p></p:txBody></p:sp></p:grpSp><p:graphicFrame><a:graphic><a:graphicData><a:tbl><a:tr><a:tc><a:txBody><a:p><a:r><a:t>Cell</a:t></a:r></a:p></a:txBody></a:tc></a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame></p:spTree></p:cSld></p:sld>"#;
        let mut slide = SlideText::default();
        xml::read(
            Cursor::new(source.as_bytes()),
            &mut 100000,
            100000,
            (Namespace::Presentation, "sld"),
            |event, ns, depth| slide.event(event, ns, depth),
        )
        .unwrap();
        assert_eq!(slide.text, "Shape\nField\nCell\n");
    }

    #[test]
    fn compatible_math_shape_is_selected_once_before_fallback() {
        let source = r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:a14="http://schemas.microsoft.com/office/drawing/2010/main" xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><p:cSld><p:spTree><mc:AlternateContent><mc:Choice Requires="a14"><p:sp><p:txBody><a:p><a:r><a:t>Formula: </a:t></a:r><a14:m><m:oMath><m:r><m:t>x+y</m:t></m:r></m:oMath></a14:m></a:p></p:txBody></p:sp></mc:Choice><mc:Fallback><p:sp><p:txBody><a:p><a:r><a:t>Duplicate</a:t></a:r></a:p></p:txBody></p:sp></mc:Fallback></mc:AlternateContent></p:spTree></p:cSld></p:sld>"#;
        let mut slide = SlideText::default();
        xml::read(
            Cursor::new(source.as_bytes()),
            &mut 100000,
            100000,
            (Namespace::Presentation, "sld"),
            |event, ns, depth| slide.event(event, ns, depth),
        )
        .unwrap();
        assert_eq!(slide.text, "Formula: x+y\n");
    }
}
