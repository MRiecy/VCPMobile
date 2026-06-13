use serde::{Serialize, Deserialize};
use crate::vcp_modules::pre_renderer::markdown_ast::{MarkdownNode, InlineNode};

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
    #[serde(rename = "text")]
    UpdateText {
        id: String,
        value: String,
    },
    #[serde(rename = "append")]
    AppendText {
        id: String,
        chunk: String,
    },
    #[serde(rename = "prop")]
    UpdateProp {
        id: String,
        key: String,
        value: String,
    },
    #[serde(rename = "replace")]
    Replace {
        id: String,
        node: MarkdownNode,
    },
    #[serde(rename = "replace_inline")]
    ReplaceInline {
        id: String,
        node: InlineNode,
    },
    #[serde(rename = "remove")]
    Remove {
        id: String,
    },
}

/// 对外暴露的 AST 对比入口
pub fn diff_ast(
    old_nodes: &[MarkdownNode],
    new_nodes: &[MarkdownNode],
    prefix: &str,
) -> Vec<AstMutation> {
    let mut mutations = Vec::new();
    diff_markdown_nodes(old_nodes, new_nodes, "root", prefix, &mut mutations);
    mutations
}

pub fn diff_markdown_nodes(
    old_list: &[MarkdownNode],
    new_list: &[MarkdownNode],
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
            continue; // Hash 命中，相同，直接跳过
        }

        diff_single_markdown_node(old_node, new_node, &node_id, mutations);
    }

    // 2. 新增的尾部节点
    for i in common_len..new_list.len() {
        let node_id = format!("{}{}", prefix, i);
        mutations.push(AstMutation::Add {
            id: node_id,
            parent: parent_id.to_string(),
            node: new_list[i].clone(),
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
) {
    if std::mem::discriminant(old_node) != std::mem::discriminant(new_node) {
        // 类型不同，直接 Replace
        mutations.push(AstMutation::Replace {
            id: node_id.to_string(),
            node: new_node.clone(),
        });
        return;
    }

    match (old_node, new_node) {
        (MarkdownNode::Paragraph { children: old_children, .. }, MarkdownNode::Paragraph { children: new_children, .. }) => {
            diff_inline_nodes(old_children, new_children, node_id, &format!("{}.i", node_id), mutations);
        }
        (MarkdownNode::Heading { level: old_level, children: old_children, .. }, MarkdownNode::Heading { level: new_level, children: new_children, .. }) => {
            if old_level != new_level {
                mutations.push(AstMutation::UpdateProp {
                    id: node_id.to_string(),
                    key: "level".to_string(),
                    value: new_level.to_string(),
                });
            }
            diff_inline_nodes(old_children, new_children, node_id, &format!("{}.i", node_id), mutations);
        }
        (MarkdownNode::Blockquote { children: old_children, .. }, MarkdownNode::Blockquote { children: new_children, .. }) => {
            diff_markdown_nodes(old_children, new_children, node_id, &format!("{}.b", node_id), mutations);
        }
        (MarkdownNode::List { ordered: old_ordered, items: old_items, .. }, MarkdownNode::List { ordered: new_ordered, items: new_items, .. }) => {
            if old_ordered != new_ordered {
                mutations.push(AstMutation::Replace {
                    id: node_id.to_string(),
                    node: new_node.clone(),
                });
            } else {
                let common_len = old_items.len().min(new_items.len());
                for i in 0..common_len {
                    let item_prefix = format!("{}.li{}", node_id, i);
                    diff_markdown_nodes(
                        &old_items[i],
                        &new_items[i],
                        &item_prefix,
                        &format!("{}.b", item_prefix),
                        mutations,
                    );
                }

                if old_items.len() != new_items.len() {
                    mutations.push(AstMutation::Replace {
                        id: node_id.to_string(),
                        node: new_node.clone(),
                    });
                }
            }
        }
        // Table, RawHtml, MermaidPlaceholder, CodeBlock, ThematicBreak 变化时直接 Replace 整个节点
        _ => {
            mutations.push(AstMutation::Replace {
                id: node_id.to_string(),
                node: new_node.clone(),
            });
        }
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
    for i in common_len..new_list.len() {
        let node_id = format!("{}{}", prefix, i);
        mutations.push(AstMutation::AddInline {
            id: node_id,
            parent: parent_id.to_string(),
            node: new_list[i].clone(),
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
        (InlineNode::Strong { children: old_children, .. }, InlineNode::Strong { children: new_children, .. }) => {
            diff_inline_nodes(old_children, new_children, node_id, &format!("{}.i", node_id), mutations);
        }
        (InlineNode::Emphasis { children: old_children, .. }, InlineNode::Emphasis { children: new_children, .. }) => {
            diff_inline_nodes(old_children, new_children, node_id, &format!("{}.i", node_id), mutations);
        }
        (InlineNode::Link { href: old_href, title: old_title, children: old_children, .. }, InlineNode::Link { href: new_href, title: new_title, children: new_children, .. }) => {
            if old_href != new_href || old_title != new_title {
                mutations.push(AstMutation::ReplaceInline {
                    id: node_id.to_string(),
                    node: new_node.clone(),
                });
            } else {
                diff_inline_nodes(old_children, new_children, node_id, &format!("{}.i", node_id), mutations);
            }
        }
        (InlineNode::QuotedText { children: old_children, .. }, InlineNode::QuotedText { children: new_children, .. }) => {
            diff_inline_nodes(old_children, new_children, node_id, &format!("{}.i", node_id), mutations);
        }
        (InlineNode::Strikethrough { children: old_children, .. }, InlineNode::Strikethrough { children: new_children, .. }) => {
            diff_inline_nodes(old_children, new_children, node_id, &format!("{}.i", node_id), mutations);
        }
        _ => {
            mutations.push(AstMutation::ReplaceInline {
                id: node_id.to_string(),
                node: new_node.clone(),
            });
        }
    }
}

fn diff_text_node(
    id: &str,
    old_value: &str,
    new_value: &str,
    mutations: &mut Vec<AstMutation>,
) {
    if new_value == old_value {
        return;
    }

    if new_value.starts_with(old_value) {
        let chunk = &new_value[old_value.len()..];
        if !chunk.is_empty() {
            mutations.push(AstMutation::AppendText {
                id: id.to_string(),
                chunk: chunk.to_string(),
            });
        }
    } else {
        mutations.push(AstMutation::UpdateText {
            id: id.to_string(),
            value: new_value.to_string(),
        });
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
        let mut old = vec![MarkdownNode::paragraph(vec![InlineNode::text("Hello".to_string())])];
        let mut new = vec![MarkdownNode::paragraph(vec![InlineNode::text("Hello World".to_string())])];

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
    fn test_diff_add_node() {
        let mut old = vec![MarkdownNode::paragraph(vec![InlineNode::text("Hello".to_string())])];
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

    #[test]
    fn test_real_agent_stream_simulation() {
        // 读取真实的 9.8KB Agent 输出样张文档
        let text = std::fs::read_to_string("../scripts/tail-test/测试文档.txt")
            .unwrap_or_else(|_| {
                std::fs::read_to_string("scripts/tail-test/测试文档.txt")
                    .expect("Failed to find or read 测试文档.txt in scripts/tail-test")
            });

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
            let (_stable_changed, _tail_changed, tail_mutations) = buffer.process_queue();

            if let Some(mutations) = tail_mutations {
                total_mutations_count += mutations.len();
                // 确保 mutations 成功进行 serde JSON 序列化，验证没有任何序列化死锁或 panic
                let serialized = serde_json::to_string(&mutations)
                    .expect("Failed to serialize tail mutations to JSON");
                assert!(!serialized.is_empty());
            }
        }

        // 终结流并强刷所有沉淀块
        buffer.finalize();

        // 确保整个大文本在流式过程中产生了大量的 diff 更新指令
        assert!(total_mutations_count > 50, "Total mutations count was too low: {}", total_mutations_count);
    }
}
