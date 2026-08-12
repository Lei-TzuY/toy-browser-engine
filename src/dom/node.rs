// ============================================================
//  dom/node.rs  —  DOM tree types
// ============================================================
//
//  A stripped-down but faithful representation of the W3C DOM.
//
//  Key design decisions:
//   • Children are stored as owned Vec<Node> (no arena, no Rc<RefCell<>>)
//     This keeps the API simple for Phase 1; an arena can be swapped in
//     later without changing public types.
//   • Attributes are stored as a plain Vec to preserve document order,
//     with an O(n) lookup helper (fine for small attribute lists).
//   • `NodeType` carries the payload so we avoid heap-boxing common cases.

use std::fmt;

/// The type-specific payload of a DOM node.
#[derive(Debug, Clone)]
pub enum NodeType {
    /// The invisible root of the document tree.
    Document,
    /// A tagged element: `<div class="foo">`.
    Element(ElementData),
    /// A text run between tags.
    Text(String),
    /// An HTML comment: `<!-- ... -->`.
    Comment(String),
    /// A DOCTYPE declaration: `<!DOCTYPE html>`.
    Doctype(String),
}

/// Data stored on an element node.
#[derive(Debug, Clone)]
pub struct ElementData {
    /// Lower-cased tag name, e.g. `"div"`.
    pub tag_name: String,
    /// Ordered list of (name, value) attribute pairs.
    pub attributes: Vec<(String, String)>,
}

impl ElementData {
    pub fn new(tag_name: impl Into<String>, attributes: Vec<(String, String)>) -> Self {
        Self {
            tag_name: tag_name.into(),
            attributes,
        }
    }

    /// Look up an attribute value by name (case-insensitive).
    pub fn get_attr(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Set or update an attribute value by name.
    pub fn set_attr(&mut self, name: &str, val: &str) {
        if let Some((_, v)) = self.attributes.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(name)) {
            *v = val.to_string();
        } else {
            self.attributes.push((name.to_string(), val.to_string()));
        }
    }
}

/// A single node in the DOM tree.
#[derive(Debug, Clone)]
pub struct Node {
    pub node_type: NodeType,
    pub children: Vec<Node>,
}

impl Node {
    // ── Constructors ────────────────────────────────────────────────────────

    pub fn document() -> Self {
        Self::leaf(NodeType::Document)
    }

    pub fn element(tag_name: impl Into<String>, attrs: Vec<(String, String)>) -> Self {
        Self::leaf(NodeType::Element(ElementData::new(tag_name, attrs)))
    }

    pub fn text(data: impl Into<String>) -> Self {
        Self::leaf(NodeType::Text(data.into()))
    }

    pub fn comment(data: impl Into<String>) -> Self {
        Self::leaf(NodeType::Comment(data.into()))
    }

    pub fn doctype(name: impl Into<String>) -> Self {
        Self::leaf(NodeType::Doctype(name.into()))
    }

    fn leaf(node_type: NodeType) -> Self {
        Self {
            node_type,
            children: Vec::new(),
        }
    }

    // ── Accessors ───────────────────────────────────────────────────────────

    /// Returns `Some(&ElementData)` if this is an Element node.
    pub fn as_element(&self) -> Option<&ElementData> {
        match &self.node_type {
            NodeType::Element(e) => Some(e),
            _ => None,
        }
    }

    // ── Pretty-printer ──────────────────────────────────────────────────────

    /// Write an indented ASCII tree to `out`.
    ///
    /// Example:
    /// ```text
    /// Document
    /// └── <!DOCTYPE html>
    /// └── html
    ///     ├── head
    ///     │   └── title
    ///     │       └── #text "Hello"
    ///     └── body
    ///         └── p [class="intro"]
    ///             └── #text "World"
    /// ```
    pub fn pretty_print(&self) {
        self.print_recursive("", true, true);
    }

    fn print_recursive(&self, prefix: &str, is_last: bool, is_root: bool) {
        let connector = if is_root {
            ""
        } else if is_last {
            "└── "
        } else {
            "├── "
        };

        let label = self.label();
        println!("{}{}{}", prefix, connector, label);

        let child_prefix = if is_root {
            prefix.to_string()
        } else if is_last {
            format!("{}    ", prefix)
        } else {
            format!("{}│   ", prefix)
        };

        let n = self.children.len();
        for (i, child) in self.children.iter().enumerate() {
            child.print_recursive(&child_prefix, i == n - 1, false);
        }
    }

    /// Short human-readable label for a node.
    fn label(&self) -> String {
        match &self.node_type {
            NodeType::Document => "Document".to_string(),
            NodeType::Doctype(name) => format!("<!DOCTYPE {}>", name),
            NodeType::Element(e) => {
                let mut s = e.tag_name.clone();
                for (k, v) in &e.attributes {
                    s.push_str(&format!(" [{}=\"{}\"]", k, v));
                }
                s
            }
            NodeType::Text(t) => {
                let trimmed = t.trim();
                if trimmed.is_empty() {
                    "#text (whitespace)".to_string()
                } else {
                    format!("#text {:?}", trimmed)
                }
            }
            NodeType::Comment(c) => format!("<!-- {} -->", c.trim()),
        }
    }
}

impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_data_get_attr() {
        let e = ElementData::new("a", vec![("href".into(), "https://example.com".into())]);
        assert_eq!(e.get_attr("href"), Some("https://example.com"));
        assert_eq!(e.get_attr("HREF"), Some("https://example.com"));
        assert_eq!(e.get_attr("class"), None);
    }

    #[test]
    fn node_label_element() {
        let n = Node::element("div", vec![("id".into(), "main".into())]);
        assert_eq!(n.label(), r#"div [id="main"]"#);
    }

    #[test]
    fn node_label_text() {
        let n = Node::text("  hello  ");
        assert_eq!(n.label(), r#"#text "hello""#);
    }

    #[test]
    fn node_label_whitespace_text() {
        let n = Node::text("   \n  ");
        assert_eq!(n.label(), "#text (whitespace)");
    }
}
