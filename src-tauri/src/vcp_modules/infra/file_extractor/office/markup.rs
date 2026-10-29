//! The text consumers support one Markup Compatibility branch, never concatenate
//! both a Choice and its Fallback. Unknown requirements use the standard fallback.

use super::xml::{self, Namespace};
use quick_xml::events::BytesStart;
use quick_xml::name::{NamespaceResolver, QName, ResolveResult};

#[derive(Clone, Copy)]
pub(super) enum Start {
    Ordinary,
    Transparent,
    Ignored,
}

struct Alternative {
    depth: usize,
    selected: bool,
    fallback: bool,
}

#[derive(Default)]
pub(super) struct Alternatives {
    stack: Vec<Alternative>,
}

impl Alternatives {
    pub(super) fn start(
        &mut self,
        start: &BytesStart<'_>,
        namespaces: &NamespaceResolver,
        depth: usize,
        enabled: bool,
        supported_namespaces: &[Namespace],
    ) -> Result<Start, String> {
        if !enabled {
            return Ok(Start::Ignored);
        }
        if !xml::in_namespace(start.name(), namespaces, Namespace::MarkupCompatibility) {
            return Ok(Start::Ordinary);
        }
        match start.local_name().as_ref() {
            "AlternateContent" => {
                self.stack.push(Alternative {
                    depth,
                    selected: false,
                    fallback: false,
                });
                Ok(Start::Transparent)
            }
            "Choice" | "Fallback" => {
                let alternative = self
                    .stack
                    .last_mut()
                    .filter(|alternative| depth == alternative.depth + 1)
                    .ok_or("Office XML 兼容分支不在 AlternateContent 内")?;
                if alternative.fallback {
                    return Err("Office XML 兼容分支出现在 Fallback 之后".into());
                }
                let supported = if start.local_name().as_ref() == "Choice" {
                    let requires = xml::attribute(start, "Requires")?
                        .filter(|value| !value.trim().is_empty())
                        .ok_or("Office XML Choice 缺少 Requires")?;
                    let mut supported = true;
                    for prefix in requires.split_whitespace() {
                        let name = format!("{prefix}:_");
                        if !matches!(
                            namespaces.resolve_element(QName(&name)).0,
                            ResolveResult::Bound(_)
                        ) {
                            return Err("Office XML Choice 引用了未声明的命名空间".into());
                        }
                        supported &= supported_namespaces.iter().any(|namespace| {
                            xml::in_namespace(QName(&name), namespaces, *namespace)
                        });
                    }
                    supported
                } else {
                    alternative.fallback = true;
                    true
                };
                if !alternative.selected && supported {
                    alternative.selected = true;
                    Ok(Start::Transparent)
                } else {
                    Ok(Start::Ignored)
                }
            }
            _ => Ok(Start::Ignored),
        }
    }

    pub(super) fn end(&mut self, depth: usize) {
        if self
            .stack
            .last()
            .is_some_and(|alternative| alternative.depth == depth)
        {
            self.stack.pop();
        }
    }
}
