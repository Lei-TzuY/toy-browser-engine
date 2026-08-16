// ============================================================
//  html/parser.rs  —  HTML5 Tree Builder (subset)
// ============================================================
//
//  Consumes tokens from the tokenizer and builds a DOM tree.
//  Uses an open-element stack.  Void/self-closing elements are
//  inserted as leaves.  Mismatched end tags are silently ignored.

use super::tokenizer::{Token, Tokenizer};
use crate::dom::{Node, NodeType};

/// Tags that never have children (HTML5 void elements).
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Block-level starts that close an open `<p>`, per the HTML5 "in body"
/// insertion mode. `<p>one<p>two` is two siblings, not a nested pair.
const CLOSES_PARAGRAPH: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "details",
    "div",
    "dl",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "main",
    "menu",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "ul",
];

/// For a start tag, `(elements it implicitly closes, elements that stop the
/// search)`.
///
/// `<li>` closes an open `<li>` even through inline wrappers, but never
/// escapes its list — so the search walks down the open elements and gives up
/// at a boundary such as `<ul>` or `<table>`.
fn implied_end_tags(start_tag: &str) -> (&'static [&'static str], &'static [&'static str]) {
    const DOCUMENT_EDGE: &[&str] = &["body", "html"];
    match start_tag {
        "li" => (
            &["li"],
            &["ul", "ol", "table", "td", "th", "button", "body", "html"],
        ),
        "dt" | "dd" => (&["dt", "dd"], &["dl", "body", "html"]),
        "tr" => (
            &["td", "th", "tr"],
            &["table", "thead", "tbody", "tfoot", "body", "html"],
        ),
        "td" | "th" => (
            &["td", "th"],
            &["tr", "table", "thead", "tbody", "tfoot", "body", "html"],
        ),
        "thead" | "tbody" | "tfoot" => (
            &["td", "th", "tr", "thead", "tbody", "tfoot"],
            &["table", "body", "html"],
        ),
        "option" => (
            &["option"],
            &["select", "datalist", "optgroup", "body", "html"],
        ),
        "optgroup" => (
            &["option", "optgroup"],
            &["select", "datalist", "body", "html"],
        ),
        _ => (&[], DOCUMENT_EDGE),
    }
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// Stack of open elements; index 0 is always the Document root.
    stack: Vec<Node>,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            stack: vec![Node::document()],
        }
    }

    fn top_mut(&mut self) -> &mut Node {
        self.stack.last_mut().unwrap()
    }

    fn next(&mut self) -> Token {
        let tok = self.tokens.get(self.pos).cloned().unwrap_or(Token::Eof);
        self.pos += 1;
        tok
    }

    /// Tag name of the innermost open element.
    fn current_tag(&self) -> Option<&str> {
        match &self.stack.last()?.node_type {
            NodeType::Element(e) => Some(e.tag_name.as_str()),
            _ => None,
        }
    }

    /// Pop the innermost open element and attach it to its parent.
    fn pop_element(&mut self) {
        if self.stack.len() <= 1 {
            return;
        }
        let closed = self.stack.pop().unwrap();
        self.stack.last_mut().unwrap().children.push(closed);
    }

    /// Apply the implied-end-tag rules for a start tag that is about to open.
    ///
    /// HTML lets authors omit end tags — `<li>a<li>b`, `<td>a<td>b`,
    /// `<p>one<p>two` — and expects the previous element to be closed rather
    /// than nested.
    fn close_implied(&mut self, start_tag: &str) {
        let (closes, boundaries) = implied_end_tags(start_tag);
        if !closes.is_empty() {
            // Walk down the open elements for the outermost one to close,
            // stepping over inline wrappers and stopping at a scope boundary.
            // `<tr>` closes both the open `<td>` and the open `<tr>`.
            let mut target = None;
            for index in (1..self.stack.len()).rev() {
                let NodeType::Element(e) = &self.stack[index].node_type else {
                    break;
                };
                let tag = e.tag_name.as_str();
                if boundaries.contains(&tag) {
                    break;
                }
                if closes.contains(&tag) {
                    target = Some(index);
                }
            }
            if let Some(index) = target {
                while self.stack.len() > index {
                    self.pop_element();
                }
            }
        }

        // A block-level start closes an open paragraph, however deep the
        // inline content inside it went (`<p>a <em>b <div>` closes both).
        if CLOSES_PARAGRAPH.contains(&start_tag) && self.has_open_paragraph() {
            while let Some(open) = self.current_tag() {
                let is_paragraph = open == "p";
                self.pop_element();
                if is_paragraph {
                    break;
                }
            }
        }
    }

    /// True when a `<p>` is open with only inline content between it and the
    /// insertion point — the condition HTML5 calls "p in button scope".
    fn has_open_paragraph(&self) -> bool {
        for node in self.stack.iter().skip(1).rev() {
            let NodeType::Element(e) = &node.node_type else {
                return false;
            };
            match e.tag_name.as_str() {
                "p" => return true,
                // Scope boundaries: a `<p>` outside these is not "in scope".
                "div" | "section" | "article" | "aside" | "blockquote" | "td" | "th" | "li"
                | "ul" | "ol" | "table" | "body" | "html" | "form" | "button" => return false,
                _ => {}
            }
        }
        false
    }

    fn run(mut self) -> Node {
        loop {
            match self.next() {
                Token::Eof => break,

                Token::Doctype { name } => {
                    self.top_mut().children.push(Node::doctype(name));
                }

                Token::Comment(text) => {
                    self.top_mut().children.push(Node::comment(text));
                }

                Token::Character(c) => {
                    // Merge consecutive characters into a single Text node.
                    match self.top_mut().children.last_mut() {
                        Some(Node {
                            node_type: NodeType::Text(ref mut s),
                            ..
                        }) => {
                            s.push(c);
                        }
                        _ => {
                            self.top_mut().children.push(Node::text(c.to_string()));
                        }
                    }
                }

                Token::StartTag {
                    name,
                    attributes,
                    self_closing,
                } => {
                    self.close_implied(&name);

                    let attrs: Vec<(String, String)> =
                        attributes.into_iter().map(|a| (a.name, a.value)).collect();
                    let node = Node::element(name.clone(), attrs);
                    let is_void = VOID_ELEMENTS.contains(&name.as_str()) || self_closing;
                    if is_void {
                        self.top_mut().children.push(node);
                    } else {
                        self.stack.push(node);
                    }
                }

                Token::EndTag { name } => {
                    // Find the topmost open element with this tag name.
                    let idx = self.stack.iter().rposition(
                        |n| matches!(&n.node_type, NodeType::Element(e) if e.tag_name == name),
                    );
                    if let Some(idx) = idx {
                        // Pop down to and including the matched element.
                        while self.stack.len() > idx {
                            let closed = self.stack.pop().unwrap();
                            if let Some(parent) = self.stack.last_mut() {
                                parent.children.push(closed);
                            } else {
                                // Defensive: the stack was already empty — re-push.
                                self.stack.push(closed);
                                break;
                            }
                        }
                    }
                    // Mismatched end tags are ignored.
                }
            }
        }

        // Close any elements still on the stack.
        while self.stack.len() > 1 {
            let closed = self.stack.pop().unwrap();
            if let Some(parent) = self.stack.last_mut() {
                parent.children.push(closed);
            }
        }

        self.stack.pop().unwrap()
    }
}

