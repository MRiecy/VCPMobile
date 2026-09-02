use crate::vcp_modules::pre_renderer::code_highlighter::IncrementalCodeHighlighter;
use crate::vcp_modules::pre_renderer::markdown_ast::{InlineNode, MarkdownNode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "op")]
pub enum AstMutation {
    #[serde(rename = "add")]
    Add {
        id: String,
        parent: String,
        node: MarkdownNode,
    },
    #[serde(rename = "add_inline")]
    AddInline {
        id: String,
        parent: String,
        node: InlineNode,
    },
    /// 新增一个列表项（<li>）。列表项是「多个块级节点」的集合，无法用 Add 的单一 MarkdownNode 表达，
    /// 故单列一个变体。id 为 <li> 的路径 ID（如 "t3.li5"），parent 为列表 <ul>/<ol> 的 ID（如 "t3"）。
    #[serde(rename = "add_list_item")]
    AddListItem {
        id: String,
        parent: String,
        children: Vec<MarkdownNode>,
    },
    #[serde(rename = "text")]
    UpdateText { id: String, value: String },
    #[serde(rename = "append")]
    AppendText { id: String, chunk: String },
    #[serde(rename = "prop")]
    UpdateProp {
        id: String,
        key: String,
        value: String,
    },
    #[serde(rename = "replace")]
    Replace { id: String, node: MarkdownNode },
    #[serde(rename = "patch_code")]
    PatchCode {
        id: String,
        completed_html: String,
        active_html: String,
    },
    #[serde(rename = "replace_inline")]
    ReplaceInline { id: String, node: InlineNode },
    #[serde(rename = "remove")]
    Remove { id: String },
}

/// 对外暴露的 AST 对比入口
#[allow(dead_code)]
pub fn diff_ast(
    old_nodes: &[MarkdownNode],
    new_nodes: &[MarkdownNode],
    prefix: &str,
) -> Vec<AstMutation> {
    let mut mutations = Vec::new();
    let mut highlighter = None;
    diff_markdown_nodes_inner(
        old_nodes,
        new_nodes,
        "root",
        prefix,
        &mut mutations,
        &mut highlighter,
    );
    mutations
}

/// Aurora 专用 AST diff：代码块严格追加时输出增量高亮补丁，其余节点沿用通用 diff。
pub fn diff_ast_streaming(
    old_nodes: &[MarkdownNode],
    new_nodes: &[MarkdownNode],
    prefix: &str,
    code_highlighter: &mut IncrementalCodeHighlighter,
) -> Vec<AstMutation> {
    code_highlighter.begin_frame();
    let mut mutations = Vec::new();
    let mut highlighter = Some(code_highlighter);
    diff_markdown_nodes_inner(
        old_nodes,
        new_nodes,
        "root",
        prefix,
        &mut mutations,
        &mut highlighter,
    );
    if let Some(highlighter) = highlighter {
        mark_stream_code_nodes(new_nodes, prefix, highlighter);
        highlighter.finish_frame();
    }
    mutations
}

/// epoch/reset 后只推进后端行状态，不保留已生成 HTML 镜像。
pub fn prime_stream_code_highlighter(
    nodes: &[MarkdownNode],
    prefix: &str,
    code_highlighter: &mut IncrementalCodeHighlighter,
) {
    code_highlighter.clear();
    code_highlighter.begin_frame();
    prime_stream_code_nodes(nodes, prefix, code_highlighter);
    code_highlighter.finish_frame();
}

/// Snapshot/恢复帧按需生成完整高亮 DOM；临时状态随函数返回立即释放。
pub fn render_stream_snapshot(nodes: &[MarkdownNode], prefix: &str) -> Vec<MarkdownNode> {
    let mut highlighter = IncrementalCodeHighlighter::default();
    highlighter.begin_frame();
    nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            prepare_stream_node(node, &format!("{prefix}{index}"), &mut highlighter)
        })
        .collect()
}

