pub mod parser;
pub mod tokenizer;

pub use parser::parse_html;

use crate::dom::{Node, NodeType};

/// Walk the DOM and collect the text content of all `<style>` elements.
pub fn extract_inline_styles(node: &Node) -> String {
    let mut css = String::new();
    collect_styles(node, &mut css);
    css
}

fn collect_styles(node: &Node, out: &mut String) {
    if let NodeType::Element(e) = &node.node_type {
        if e.tag_name == "style" {
            for child in &node.children {
                if let NodeType::Text(text) = &child.node_type {
                    out.push_str(text);
                    out.push('\n');
                }
            }
            return; // don't recurse into <style> children
        }
    }
    for child in &node.children {
        collect_styles(child, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::parse_html;

    #[test]
    fn extracts_style_block() {
        let dom = parse_html(
            r#"<html><head><style>p { color: red; }</style></head><body></body></html>"#,
        );
        let css = extract_inline_styles(&dom);
        assert!(css.contains("color: red"));
    }

    #[test]
    fn extracts_multiple_style_blocks() {
        let dom = parse_html(r#"<style>a{}</style><style>b{}</style>"#);
        let css = extract_inline_styles(&dom);
        assert!(css.contains("a{}"));
        assert!(css.contains("b{}"));
    }
}
