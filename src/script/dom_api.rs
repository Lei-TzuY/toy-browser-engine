// ============================================================
//  script/dom_api.rs  —  DOM plumbing for the script engine
// ============================================================
//
//  The DOM is a tree of owned `Node`s, so scripts refer to elements
//  by *path*: the sequence of child indices from the document root.
//  A path is stable as long as the surrounding tree is not
//  restructured, which is enough for the mutations scripts perform
//  here (text, attributes, appending and removing nodes).
//
//  This module also provides the selector engine used by
//  `querySelector` / `querySelectorAll`, plus helpers for inline
//  styles and class lists.

use crate::css::parser::{parse_css, Combinator, Selector, SelectorPart};
use crate::dom::{ElementData, ElementId, Node, NodeType};

/// Sequence of child indices from the document root to a node.
pub type NodePath = Vec<usize>;

// ── Path navigation ───────────────────────────────────────────────────────────

pub fn node_at<'a>(root: &'a Node, path: &[usize]) -> Option<&'a Node> {
    let mut cur = root;
    for &i in path {
        cur = cur.children.get(i)?;
    }
    Some(cur)
}

pub fn node_at_mut<'a>(root: &'a mut Node, path: &[usize]) -> Option<&'a mut Node> {
    let mut cur = root;
    for &i in path {
        cur = cur.children.get_mut(i)?;
    }
    Some(cur)
}

/// Find the path of `target` inside `root` by node identity.
///
/// Used to translate a hit-test result (a `&Node`) back into a handle the
/// script engine can address.
pub fn path_of(root: &Node, target: &Node) -> Option<NodePath> {
    fn walk(node: &Node, target: &Node, path: &mut NodePath) -> bool {
        if std::ptr::eq(node, target) {
            return true;
        }
        for (i, child) in node.children.iter().enumerate() {
            path.push(i);
            if walk(child, target, path) {
                return true;
            }
            path.pop();
        }
        false
    }
    let mut path = Vec::new();
    if walk(root, target, &mut path) {
        Some(path)
    } else {
        None
    }
}

/// Every path from `path` up to (and including) the document root, nearest first.
pub fn ancestor_paths(path: &[usize]) -> Vec<NodePath> {
    (0..=path.len()).rev().map(|n| path[..n].to_vec()).collect()
}

// ── Text content ──────────────────────────────────────────────────────────────

/// Concatenated text of a node and its descendants (`textContent`).
pub fn text_content(node: &Node) -> String {
    match &node.node_type {
        NodeType::Text(t) => t.clone(),
        _ => node.children.iter().map(text_content).collect(),
    }
}

/// Replace all children with a single text node.
pub fn set_text_content(node: &mut Node, text: &str) {
    node.children.clear();
    if !text.is_empty() {
        node.children.push(Node::text(text.to_string()));
    }
}

/// Serialize a node and its descendants back to HTML.
pub fn outer_html(node: &Node) -> String {
    match &node.node_type {
        NodeType::Text(t) => t.clone(),
        NodeType::Comment(c) => format!("<!--{}-->", c),
        NodeType::Doctype(d) => format!("<!DOCTYPE {}>", d),
        NodeType::Document => node.children.iter().map(outer_html).collect(),
        NodeType::Element(e) => {
            let mut s = format!("<{}", e.tag_name);
            for (k, v) in &e.attributes {
                s.push_str(&format!(" {}=\"{}\"", k, v));
            }
            s.push('>');
            s.push_str(&inner_html(node));
            s.push_str(&format!("</{}>", e.tag_name));
            s
        }
    }
}

pub fn inner_html(node: &Node) -> String {
    node.children.iter().map(outer_html).collect()
}

/// Parse an HTML fragment into a list of nodes (the children of the parse root).
pub fn parse_fragment(html: &str) -> Vec<Node> {
    crate::html::parse_html(html).children
}

// ── Inline style attribute ────────────────────────────────────────────────────

/// Read one property out of an element's inline `style` attribute.
pub fn get_style_property(element: &ElementData, prop: &str) -> Option<String> {
    let style = element.get_attr("style")?;
    for decl in style.split(';') {
        let (name, value) = decl.split_once(':')?;
        if name.trim().eq_ignore_ascii_case(prop) {
            return Some(value.trim().to_string());
        }
    }
    None
}