#[allow(dead_code)]
pub fn diff_markdown_nodes(
    old_list: &[MarkdownNode],
    new_list: &[MarkdownNode],
    parent_id: &str,
    prefix: &str,
    mutations: &mut Vec<AstMutation>,
) {
    let mut highlighter = None;
    diff_markdown_nodes_inner(
        old_list,
        new_list,
        parent_id,
        prefix,
        mutations,
        &mut highlighter,
    );
}

fn diff_markdown_nodes_inner(
    old_list: &[MarkdownNode],
    new_list: &[MarkdownNode],
    parent_id: &str,
    prefix: &str,
    mutations: &mut Vec<AstMutation>,
    highlighter: &mut Option<&mut IncrementalCodeHighlighter>,
) {
    let common_len = old_list.len().min(new_list.len());

    // 1. 对比公共部分
    for i in 0..common_len {
        let node_id = format!("{}{}", prefix, i);
        let old_node = &old_list[i];
        let new_node = &new_list[i];

        if old_node.get_hash() == new_node.get_hash() && old_node.get_hash().is_some() {
            continue; // Hash 命中，相同，直接跳过
        }

        diff_single_markdown_node(old_node, new_node, &node_id, mutations, highlighter);
    }

    // 2. 新增的尾部节点
    for (i, item) in new_list.iter().enumerate().skip(common_len) {
        let node_id = format!("{}{}", prefix, i);
        let node = highlighter.as_deref_mut().map_or_else(
            || item.clone(),
            |highlighter| prepare_stream_node(item, &node_id, highlighter),
        );
        mutations.push(AstMutation::Add {
            id: node_id,
            parent: parent_id.to_string(),
            node,
        });
    }

    // 3. 删除的尾部节点
    for i in common_len..old_list.len() {
        let node_id = format!("{}{}", prefix, i);
        mutations.push(AstMutation::Remove { id: node_id });
    }
}