/// Parse an HTML string and return the root Document node.
pub fn parse_html(input: &str) -> Node {
    let chars: Vec<char> = input.chars().collect();
    let tokens = Tokenizer::new(&chars).tokenize();
    Parser::new(tokens).run()
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::NodeType;

    #[test]
    fn full_document() {
        let doc = parse_html("<!DOCTYPE html><html><head></head><body><p>Hello</p></body></html>");
        assert!(matches!(doc.node_type, NodeType::Document));
        assert!(matches!(&doc.children[0].node_type, NodeType::Doctype(n) if n == "html"));
        let html = &doc.children[1];
        assert!(matches!(&html.node_type, NodeType::Element(e) if e.tag_name == "html"));
        assert_eq!(html.children.len(), 2); // head + body
    }

    #[test]
    fn void_elements_are_leaves() {
        let doc = parse_html("<div><br /><img src='x.png' /></div>");
        let div = &doc.children[0];
        assert_eq!(div.children.len(), 2);
        assert!(div.children.iter().all(|n| n.children.is_empty()));
    }

    #[test]
    fn text_merging() {
        let doc = parse_html("<p>Hello World</p>");
        let p = &doc.children[0];
        assert_eq!(p.children.len(), 1);
        assert!(matches!(&p.children[0].node_type, NodeType::Text(t) if t == "Hello World"));
    }

    #[test]
    fn mixed_inline_children() {
        let doc = parse_html("<p>Hello <b>World</b>!</p>");
        let p = &doc.children[0];
        assert!(matches!(&p.children[0].node_type, NodeType::Text(t) if t == "Hello "));
        assert!(matches!(&p.children[1].node_type, NodeType::Element(e) if e.tag_name == "b"));
        assert!(matches!(&p.children[2].node_type, NodeType::Text(t) if t == "!"));
    }

    #[test]
    fn comment_node() {
        let doc = parse_html("<!-- note --><p>x</p>");
        assert!(matches!(&doc.children[0].node_type, NodeType::Comment(_)));
    }

    // ── Implied end tags ──────────────────────────────────────────────────

    /// Tag names of an element's element children.
    fn child_tags(node: &Node) -> Vec<&str> {
        node.children
            .iter()
            .filter_map(|c| match &c.node_type {
                NodeType::Element(e) => Some(e.tag_name.as_str()),
                _ => None,
            })
            .collect()
    }

    fn text_of(node: &Node) -> String {
        match &node.node_type {
            NodeType::Text(t) => t.clone(),
            _ => node.children.iter().map(text_of).collect(),
        }
    }

    #[test]
    fn list_items_close_each_other() {
        let doc = parse_html("<ul><li>one<li>two<li>three</ul>");
        let ul = &doc.children[0];
        assert_eq!(child_tags(ul), vec!["li", "li", "li"]);
        assert_eq!(text_of(&ul.children[0]), "one");
        assert_eq!(text_of(&ul.children[2]), "three");
    }

    #[test]
    fn list_item_closes_through_inline_wrappers() {
        let doc = parse_html("<ul><li><em>a<li>b</ul>");
        assert_eq!(child_tags(&doc.children[0]), vec!["li", "li"]);
    }

    #[test]
    fn nested_list_is_not_flattened() {
        // The inner <li> belongs to the inner <ul>, not to the outer list.
        let doc = parse_html("<ul><li>outer<ul><li>inner</ul></ul>");
        let outer = &doc.children[0];
        assert_eq!(child_tags(outer), vec!["li"]);
        let outer_item = &outer.children[0];
        assert_eq!(child_tags(outer_item), vec!["ul"]);
        assert_eq!(child_tags(&outer_item.children[1]), vec!["li"]);
    }

    #[test]
    fn paragraphs_close_each_other() {
        let doc = parse_html("<p>one<p>two");
        assert_eq!(child_tags(&doc), vec!["p", "p"]);
        assert_eq!(text_of(&doc.children[0]), "one");
    }

    #[test]
    fn block_start_closes_an_open_paragraph() {
        let doc = parse_html("<p>text<div>block</div>");
        assert_eq!(child_tags(&doc), vec!["p", "div"]);
    }

    #[test]
    fn paragraph_may_contain_inline_elements() {
        let doc = parse_html("<p>a <em>b</em> c</p>");
        assert_eq!(child_tags(&doc), vec!["p"]);
        assert_eq!(child_tags(&doc.children[0]), vec!["em"]);
    }

    #[test]
    fn paragraph_inside_a_list_item_does_not_leak_out() {
        // The <li> is a scope boundary, so the <div> closes only the inner <p>.
        let doc = parse_html("<ul><li><p>a<div>b</div></li></ul>");
        let li = &doc.children[0].children[0];
        assert_eq!(child_tags(li), vec!["p", "div"]);
    }

    #[test]
    fn table_cells_and_rows_close_each_other() {
        let doc = parse_html("<table><tr><td>a<td>b<tr><td>c<td>d</table>");
        let table = &doc.children[0];
        assert_eq!(child_tags(table), vec!["tr", "tr"]);
        for row in &table.children {
            assert_eq!(child_tags(row), vec!["td", "td"]);
        }
        assert_eq!(text_of(&table.children[1].children[1]), "d");
    }

    #[test]
    fn table_sections_close_previous_rows() {
        let doc = parse_html("<table><thead><tr><th>h<tbody><tr><td>d</table>");
        let table = &doc.children[0];
        assert_eq!(child_tags(table), vec!["thead", "tbody"]);
        assert_eq!(child_tags(&table.children[0]), vec!["tr"]);
        assert_eq!(child_tags(&table.children[1]), vec!["tr"]);
    }

    #[test]
    fn definition_lists_close_terms_and_definitions() {
        let doc = parse_html("<dl><dt>term<dd>definition<dt>next</dl>");
        assert_eq!(child_tags(&doc.children[0]), vec!["dt", "dd", "dt"]);
    }

    #[test]
    fn select_options_close_each_other() {
        let doc = parse_html("<select><option>a<option>b</select>");
        assert_eq!(child_tags(&doc.children[0]), vec!["option", "option"]);
    }

    #[test]
    fn explicit_end_tags_still_win() {
        let doc = parse_html("<ul><li>one</li><li>two</li></ul>");
        assert_eq!(child_tags(&doc.children[0]), vec!["li", "li"]);
    }
}