/// Set one property in an element's inline `style` attribute, preserving the
/// other declarations. An empty value removes the property.
pub fn set_style_property(element: &mut ElementData, prop: &str, value: &str) {
    let existing = element.get_attr("style").unwrap_or("").to_string();
    let mut decls: Vec<(String, String)> = Vec::new();
    let mut replaced = false;

    for decl in existing.split(';') {
        let decl = decl.trim();
        if decl.is_empty() {
            continue;
        }
        let Some((name, val)) = decl.split_once(':') else {
            continue;
        };
        let (name, val) = (name.trim(), val.trim());
        if name.eq_ignore_ascii_case(prop) {
            if !value.is_empty() {
                decls.push((prop.to_string(), value.to_string()));
            }
            replaced = true;
        } else {
            decls.push((name.to_string(), val.to_string()));
        }
    }
    if !replaced && !value.is_empty() {
        decls.push((prop.to_string(), value.to_string()));
    }

    let serialized = decls
        .iter()
        .map(|(k, v)| format!("{}: {}", k, v))
        .collect::<Vec<_>>()
        .join("; ");
    element.set_attr("style", &serialized);
}

/// Convert a JS style property name (`backgroundColor`) to CSS (`background-color`).
pub fn css_property_name(js_name: &str) -> String {
    let mut out = String::with_capacity(js_name.len() + 2);
    for c in js_name.chars() {
        if c.is_ascii_uppercase() {
            out.push('-');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

// ── Class list ────────────────────────────────────────────────────────────────

pub fn class_list(element: &ElementData) -> Vec<String> {
    element
        .get_attr("class")
        .map(|c| c.split_whitespace().map(String::from).collect())
        .unwrap_or_default()
}

pub fn set_class_list(element: &mut ElementData, classes: &[String]) {
    element.set_attr("class", &classes.join(" "));
}

// ── Selector engine ───────────────────────────────────────────────────────────

/// Parse a selector list (`"div.card, #main p"`) by round-tripping it through
/// the CSS parser, so `querySelector` and the cascade agree on syntax.
pub fn parse_selector_list(selector: &str) -> Vec<Selector> {
    let sheet = parse_css(&format!("{} {{}}", selector));
    sheet
        .rules
        .into_iter()
        .next()
        .map(|r| r.selectors)
        .unwrap_or_default()
}

/// Match a single compound part (tag / #id / .class / [attr]) against an element.
///
/// Pseudo-classes are ignored here: a static query has no interaction state,
/// and the cascade in `style::` remains the authority on those.
fn part_matches(element: &ElementData, part: &SelectorPart) -> bool {
    if let Some(tag) = &part.tag_name {
        if tag != "*" && element.tag_name != *tag {
            return false;
        }
    }
    if let Some(id) = &part.id {
        if element.get_attr("id") != Some(id.as_str()) {
            return false;
        }
    }
    if !part.classes.is_empty() {
        let classes = class_list(element);
        if !part.classes.iter().all(|c| classes.contains(c)) {
            return false;
        }
    }
    for (attr, expected) in &part.attributes {
        match element.get_attr(attr) {
            Some(actual) => {
                if let Some(want) = expected {
                    if actual != want {
                        return false;
                    }
                }
            }
            None => return false,
        }
    }
    true
}

/// Element ancestors of `path`, nearest first.
fn ancestor_elements<'a>(root: &'a Node, path: &[usize]) -> Vec<&'a ElementData> {
    (0..path.len())
        .rev()
        .filter_map(|n| node_at(root, &path[..n]))
        .filter_map(|n| n.as_element())
        .collect()
}

/// Preceding element siblings of `path`, in document order.
fn preceding_siblings<'a>(root: &'a Node, path: &[usize]) -> Vec<&'a ElementData> {
    let Some((&index, parent_path)) = path.split_last() else {
        return Vec::new();
    };
    let Some(parent) = node_at(root, parent_path) else {
        return Vec::new();
    };
    parent.children[..index]
        .iter()
        .filter_map(|n| n.as_element())
        .collect()
}