fn diff_single_markdown_node(
    old_node: &MarkdownNode,
    new_node: &MarkdownNode,
    node_id: &str,
    mutations: &mut Vec<AstMutation>,
    highlighter: &mut Option<&mut IncrementalCodeHighlighter>,
) {
    if std::mem::discriminant(old_node) != std::mem::discriminant(new_node) {
        // 类型不同，直接 Replace
        let node = highlighter.as_deref_mut().map_or_else(
            || new_node.clone(),
            |highlighter| prepare_stream_node(new_node, node_id, highlighter),
        );
        mutations.push(AstMutation::Replace {
            id: node_id.to_string(),
            node,
        });
        return;
    }

    match (old_node, new_node) {
        (
            MarkdownNode::Paragraph {
                children: old_children,
                ..
            },
            MarkdownNode::Paragraph {
                children: new_children,
                ..
            },
        ) => {
            diff_inline_nodes(
                old_children,
                new_children,
                node_id,
                &format!("{}.i", node_id),
                mutations,
            );
        }
        (
            MarkdownNode::Heading {
                level: old_level,
                children: old_children,
                ..
            },
            MarkdownNode::Heading {
                level: new_level,
                children: new_children,
                ..
            },
        ) => {
            if old_level != new_level {
                mutations.push(AstMutation::UpdateProp {
                    id: node_id.to_string(),
                    key: "level".to_string(),
                    value: new_level.to_string(),
                });
            }
            diff_inline_nodes(
                old_children,
                new_children,
                node_id,
                &format!("{}.i", node_id),
                mutations,
            );
        }
        (
            MarkdownNode::Blockquote {
                children: old_children,
                ..
            },
            MarkdownNode::Blockquote {
                children: new_children,
                ..
            },
        ) => {
            diff_markdown_nodes_inner(
                old_children,
                new_children,
                node_id,
                &format!("{}.b", node_id),
                mutations,
                highlighter,
            );
        }
        (
            MarkdownNode::List {
                ordered: old_ordered,
                items: old_items,
                ..
            },
            MarkdownNode::List {
                ordered: new_ordered,
                items: new_items,
                ..
            },
        ) => {
            if old_ordered != new_ordered {
                // 有序/无序切换会改变标签名（ul<->ol），无法原地修改，只能整体 Replace
                let node = highlighter.as_deref_mut().map_or_else(
                    || new_node.clone(),
                    |highlighter| prepare_stream_node(new_node, node_id, highlighter),
                );
                mutations.push(AstMutation::Replace {
                    id: node_id.to_string(),
                    node,
                });
            } else {
                let common_len = old_items.len().min(new_items.len());
                // 1. 公共项：逐项递归 diff（item 内块级子节点 parent 为该 <li>）
                for i in 0..common_len {
                    let item_prefix = format!("{}.li{}", node_id, i);
                    diff_markdown_nodes_inner(
                        &old_items[i],
                        &new_items[i],
                        &item_prefix,
                        &format!("{}.b", item_prefix),
                        mutations,
                        highlighter,
                    );
                }
                // 2. 新增的尾部列表项：item 级别增量 Add（挂到 <ul>/<ol> 下），不再整表重建
                for (i, item) in new_items.iter().enumerate().skip(common_len) {
                    let item_id = format!("{}.li{}", node_id, i);
                    let children = highlighter.as_deref_mut().map_or_else(
                        || item.clone(),
                        |highlighter| {
                            item.iter()
                                .enumerate()
                                .map(|(block_index, child)| {
                                    prepare_stream_node(
                                        child,
                                        &format!("{}.b{}", item_id, block_index),
                                        highlighter,
                                    )
                                })
                                .collect()
                        },
                    );
                    mutations.push(AstMutation::AddListItem {
                        id: item_id,
                        parent: node_id.to_string(),
                        children,
                    });
                }
                // 3. 删除的尾部列表项：直接 Remove 对应 <li>
                for i in new_items.len()..old_items.len() {
                    mutations.push(AstMutation::Remove {
                        id: format!("{}.li{}", node_id, i),
                    });
                }
            }
        }
        (
            MarkdownNode::CodeBlock {
                lang: old_lang,
                code: old_code,
                ..
            },
            MarkdownNode::CodeBlock {
                lang: new_lang,
                code: new_code,
                ..
            },
        ) => {
            let lang = new_lang.as_deref().unwrap_or("plaintext");
            let patch = highlighter.as_deref_mut().and_then(|highlighter| {
                (old_lang == new_lang && lang != "mermaid")
                    .then(|| highlighter.append(node_id, old_code, new_code, lang))
                    .flatten()
            });

            if let Some(patch) = patch {
                mutations.push(AstMutation::PatchCode {
                    id: node_id.to_string(),
                    completed_html: patch.completed_html,
                    active_html: patch.active_html,
                });
            } else {
                let node = highlighter.as_deref_mut().map_or_else(
                    || new_node.clone(),
                    |highlighter| prepare_stream_node(new_node, node_id, highlighter),
                );
                mutations.push(AstMutation::Replace {
                    id: node_id.to_string(),
                    node,
                });
            }
        }
        // Table、RawHtml、ThematicBreak 变化时直接 Replace 整个节点。
        _ => {
            let node = highlighter.as_deref_mut().map_or_else(
                || new_node.clone(),
                |highlighter| prepare_stream_node(new_node, node_id, highlighter),
            );
            mutations.push(AstMutation::Replace {
                id: node_id.to_string(),
                node,
            });
        }
    }
}

fn prepare_stream_node(
    node: &MarkdownNode,
    node_id: &str,
    highlighter: &mut IncrementalCodeHighlighter,
) -> MarkdownNode {
    let mut prepared = node.clone();
    match &mut prepared {
        MarkdownNode::CodeBlock {
            lang,
            code,
            highlighted_html,
            ..
        } => {
            let lang = lang.as_deref().unwrap_or("plaintext");
            if lang != "mermaid" {
                *highlighted_html = highlighter.start(node_id, code, lang);
            }
        }
        MarkdownNode::Blockquote { children, .. } => {
            for (index, child) in children.iter_mut().enumerate() {
                *child =
                    prepare_stream_node(child, &format!("{}.b{}", node_id, index), highlighter);
            }
        }
        MarkdownNode::List { items, .. } => {
            for (item_index, item) in items.iter_mut().enumerate() {
                for (block_index, child) in item.iter_mut().enumerate() {
                    *child = prepare_stream_node(
                        child,
                        &format!("{}.li{}.b{}", node_id, item_index, block_index),
                        highlighter,
                    );
                }
            }
        }
        _ => {}
    }
    prepared
}

