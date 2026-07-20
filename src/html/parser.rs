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
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link",
    "meta", "param", "source", "track", "wbr",
];

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
                        Some(Node { node_type: NodeType::Text(ref mut s), .. }) => {
                            s.push(c);
                        }
                        _ => {
                            self.top_mut().children.push(Node::text(c.to_string()));
                        }
                    }
                }

                Token::StartTag { name, attributes, self_closing } => {
                    let attrs: Vec<(String, String)> = attributes
                        .into_iter()
                        .map(|a| (a.name, a.value))
                        .collect();
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
                    let idx = self.stack.iter().rposition(|n| {
                        matches!(&n.node_type, NodeType::Element(e) if e.tag_name == name)
                    });
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
        let doc = parse_html(
            "<!DOCTYPE html><html><head></head><body><p>Hello</p></body></html>",
        );
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
}