fn selector_matches_path(root: &Node, path: &[usize], selector: &Selector) -> bool {
    let Some(element) = node_at(root, path).and_then(|n| n.as_element()) else {
        return false;
    };
    let Some(subject) = selector.parts.last() else {
        return false;
    };
    if !part_matches(element, subject) {
        return false;
    }
    if selector.parts.len() == 1 {
        return true;
    }

    let ancestors = ancestor_elements(root, path);
    let mut siblings = preceding_siblings(root, path);
    let mut cursor = 0usize;

    for idx in (0..selector.parts.len() - 1).rev() {
        let part = &selector.parts[idx];
        match selector.parts[idx + 1].combinator {
            Combinator::Root => break,
            Combinator::Child => {
                let Some(parent) = ancestors.get(cursor) else {
                    return false;
                };
                if !part_matches(parent, part) {
                    return false;
                }
                cursor += 1;
                siblings.clear();
            }
            Combinator::Descendant => {
                match ancestors[cursor..]
                    .iter()
                    .position(|a| part_matches(a, part))
                {
                    Some(offset) => {
                        cursor += offset + 1;
                        siblings.clear();
                    }
                    None => return false,
                }
            }
            Combinator::AdjacentSibling => match siblings.pop() {
                Some(prev) if part_matches(prev, part) => {}
                _ => return false,
            },
            Combinator::GeneralSibling => {
                match siblings.iter().rposition(|s| part_matches(s, part)) {
                    Some(i) => siblings.truncate(i),
                    None => return false,
                }
            }
        }
    }
    true
}

/// All descendant paths of `scope` matching `selector`, in document order.
pub fn query_selector_all(root: &Node, scope: &[usize], selector: &str) -> Vec<NodePath> {
    let selectors = parse_selector_list(selector);
    if selectors.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let Some(scope_node) = node_at(root, scope) else {
        return out;
    };

    fn walk(
        root: &Node,
        node: &Node,
        path: &mut NodePath,
        selectors: &[Selector],
        skip_self: bool,
        out: &mut Vec<NodePath>,
    ) {
        if !skip_self
            && node.as_element().is_some()
            && selectors
                .iter()
                .any(|s| selector_matches_path(root, path, s))
        {
            out.push(path.clone());
        }
        for (i, child) in node.children.iter().enumerate() {
            path.push(i);
            walk(root, child, path, selectors, false, out);
            path.pop();
        }
    }

    let mut path = scope.to_vec();
    walk(root, scope_node, &mut path, &selectors, true, &mut out);
    out
}

pub fn query_selector(root: &Node, scope: &[usize], selector: &str) -> Option<NodePath> {
    query_selector_all(root, scope, selector).into_iter().next()
}

/// Tests whether the element at `path` matches `selector`.
pub fn element_matches(root: &Node, path: &[usize], selector: &str) -> bool {
    let selectors = parse_selector_list(selector);
    if selectors.is_empty() {
        return false;
    }
    selectors.iter().any(|s| selector_matches_path(root, path, s))
}

/// Finds the closest ancestor element (starting with `path`) matching `selector`.
pub fn element_closest(root: &Node, path: &[usize], selector: &str) -> Option<NodePath> {
    let selectors = parse_selector_list(selector);
    if selectors.is_empty() {
        return None;
    }
    for n in (0..=path.len()).rev() {
        let candidate = &path[..n];
        if node_at(root, candidate).and_then(|node| node.as_element()).is_some() {
            if selectors.iter().any(|s| selector_matches_path(root, candidate, s)) {
                return Some(candidate.to_vec());
            }
        }
    }
    None
}

/// Clones a node, optionally performing a deep clone with all child subtrees.
pub fn clone_node(node: &Node, deep: bool) -> Node {
    let mut cloned = node.clone();
    if !deep {
        cloned.children.clear();
    }
    cloned
}