fn prime_stream_code_nodes(
    nodes: &[MarkdownNode],
    prefix: &str,
    highlighter: &mut IncrementalCodeHighlighter,
) {
    for (index, node) in nodes.iter().enumerate() {
        prime_stream_code_node(node, &format!("{prefix}{index}"), highlighter);
    }
}

fn prime_stream_code_node(
    node: &MarkdownNode,
    node_id: &str,
    highlighter: &mut IncrementalCodeHighlighter,
) {
    match node {
        MarkdownNode::CodeBlock { lang, code, .. } => {
            let lang = lang.as_deref().unwrap_or("plaintext");
            if lang != "mermaid" {
                let _ = highlighter.prime(node_id, code, lang);
            }
        }
        MarkdownNode::Blockquote { children, .. } => {
            for (index, child) in children.iter().enumerate() {
                prime_stream_code_node(child, &format!("{}.b{}", node_id, index), highlighter);
            }
        }
        MarkdownNode::List { items, .. } => {
            for (item_index, item) in items.iter().enumerate() {
                for (block_index, child) in item.iter().enumerate() {
                    prime_stream_code_node(
                        child,
                        &format!("{}.li{}.b{}", node_id, item_index, block_index),
                        highlighter,
                    );
                }
            }
        }
        _ => {}
    }
}

fn mark_stream_code_nodes(
    nodes: &[MarkdownNode],
    prefix: &str,
    highlighter: &mut IncrementalCodeHighlighter,
) {
    for (index, node) in nodes.iter().enumerate() {
        mark_stream_code_node(node, &format!("{prefix}{index}"), highlighter);
    }
}

fn mark_stream_code_node(
    node: &MarkdownNode,
    node_id: &str,
    highlighter: &mut IncrementalCodeHighlighter,
) {
    match node {
        MarkdownNode::CodeBlock { lang, .. } => {
            if lang.as_deref() != Some("mermaid") {
                highlighter.mark_seen(node_id);
            }
        }
        MarkdownNode::Blockquote { children, .. } => {
            for (index, child) in children.iter().enumerate() {
                mark_stream_code_node(child, &format!("{}.b{}", node_id, index), highlighter);
            }
        }
        MarkdownNode::List { items, .. } => {
            for (item_index, item) in items.iter().enumerate() {
                for (block_index, child) in item.iter().enumerate() {
                    mark_stream_code_node(
                        child,
                        &format!("{}.li{}.b{}", node_id, item_index, block_index),
                        highlighter,
                    );
                }
            }
        }
        _ => {}
    }
}

pub fn diff_inline_nodes(
    old_list: &[InlineNode],
    new_list: &[InlineNode],
    parent_id: &str,
    prefix: &str,
    mutations: &mut Vec<AstMutation>,
) {
    let common_len = old_list.len().min(new_list.len());

    // 1. 对比公共部分
    for i in 0..common_len {
        let node_id = format!("{}{}", prefix, i);
        let old_node = &old_list[i];
        let new_node = &new_list[i];

        if old_node.get_hash() == new_node.get_hash() && old_node.get_hash().is_some() {
            continue; // Hash 相同，直接跳过
        }

        diff_single_inline_node(old_node, new_node, &node_id, mutations);
    }

    // 2. 新增的尾部节点
    for (i, item) in new_list.iter().enumerate().skip(common_len) {
        let node_id = format!("{}{}", prefix, i);
        mutations.push(AstMutation::AddInline {
            id: node_id,
            parent: parent_id.to_string(),
            node: item.clone(),
        });
    }

    // 3. 删除的尾部节点
    for i in common_len..old_list.len() {
        let node_id = format!("{}{}", prefix, i);
        mutations.push(AstMutation::Remove { id: node_id });
    }
}

fn diff_single_inline_node(
    old_node: &InlineNode,
    new_node: &InlineNode,
    node_id: &str,
    mutations: &mut Vec<AstMutation>,
) {
    if std::mem::discriminant(old_node) != std::mem::discriminant(new_node) {
        mutations.push(AstMutation::ReplaceInline {
            id: node_id.to_string(),
            node: new_node.clone(),
        });
        return;
    }

    match (old_node, new_node) {
        (InlineNode::Text { value: old_val }, InlineNode::Text { value: new_val }) => {
            diff_text_node(node_id, old_val, new_val, mutations);
        }
        (
            InlineNode::Strong {
                children: old_children,
                ..
            },
            InlineNode::Strong {
                children: new_children,
                ..
            },
        ) => {
            diff_inline_nodes(
                old_children,
                new_children,
                node_id,
                &format!("{}.i", node_id),
                mutations,
            );
        }
        (
            InlineNode::Emphasis {
                children: old_children,
                ..
            },
            InlineNode::Emphasis {
                children: new_children,
                ..
            },
        ) => {
            diff_inline_nodes(
                old_children,
                new_children,
                node_id,
                &format!("{}.i", node_id),
                mutations,
            );
        }
        (
            InlineNode::Link {
                href: old_href,
                title: old_title,
                children: old_children,
                ..
            },
            InlineNode::Link {
                href: new_href,
                title: new_title,
                children: new_children,
                ..
            },
        ) => {
            if old_href != new_href || old_title != new_title {
                mutations.push(AstMutation::ReplaceInline {
                    id: node_id.to_string(),
                    node: new_node.clone(),
                });
            } else {
                diff_inline_nodes(
                    old_children,
                    new_children,
                    node_id,
                    &format!("{}.i", node_id),
                    mutations,
                );
            }
        }
        (
            InlineNode::VcpCustom {
                kind: old_kind,
                value: old_value,
                children: old_children,
                ..
            },
            InlineNode::VcpCustom {
                kind: new_kind,
                value: new_value,
                children: new_children,
                ..
            },
        ) => {
            if old_kind != new_kind || old_value != new_value {
                mutations.push(AstMutation::ReplaceInline {
                    id: node_id.to_string(),
                    node: new_node.clone(),
                });
            } else {
                match (old_children, new_children) {
                    (Some(oc), Some(nc)) => {
                        diff_inline_nodes(oc, nc, node_id, &format!("{}.i", node_id), mutations);
                    }
                    (None, None) => {}
                    _ => {
                        mutations.push(AstMutation::ReplaceInline {
                            id: node_id.to_string(),
                            node: new_node.clone(),
                        });
                    }
                }
            }
        }
        (
            InlineNode::Strikethrough {
                children: old_children,
                ..
            },
            InlineNode::Strikethrough {
                children: new_children,
                ..
            },
        ) => {
            diff_inline_nodes(
                old_children,
                new_children,
                node_id,
                &format!("{}.i", node_id),
                mutations,
            );
        }
        // 行内代码（无 hash）：按值比较，值变才更新，避免每帧无谓 ReplaceInline
        (InlineNode::Code { value: old_val }, InlineNode::Code { value: new_val }) => {
            if old_val != new_val {
                mutations.push(AstMutation::ReplaceInline {
                    id: node_id.to_string(),
                    node: new_node.clone(),
                });
            }
        }
        // 硬/软换行：同类型即等价，无字段，直接 no-op，杜绝每帧销毁重建 <br>
        (InlineNode::Break, InlineNode::Break) => {}
        _ => {
            mutations.push(AstMutation::ReplaceInline {
                id: node_id.to_string(),
                node: new_node.clone(),
            });
        }
    }
}