/// Converts a camelCase identifier (`userId`) to kebab-case (`user-id`).
pub fn camel_to_kebab(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_uppercase() {
            out.push('-');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Converts a kebab-case identifier (`user-id`) to camelCase (`userId`).
pub fn kebab_to_camel(s: &str) -> String {
    let mut out = String::new();
    let mut capitalize_next = false;
    for c in s.chars() {
        if c == '-' {
            capitalize_next = true;
        } else if capitalize_next {
            out.push(c.to_ascii_uppercase());
            capitalize_next = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Path of the first element whose `id` attribute equals `id`.
/// Path of the element with a given stable identity.
///
/// Unlike a `NodePath`, an [`ElementId`] survives tree mutations, so focus and
/// form state are keyed by it and resolved back to a path when needed.
pub fn path_of_element_id(root: &Node, target: ElementId) -> Option<NodePath> {
    fn walk(node: &Node, target: ElementId, path: &mut NodePath) -> bool {
        if node.as_element().is_some_and(|e| e.element_id() == target) {
            return true;
        }
        for (index, child) in node.children.iter().enumerate() {
            path.push(index);
            if walk(child, target, path) {
                return true;
            }
            path.pop();
        }
        false
    }
    let mut path = Vec::new();
    walk(root, target, &mut path).then_some(path)
}

pub fn get_element_by_id(root: &Node, id: &str) -> Option<NodePath> {
    fn walk(node: &Node, id: &str, path: &mut NodePath) -> bool {
        if let Some(e) = node.as_element() {
            if e.get_attr("id") == Some(id) {
                return true;
            }
        }
        for (i, child) in node.children.iter().enumerate() {
            path.push(i);
            if walk(child, id, path) {
                return true;
            }
            path.pop();
        }
        false
    }
    let mut path = Vec::new();
    if walk(root, id, &mut path) {
        Some(path)
    } else {
        None
    }
}

/// Path of the document's `<body>` element, if there is one.
pub fn body_path(root: &Node) -> Option<NodePath> {
    query_selector(root, &[], "body")
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::parse_html;

    #[test]
    fn resolves_paths_and_identity() {
        let dom = parse_html("<div><p>a</p><p id='x'>b</p></div>");
        let path = get_element_by_id(&dom, "x").expect("id lookup");
        let node = node_at(&dom, &path).unwrap();
        assert_eq!(text_content(node), "b");
        assert_eq!(path_of(&dom, node), Some(path));
    }

    #[test]
    fn query_selector_supports_compound_and_descendant() {
        let dom =
            parse_html("<div class='card'><p>a</p></div><section><p class='hit'>b</p></section>");
        let path = query_selector(&dom, &[], "section p.hit").expect("descendant match");
        assert_eq!(text_content(node_at(&dom, &path).unwrap()), "b");
        assert!(query_selector(&dom, &[], "div p.hit").is_none());
    }

    #[test]
    fn query_selector_all_returns_document_order() {
        let dom = parse_html("<ul><li>1</li><li>2</li><li>3</li></ul>");
        let paths = query_selector_all(&dom, &[], "li");
        let texts: Vec<String> = paths
            .iter()
            .map(|p| text_content(node_at(&dom, p).unwrap()))
            .collect();
        assert_eq!(texts, vec!["1", "2", "3"]);
    }

    #[test]
    fn query_selector_scope_excludes_the_scope_element() {
        let dom = parse_html("<div id='a'><div id='b'></div></div>");
        let outer = get_element_by_id(&dom, "a").unwrap();
        let found = query_selector_all(&dom, &outer, "div");
        assert_eq!(found.len(), 1);
        assert_eq!(
            node_at(&dom, &found[0])
                .unwrap()
                .as_element()
                .unwrap()
                .get_attr("id"),
            Some("b")
        );
    }

    #[test]
    fn sibling_combinators_match() {
        let dom = parse_html("<div><h1>h</h1><p>one</p><p>two</p></div>");
        let adjacent = query_selector_all(&dom, &[], "h1 + p");
        assert_eq!(adjacent.len(), 1);
        let general = query_selector_all(&dom, &[], "h1 ~ p");
        assert_eq!(general.len(), 2);
    }

    #[test]
    fn style_property_round_trip() {
        let mut e = ElementData::new("div", vec![]);
        set_style_property(&mut e, "color", "red");
        set_style_property(&mut e, "background-color", "blue");
        assert_eq!(get_style_property(&e, "color").as_deref(), Some("red"));

        set_style_property(&mut e, "color", "green");
        assert_eq!(get_style_property(&e, "color").as_deref(), Some("green"));
        // Updating one property must not drop the others.
        assert_eq!(
            get_style_property(&e, "background-color").as_deref(),
            Some("blue")
        );

        set_style_property(&mut e, "color", "");
        assert_eq!(get_style_property(&e, "color"), None);
    }

    #[test]
    fn camel_case_style_names_become_css_names() {
        assert_eq!(css_property_name("backgroundColor"), "background-color");
        assert_eq!(css_property_name("color"), "color");
    }

    #[test]
    fn serializes_inner_html() {
        let dom = parse_html("<div><b>hi</b> there</div>");
        let path = query_selector(&dom, &[], "div").unwrap();
        assert_eq!(inner_html(node_at(&dom, &path).unwrap()), "<b>hi</b> there");
    }
}