fn diff_text_node(id: &str, old_value: &str, new_value: &str, mutations: &mut Vec<AstMutation>) {
    match new_value.strip_prefix(old_value) {
        Some("") => {}
        Some(chunk) => {
            mutations.push(AstMutation::AppendText {
                id: id.to_string(),
                chunk: chunk.to_string(),
            });
        }
        None => {
            mutations.push(AstMutation::UpdateText {
                id: id.to_string(),
                value: new_value.to_string(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcp_modules::aurora_pipeline::AuroraBuffer;

    struct SimpleRng {
        state: u32,
    }

    impl SimpleRng {
        fn new(seed: u32) -> Self {
            Self { state: seed }
        }
        fn next_range(&mut self, min: usize, max: usize) -> usize {
            self.state = self.state.wrapping_mul(1103515245).wrapping_add(12345);
            let val = (self.state / 65536) % 32768;
            min + (val as usize) % (max - min + 1)
        }
    }

    #[test]
    fn test_diff_append_text() {
        let mut old = vec![MarkdownNode::paragraph(vec![InlineNode::text(
            "Hello".to_string(),
        )])];
        let mut new = vec![MarkdownNode::paragraph(vec![InlineNode::text(
            "Hello World".to_string(),
        )])];

        old[0].compute_hashes_recursively();
        new[0].compute_hashes_recursively();

        let mutations = diff_ast(&old, &new, "t");
        assert_eq!(mutations.len(), 1);
        match &mutations[0] {
            AstMutation::AppendText { id, chunk } => {
                assert_eq!(id, "t0.i0");
                assert_eq!(chunk, " World");
            }
            _ => panic!("Expected AppendText mutation"),
        }
    }

    #[test]
    fn text_diff_scans_once_without_weakening_prefix_validation() {
        let old = "你".repeat(20_000);

        let mut mutations = Vec::new();
        diff_text_node("t0.i0", &old, &old, &mut mutations);
        assert!(mutations.is_empty());

        let appended = format!("{old}🙂");
        diff_text_node("t0.i0", &old, &appended, &mut mutations);
        assert!(matches!(
            mutations.as_slice(),
            [AstMutation::AppendText { chunk, .. }] if chunk == "🙂"
        ));

        mutations.clear();
        let mut changed = old.clone();
        changed.replace_range(changed.len() - "你".len().., "他");
        diff_text_node("t0.i0", &old, &changed, &mut mutations);
        assert!(matches!(
            mutations.as_slice(),
            [AstMutation::UpdateText { value, .. }] if value == &changed
        ));

        mutations.clear();
        diff_text_node("t0.i0", &old, "短", &mut mutations);
        assert!(matches!(
            mutations.as_slice(),
            [AstMutation::UpdateText { value, .. }] if value == "短"
        ));
    }

    #[test]
    fn test_diff_add_node() {
        let mut old = vec![MarkdownNode::paragraph(vec![InlineNode::text(
            "Hello".to_string(),
        )])];
        let mut new = vec![
            MarkdownNode::paragraph(vec![InlineNode::text("Hello".to_string())]),
            MarkdownNode::paragraph(vec![InlineNode::text("World".to_string())]),
        ];

        old[0].compute_hashes_recursively();
        new[0].compute_hashes_recursively();
        new[1].compute_hashes_recursively();

        let mutations = diff_ast(&old, &new, "t");
        assert_eq!(mutations.len(), 1);
        match &mutations[0] {
            AstMutation::Add { id, parent, .. } => {
                assert_eq!(id, "t1");
                assert_eq!(parent, "root");
            }
            _ => panic!("Expected Add mutation"),
        }
    }

    // #4：列表新增项走 item 级别增量 AddListItem，而非整表 Replace
    #[test]
    fn test_diff_list_add_item_incremental() {
        let mk_item = |s: &str| {
            vec![MarkdownNode::paragraph(vec![InlineNode::text(
                s.to_string(),
            )])]
        };

        let mut old = vec![MarkdownNode::list(false, vec![mk_item("A"), mk_item("B")])];
        let mut new = vec![MarkdownNode::list(
            false,
            vec![mk_item("A"), mk_item("B"), mk_item("C")],
        )];
        old[0].compute_hashes_recursively();
        new[0].compute_hashes_recursively();

        let mutations = diff_ast(&old, &new, "t");
        // 期望恰好一条 AddListItem（li2 挂到 t0 下），绝不出现整表 Replace
        assert_eq!(mutations.len(), 1, "got: {:?}", mutations);
        match &mutations[0] {
            AstMutation::AddListItem {
                id,
                parent,
                children,
            } => {
                assert_eq!(id, "t0.li2");
                assert_eq!(parent, "t0");
                assert_eq!(children.len(), 1);
            }
            other => panic!("Expected AddListItem, got {:?}", other),
        }
    }

    // #4：列表删尾项走 Remove，不整表重建
    #[test]
    fn test_diff_list_remove_item_incremental() {
        let mk_item = |s: &str| {
            vec![MarkdownNode::paragraph(vec![InlineNode::text(
                s.to_string(),
            )])]
        };
        let mut old = vec![MarkdownNode::list(
            true,
            vec![mk_item("A"), mk_item("B"), mk_item("C")],
        )];
        let mut new = vec![MarkdownNode::list(true, vec![mk_item("A"), mk_item("B")])];
        old[0].compute_hashes_recursively();
        new[0].compute_hashes_recursively();

        let mutations = diff_ast(&old, &new, "t");
        assert_eq!(mutations.len(), 1, "got: {:?}", mutations);
        match &mutations[0] {
            AstMutation::Remove { id } => assert_eq!(id, "t0.li2"),
            other => panic!("Expected Remove t0.li2, got {:?}", other),
        }
    }

    // #5：行内 Code 值不变时零 mutation；Break 同类型零 mutation（杜绝每帧重建 <br>）
    #[test]
    fn test_diff_inline_code_and_break_no_churn() {
        let mut old = vec![MarkdownNode::paragraph(vec![
            InlineNode::code("x".to_string()),
            InlineNode::r#break(),
        ])];
        let mut new = vec![MarkdownNode::paragraph(vec![
            InlineNode::code("x".to_string()),
            InlineNode::r#break(),
        ])];
        old[0].compute_hashes_recursively();
        new[0].compute_hashes_recursively();

        let mutations = diff_ast(&old, &new, "t");
        assert!(
            mutations.is_empty(),
            "unchanged code+break should produce zero mutations, got: {:?}",
            mutations
        );

        // Code 值变化时才发 ReplaceInline
        let mut new2 = vec![MarkdownNode::paragraph(vec![
            InlineNode::code("y".to_string()),
            InlineNode::r#break(),
        ])];
        new2[0].compute_hashes_recursively();
        let mutations2 = diff_ast(&old, &new2, "t");
        assert_eq!(mutations2.len(), 1, "got: {:?}", mutations2);
        assert!(matches!(
            &mutations2[0],
            AstMutation::ReplaceInline { id, .. } if id == "t0.i0"
        ));
    }

    #[test]
    fn test_real_agent_stream_simulation() {
        // 读取真实的 9.8KB Agent 输出样张文档（编译期内嵌，杜绝运行时路径依赖）
        let text = include_str!("fixtures/测试文档.txt");

        let mut rng = SimpleRng::new(42); // 固定 seed 保证测试具有确定的可复现性
        let mut buffer = AuroraBuffer::new();

        let chars: Vec<char> = text.chars().collect();
        let mut idx = 0;

        let mut total_mutations_count = 0;

        // 模拟 SSE 流：每次随机取 5 到 150 字节的字符片段推送至缓冲区
        while idx < chars.len() {
            let chunk_len = rng.next_range(5, 150);
            let end = (idx + chunk_len).min(chars.len());
            let chunk: String = chars[idx..end].iter().collect();
            idx = end;

            buffer.append_chunk(&chunk);
            let (_stable_changed, _tail_changed) = buffer.process_queue();
            let tail_frame = buffer.take_tail_frame();

            if let Some(frame) = tail_frame {
                total_mutations_count += frame.mutations.len();
                // 确保 frame 成功进行 serde JSON 序列化，验证没有任何序列化死锁或 panic
                let serialized =
                    serde_json::to_string(&frame).expect("Failed to serialize tail frame to JSON");
                assert!(!serialized.is_empty());
            }
        }

        // 终结流并强刷所有沉淀块
        buffer.finalize();

        // 确保整个大文本在流式过程中产生了大量的 diff 更新指令
        assert!(
            total_mutations_count > 50,
            "Total mutations count was too low: {}",
            total_mutations_count
        );
    }
}
