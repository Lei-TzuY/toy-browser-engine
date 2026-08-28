// ============================================================
//  style/mod.rs  —  CSS cascade & style tree
// ============================================================
//
//  Features:
//   • Simple selectors (tag, #id, .class)
//   • Pseudo-classes: :first-child, :last-child, :nth-child(),
//     :nth-of-type(), :not(), :only-child, :root, :empty, etc.
//   • Descendant ( ) and child (>) combinators
//   • CSS custom properties (--var) with var() resolution
//   • @media (min-width / max-width) filtering by viewport width
//   • CSS property inheritance for 16+ inheritable properties

use std::collections::HashMap;

use crate::css::parser::{
    parse_declaration_block, parse_single_value, Combinator, Declaration, PseudoClass, Selector,
    SelectorPart, Stylesheet, Unit, Value,
};
use crate::dom::{ElementData, Node, NodeType};

pub type PropertyMap = HashMap<String, Value>;

/// A rule that matched an element: its winning selector's specificity
/// (id, class, tag) and the declarations it contributes.
type MatchedRule<'a> = ((usize, usize, usize), &'a [Declaration]);

// ── Inheritable CSS properties ────────────────────────────────────────────────

const INHERITED: &[&str] = &[
    "color",
    "font-size",
    "font-family",
    "font-weight",
    "font-style",
    "font-variant",
    "line-height",
    "text-align",
    "text-indent",
    "text-transform",
    "letter-spacing",
    "word-spacing",
    "white-space",
    "visibility",
    "cursor",
    "list-style-type",
    "list-style-position",
];

// ── StyledNode ────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct StyledNode<'a> {
    pub node: &'a Node,
    pub specified_values: PropertyMap,
    pub children: Vec<StyledNode<'a>>,
}

impl<'a> StyledNode<'a> {
    pub fn find_at_path(&self, path: &[usize]) -> Option<&StyledNode<'a>> {
        let mut cur = self;
        for &idx in path {
            cur = cur.children.get(idx)?;
        }
        Some(cur)
    }

    pub fn value(&self, name: &str) -> Option<&Value> {
        self.specified_values.get(name)
    }

    pub fn lookup(&self, name: &str, fallback: &str, default: &Value) -> Value {
        self.value(name)
            .or_else(|| self.value(fallback))
            .unwrap_or(default)
            .clone()
    }

    pub fn display(&self) -> Display {
        match self.value("display") {
            Some(Value::Keyword(s)) => match s.as_str() {
                "block" => Display::Block,
                "flex" => Display::Flex,
                "grid" => Display::Grid,
                "table" => Display::Table,
                "table-row" => Display::TableRow,
                "table-cell" => Display::TableCell,
                "inline-block" => Display::InlineBlock,
                "none" => Display::None,
                _ => Display::Inline,
            },
            None => default_display(&self.node.node_type),
            _ => Display::Inline,
        }
    }

    pub fn position(&self) -> Position {
        match self.value("position") {
            Some(Value::Keyword(s)) => match s.as_str() {
                "relative" => Position::Relative,
                "absolute" => Position::Absolute,
                "fixed" => Position::Fixed,
                "sticky" => Position::Sticky,
                _ => Position::Static,
            },
            _ => Position::Static,
        }
    }

    pub fn z_index(&self) -> Option<i32> {
        match self.value("z-index") {
            Some(Value::Keyword(s)) if s == "auto" => None,
            Some(Value::Keyword(s)) => s.parse().ok(),
            Some(Value::Length(n, Unit::Px)) if n.is_finite() && n.fract() == 0.0 => {
                Some(*n as i32)
            }
            Some(Value::Number(n)) if n.is_finite() => Some(n.round() as i32),
            _ => None,
        }
    }

    pub fn establishes_stacking_context(&self) -> bool {
        self.position() != Position::Static && self.z_index().is_some()
    }

    pub fn overflow(&self) -> Overflow {
        match self.value("overflow") {
            Some(Value::Keyword(s)) => match s.as_str() {
                "hidden" => Overflow::Hidden,
                "scroll" => Overflow::Scroll,
                "auto" => Overflow::Auto,
                _ => Overflow::Visible,
            },
            _ => Overflow::Visible,
        }
    }
}

fn default_display(node_type: &NodeType) -> Display {
    match node_type {
        NodeType::Document => Display::Block,
        NodeType::Element(e) => match e.tag_name.as_str() {
            "html" | "body" | "div" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul"
            | "ol" | "li" | "blockquote" | "pre" | "header" | "footer" | "section" | "article"
            | "nav" | "main" | "aside" | "form" | "thead" | "tbody" | "tfoot" | "fieldset"
            | "figcaption" | "figure" | "hr" | "dd" | "dt" | "dl" => Display::Block,
            "table" => Display::Table,
            "tr" => Display::TableRow,
            "td" | "th" => Display::TableCell,
            "button" | "input" | "select" | "textarea" | "img" | "canvas" => Display::InlineBlock,
            "head" | "script" | "style" | "meta" | "link" | "title" => Display::None,
            _ => Display::Inline,
        },
        NodeType::Text(_) => Display::Inline,
        _ => Display::None,
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Display {
    Inline,
    Block,
    Flex,
    Grid,
    Table,
    TableRow,
    TableCell,
    InlineBlock,
    None,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Position {
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Overflow {
    Visible,
    Hidden,
    Scroll,
    Auto,
}

// ── Interactive state context ───────────────────────────────────────────────

#[derive(Clone, Default)]
pub struct InteractionState<'a> {
    pub hovered_node: Option<&'a Node>,
    pub active_node: Option<&'a Node>,
    pub focused_node: Option<&'a Node>,
}

// ── Sibling context (for pseudo-class matching) ───────────────────────────────

/// Information about an element's position among its siblings.
/// Used to evaluate `:nth-child`, `:first-child`, `:empty`, etc.
#[derive(Clone)]
struct SiblingContext<'a> {
    /// 1-indexed position among all element siblings.
    position: usize,
    /// Total element sibling count (including self).
    total: usize,
    /// 1-indexed position among element siblings of the same tag type.
    type_position: usize,
    /// Total element sibling count of the same tag type.
    type_total: usize,
    /// True when the element has no meaningful children (for `:empty`).
    is_empty: bool,
    /// True when the element is the root element of the document.
    is_root: bool,
    /// Current element node reference for interactive pseudo-classes.
    current_node: Option<&'a Node>,
}

impl<'a> SiblingContext<'a> {
    /// Fallback context used when sibling information is unavailable (e.g. for
    /// ancestor parts in compound selectors).
    fn unknown() -> Self {
        Self {
            position: 1,
            total: 1,
            type_position: 1,
            type_total: 1,
            is_empty: false,
            is_root: false,
            current_node: None,
        }
    }
}

// ── Selector matching ─────────────────────────────────────────────────────────

fn pseudo_class_matches(
    pc: &PseudoClass,
    element: &ElementData,
    ancestors: &[&ElementData],
    ctx: &SiblingContext,
    interaction: &InteractionState,
) -> bool {
    match pc {
        PseudoClass::FirstChild => ctx.position == 1,
        PseudoClass::LastChild => ctx.position == ctx.total,
        PseudoClass::OnlyChild => ctx.total == 1,
        PseudoClass::Root => ctx.is_root,
        PseudoClass::Empty => ctx.is_empty,
        PseudoClass::FirstOfType => ctx.type_position == 1,
        PseudoClass::LastOfType => ctx.type_position == ctx.type_total,
        PseudoClass::OnlyOfType => ctx.type_total == 1,
        PseudoClass::NthChild(expr) => expr.matches(ctx.position),
        PseudoClass::NthLastChild(expr) => expr.matches(ctx.total + 1 - ctx.position),
        PseudoClass::NthOfType(expr) => expr.matches(ctx.type_position),
        PseudoClass::NthLastOfType(expr) => expr.matches(ctx.type_total + 1 - ctx.type_position),
        PseudoClass::Not(inner) => {
            !simple_part_matches(element, inner, ancestors, ctx, interaction)
        }
        PseudoClass::Hover => {
            if let (Some(cur), Some(target)) = (ctx.current_node, interaction.hovered_node) {
                std::ptr::eq(cur, target)
            } else {
                false
            }
        }
        PseudoClass::Active => {
            if let (Some(cur), Some(target)) = (ctx.current_node, interaction.active_node) {
                std::ptr::eq(cur, target)
            } else {
                false
            }
        }
        // Checkedness is live state: a box the user toggled no longer agrees
        // with its `checked` attribute.
        PseudoClass::Checked => element.is_checked(),
        // Only elements that can actually be disabled match these.
        PseudoClass::Disabled => element.is_form_control() && element.is_disabled(),
        PseudoClass::Enabled => element.is_form_control() && !element.is_disabled(),
        PseudoClass::PlaceholderShown => element.placeholder_shown(),
        PseudoClass::Focus => {
            if let (Some(cur), Some(target)) = (ctx.current_node, interaction.focused_node) {
                std::ptr::eq(cur, target)
            } else {
                false
            }
        }
        PseudoClass::FocusWithin => match (ctx.current_node, interaction.focused_node) {
            (Some(cur), Some(target)) => contains_node(cur, target),
            _ => false,
        },
        PseudoClass::Visited | PseudoClass::Link => false,
    }
}

/// True when `target` is `root` or sits inside it — the test behind
/// `:focus-within`.
fn contains_node(root: &Node, target: &Node) -> bool {
    if std::ptr::eq(root, target) {
        return true;
    }
    root.children
        .iter()
        .any(|child| contains_node(child, target))
}

fn simple_part_matches(
    element: &ElementData,
    part: &SelectorPart,
    ancestors: &[&ElementData],
    ctx: &SiblingContext,
    interaction: &InteractionState,
) -> bool {
    if let Some(ref tag) = part.tag_name {
        if element.tag_name != *tag {
            return false;
        }
    }
    if let Some(ref id) = part.id {
        if element.get_attr("id") != Some(id.as_str()) {
            return false;
        }
    }
    let elem_classes: Vec<&str> = element
        .get_attr("class")
        .map(|s| s.split_whitespace().collect())
        .unwrap_or_default();
    for cls in &part.classes {
        if !elem_classes.contains(&cls.as_str()) {
            return false;
        }
    }
    for (attr, expected_val) in &part.attributes {
        match element.get_attr(attr) {
            Some(actual_val) => {
                if let Some(expected) = expected_val {
                    if actual_val != expected {
                        return false;
                    }
                }
            }
            None => return false,
        }
    }
    for pc in &part.pseudo_classes {
        if !pseudo_class_matches(pc, element, ancestors, ctx, interaction) {
            return false;
        }
    }
    true
}

/// Match a full compound selector against `element` with its ancestor chain.
/// `ancestors` is ordered closest-first: [parent, grandparent, …].
/// `preceding_siblings` is ordered first-to-last (last = immediately preceding sibling).
fn selector_matches(
    element: &ElementData,
    ancestors: &[&ElementData],
    preceding_siblings: &[&ElementData],
    selector: &Selector,
    ctx: &SiblingContext,
    interaction: &InteractionState,
) -> bool {
    if selector.parts.is_empty() {
        return false;
    }

    // The last part must match the subject element.
    if !simple_part_matches(
        element,
        selector.parts.last().unwrap(),
        ancestors,
        ctx,
        interaction,
    ) {
        return false;
    }
    if selector.parts.len() == 1 {
        return true;
    }

    let mut cursor = 0usize;
    let dummy = SiblingContext::unknown();
    // Working copy of preceding siblings; shrinks as we consume sibling combinators.
    let mut sibs: Vec<&ElementData> = preceding_siblings.to_vec();

    for part_idx in (0..selector.parts.len() - 1).rev() {
        let part = &selector.parts[part_idx];
        let combinator = &selector.parts[part_idx + 1].combinator;

        match combinator {
            Combinator::Root => break,
            Combinator::Child => {
                if cursor >= ancestors.len() {
                    return false;
                }
                if !simple_part_matches(
                    ancestors[cursor],
                    part,
                    &ancestors[cursor + 1..],
                    &dummy,
                    interaction,
                ) {
                    return false;
                }
                cursor += 1;
                sibs.clear(); // sibling info lost after jumping up to an ancestor
            }
            Combinator::Descendant => {
                let offset = ancestors[cursor..].iter().enumerate().position(|(i, a)| {
                    simple_part_matches(a, part, &ancestors[cursor + i + 1..], &dummy, interaction)
                });
                match offset {
                    Some(i) => {
                        cursor += i + 1;
                        sibs.clear();
                    }
                    None => return false,
                }
            }
            Combinator::AdjacentSibling => {
                // The immediately preceding element sibling must match `part`.
                if sibs.is_empty() {
                    return false;
                }
                let prev = sibs[sibs.len() - 1];
                if !simple_part_matches(prev, part, ancestors, &dummy, interaction) {
                    return false;
                }
                sibs.pop();
            }
            Combinator::GeneralSibling => {
                // Any preceding element sibling (right-to-left search) must match `part`.
                let found = sibs
                    .iter()
                    .rposition(|s| simple_part_matches(s, part, ancestors, &dummy, interaction));
                match found {
                    Some(i) => sibs.truncate(i),
                    None => return false,
                }
            }
        }
    }
    true
}

/// All rules that match `element`, paired with their winning selector's specificity.
/// The parts of a cascade that are the same for every element in one pass:
/// the stylesheet being matched, the viewport width `@media` is evaluated
/// against, and the interaction state that `:hover` / `:focus` / `:checked`
/// read. Carrying them as one value keeps the recursive walk's signature from
/// growing a parameter every time the cascade learns something new.
#[derive(Clone, Copy)]
struct Cascade<'s, 'i> {
    stylesheet: &'s Stylesheet,
    viewport_w: f32,
    interaction: &'s InteractionState<'i>,
}

fn matching_rules<'a>(
    element: &ElementData,
    ancestors: &[&ElementData],
    preceding_siblings: &[&ElementData],
    ctx: &SiblingContext,
    cascade: Cascade<'a, '_>,
) -> Vec<MatchedRule<'a>> {
    let Cascade {
        stylesheet,
        viewport_w,
        interaction,
    } = cascade;
    let mut matched = Vec::new();
    for rule in &stylesheet.rules {
        // Filter by @media query.
        if let Some(mq) = &rule.media_query {
            if !mq.matches(viewport_w, 600.0) {
                continue;
            }
        }
        for selector in &rule.selectors {
            if selector_matches(
                element,
                ancestors,
                preceding_siblings,
                selector,
                ctx,
                interaction,
            ) {
                matched.push((selector.specificity(), rule.declarations.as_slice()));
                break;
            }
        }
    }
    // Low specificity first so higher specificity wins by overwriting.
    matched.sort_by_key(|&(spec, _)| spec);
    matched
}

// ── CSS variable resolution ───────────────────────────────────────────────────

/// Resolve a `Value::Var { name, fallback }` against the given custom-property map.
/// `custom_props` holds values for `--*` properties from the element and its ancestors.
fn resolve_value(val: Value, custom_props: &PropertyMap) -> Value {
    match val {
        Value::Var { name, fallback } => {
            if let Some(stored) = custom_props.get(&name) {
                let parsed = match stored {
                    Value::Keyword(raw) => parse_single_value(raw),
                    other => other.clone(),
                };
                resolve_value(parsed, custom_props)
            } else if let Some(fb) = fallback {
                resolve_value(*fb, custom_props)
            } else {
                Value::Keyword(String::new())
            }
        }
        other => other,
    }
}

fn compute_specified_values(
    element: &ElementData,
    ancestors: &[&ElementData],
    preceding_siblings: &[&ElementData],
    inherited: &PropertyMap,
    ctx: &SiblingContext,
    cascade: Cascade,
) -> PropertyMap {
    // Cascade order, weakest first: normal author rules (by specificity), then
    // the normal `style` attribute, then the same two rounds again for
    // `!important` declarations, which outrank every normal one.
    let matched = matching_rules(element, ancestors, preceding_siblings, ctx, cascade);
    let inline = element
        .get_attr("style")
        .map(parse_declaration_block)
        .unwrap_or_default();

    let mut map = PropertyMap::new();
    for important in [false, true] {
        for (_, declarations) in &matched {
            for decl in declarations.iter().filter(|d| d.important == important) {
                map.insert(decl.name.clone(), decl.value.clone());
            }
        }
        // The `style` attribute outranks author rules of the same weight — this
        // is also how a script's `element.style.x = …` write takes effect.
        for decl in inline.iter().filter(|d| d.important == important) {
            map.insert(decl.name.clone(), decl.value.clone());
        }
    }

    // Build the custom-property lookup: inherited --vars first, then own --vars override.
    let mut custom_props: PropertyMap = HashMap::new();
    for (k, v) in inherited {
        if k.starts_with("--") {
            custom_props.insert(k.clone(), v.clone());
        }
    }
    for (k, v) in &map {
        if k.starts_with("--") {
            custom_props.insert(k.clone(), v.clone());
        }
    }

    // Resolve var() in non-custom properties (always, even when custom_props is empty,
    // so that var(--x, fallback) can fall through to its fallback).
    for (k, v) in map.iter_mut() {
        if !k.starts_with("--") && matches!(v, Value::Var { .. }) {
            *v = resolve_value(v.clone(), &custom_props);
        }
    }

    // Retain all inherited custom properties in the specified_values map
    // so getComputedStyle() and introspection can read them directly.
    for (k, v) in inherited {
        if k.starts_with("--") && !map.contains_key(k) {
            map.insert(k.clone(), v.clone());
        }
    }

    map
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Compute the final specified/computed property map for an element at a given path in the DOM tree.
pub fn compute_element_style(
    root: &Node,
    target_path: &[usize],
    stylesheet: &Stylesheet,
    viewport_width: f32,
) -> Option<PropertyMap> {
    let tree = style_tree_for_viewport(root, stylesheet, viewport_width);
    tree.find_at_path(target_path).map(|sn| sn.specified_values.clone())
}

/// Build a style tree assuming an 800 px viewport (for backwards compatibility).
pub fn style_tree<'a>(root: &'a Node, stylesheet: &Stylesheet) -> StyledNode<'a> {
    style_tree_for_viewport(root, stylesheet, 800.0)
}

/// Build a style tree with an interaction state (e.g. hovered node).
pub fn style_tree_with_interaction<'a>(
    root: &'a Node,
    stylesheet: &Stylesheet,
    interaction: &InteractionState<'a>,
) -> StyledNode<'a> {
    style_tree_full(root, stylesheet, 800.0, interaction)
}

/// Build a style tree with a specific viewport width for @media query evaluation.
pub fn style_tree_for_viewport<'a>(
    root: &'a Node,
    stylesheet: &Stylesheet,
    viewport_width: f32,
) -> StyledNode<'a> {
    style_tree_full(
        root,
        stylesheet,
        viewport_width,
        &InteractionState::default(),
    )
}

/// Build a style tree with both a viewport width and an interaction state —
/// the entry point the document layer uses, so `@media` and `:hover` agree on
/// the same frame.
pub fn style_tree_full<'a>(
    root: &'a Node,
    stylesheet: &Stylesheet,
    viewport_width: f32,
    interaction: &InteractionState<'a>,
) -> StyledNode<'a> {
    style_tree_inner(
        root,
        &PropertyMap::new(),
        &[],
        &[],
        SiblingContext::unknown(),
        Cascade {
            stylesheet,
            viewport_w: viewport_width,
            interaction,
        },
    )
}

fn style_tree_inner<'a>(
    root: &'a Node,
    inherited: &PropertyMap,
    ancestors: &[&'a ElementData],
    preceding_siblings: &[&'a ElementData],
    mut sibling_ctx: SiblingContext<'a>,
    cascade: Cascade<'_, 'a>,
) -> StyledNode<'a> {
    sibling_ctx.current_node = Some(root);
    // Compute values matched by CSS rules for this element.
    let mut specified_values = match &root.node_type {
        NodeType::Element(e) => compute_specified_values(
            e,
            ancestors,
            preceding_siblings,
            inherited,
            &sibling_ctx,
            cascade,
        ),
        _ => PropertyMap::new(),
    };

    // Resolve `font-size: Nem` against the PARENT's font-size (not the element's own).
    // This must happen before inheritance is applied so children see the resolved px value.
    if let Some(Value::Length(n, Unit::Em)) = specified_values.get("font-size").cloned() {
        let parent_fs = inherited
            .get("font-size")
            .and_then(|v| {
                if let Value::Length(px, Unit::Px) = v {
                    Some(*px)
                } else {
                    None
                }
            })
            .unwrap_or(16.0);
        specified_values.insert("font-size".into(), Value::Length(n * parent_fs, Unit::Px));
    }

    // Apply CSS inheritance: inheritable properties not explicitly set are taken
    // from the parent's computed values.
    for prop in INHERITED {
        if !specified_values.contains_key(*prop) {
            if let Some(val) = inherited.get(*prop) {
                specified_values.insert(prop.to_string(), val.clone());
            }
        }
    }

    // Build the inherited map for children.
    let mut child_inherited = inherited.clone();
    for prop in INHERITED {
        if let Some(val) = specified_values.get(*prop) {
            child_inherited.insert(prop.to_string(), val.clone());
        }
    }
    // Custom properties (--*) always inherit.
    for (k, v) in &specified_values {
        if k.starts_with("--") {
            child_inherited.insert(k.clone(), v.clone());
        }
    }
    // Also propagate any inherited custom properties that were not overridden.
    for (k, v) in inherited {
        if k.starts_with("--") && !child_inherited.contains_key(k) {
            child_inherited.insert(k.clone(), v.clone());
        }
    }

    // Determine the ancestor chain for children.
    let child_ancestor_vec: Vec<&'a ElementData> = match &root.node_type {
        NodeType::Element(e) => {
            let mut v = Vec::with_capacity(ancestors.len() + 1);
            v.push(e);
            v.extend_from_slice(ancestors);
            v
        }
        _ => ancestors.to_vec(),
    };

    // Collect element-child indices for sibling-position computation.
    let elem_child_indices: Vec<usize> = root
        .children
        .iter()
        .enumerate()
        .filter(|(_, c)| matches!(c.node_type, NodeType::Element(_)))
        .map(|(i, _)| i)
        .collect();
    let elem_count = elem_child_indices.len();

    let children: Vec<StyledNode<'a>> = root
        .children
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let ctx = if let NodeType::Element(ce) = &c.node_type {
                // 1-indexed position among element siblings.
                let position = elem_child_indices
                    .iter()
                    .position(|&ei| ei == i)
                    .map(|p| p + 1)
                    .unwrap_or(1);

                // Position among same-tag-type siblings.
                let type_total = elem_child_indices
                    .iter()
                    .filter(|&&ei| {
                        matches!(&root.children[ei].node_type,
                    NodeType::Element(se) if se.tag_name == ce.tag_name)
                    })
                    .count();
                let type_position = elem_child_indices
                    .iter()
                    .filter(|&&ei| {
                        ei <= i
                            && matches!(&root.children[ei].node_type,
                    NodeType::Element(se) if se.tag_name == ce.tag_name)
                    })
                    .count()
                    .max(1);

                let is_empty = c.children.iter().all(|n| match &n.node_type {
                    NodeType::Text(t) => t.trim().is_empty(),
                    NodeType::Comment(_) => true,
                    _ => false,
                });
                let is_root = ancestors.is_empty();

                SiblingContext {
                    position,
                    total: elem_count,
                    type_position,
                    type_total,
                    is_empty,
                    is_root,
                    current_node: Some(c),
                }
            } else {
                SiblingContext::unknown()
            };

            // Preceding element siblings (first…last order) for sibling combinator matching.
            let child_preceding: Vec<&'a ElementData> = elem_child_indices
                .iter()
                .filter(|&&ei| ei < i)
                .filter_map(|&ei| {
                    if let NodeType::Element(se) = &root.children[ei].node_type {
                        Some(se as &'a ElementData)
                    } else {
                        None
                    }
                })
                .collect();

            style_tree_inner(
                c,
                &child_inherited,
                &child_ancestor_vec,
                &child_preceding,
                ctx,
                cascade,
            )
        })
        .collect();

    StyledNode {
        node: root,
        specified_values,
        children,
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::parser::{parse_css, Color, Value};
    use crate::html::parse_html;

    #[test]
    fn tag_rule_matches() {
        let dom = parse_html("<p>text</p>");
        let ss = parse_css("p { color: red; }");
        let styled = style_tree(&dom, &ss);
        assert!(styled.specified_values.is_empty());
        let p = &styled.children[0];
        assert_eq!(p.value("color"), Some(&Value::Color(Color::rgb(255, 0, 0))));
    }

    #[test]
    fn id_rule_matches() {
        let dom = parse_html(r#"<div id="hero">x</div>"#);
        let ss = parse_css("#hero { background-color: blue; }");
        let styled = style_tree(&dom, &ss);
        let div = &styled.children[0];
        assert_eq!(
            div.value("background-color"),
            Some(&Value::Color(Color::rgb(0, 0, 255)))
        );
    }

    #[test]
    fn class_rule_matches() {
        let dom = parse_html(r#"<span class="highlight">x</span>"#);
        let ss = parse_css(".highlight { color: yellow; }");
        let styled = style_tree(&dom, &ss);
        let span = &styled.children[0];
        assert_eq!(
            span.value("color"),
            Some(&Value::Color(Color::rgb(255, 255, 0)))
        );
    }

    #[test]
    fn specificity_cascade() {
        let dom = parse_html(r#"<p id="x">t</p>"#);
        let ss = parse_css("p { color: red; } #x { color: blue; }");
        let styled = style_tree(&dom, &ss);
        let p = &styled.children[0];
        assert_eq!(p.value("color"), Some(&Value::Color(Color::rgb(0, 0, 255))));
    }

    #[test]
    fn default_display_block() {
        let dom = parse_html("<div></div>");
        let ss = parse_css("");
        let styled = style_tree(&dom, &ss);
        assert_eq!(styled.children[0].display(), Display::Block);
    }

    #[test]
    fn display_none_hides_head() {
        let dom = parse_html("<head><title>T</title></head>");
        let ss = parse_css("");
        let styled = style_tree(&dom, &ss);
        assert_eq!(styled.children[0].display(), Display::None);
    }

    #[test]
    fn positioned_node_with_z_index_establishes_stacking_context() {
        let dom = parse_html("<div></div>");
        let ss = parse_css("div { position: relative; z-index: -2; }");
        let styled = style_tree(&dom, &ss);
        let div = &styled.children[0];
        assert_eq!(div.position(), Position::Relative);
        assert_eq!(div.z_index(), Some(-2));
        assert!(div.establishes_stacking_context());
    }

    #[test]
    fn static_node_does_not_establish_stacking_context() {
        let dom = parse_html("<div></div>");
        let ss = parse_css("div { z-index: 2; }");
        let styled = style_tree(&dom, &ss);
        let div = &styled.children[0];
        assert_eq!(div.position(), Position::Static);
        assert_eq!(div.z_index(), Some(2));
        assert!(!div.establishes_stacking_context());
    }

    #[test]
    fn descendant_selector_matches() {
        let dom = parse_html("<footer><p>text</p></footer>");
        let ss = parse_css("footer p { color: red; }");
        let styled = style_tree(&dom, &ss);
        let footer = &styled.children[0];
        let p = &footer.children[0];
        assert_eq!(p.value("color"), Some(&Value::Color(Color::rgb(255, 0, 0))));
        assert_eq!(footer.value("color"), None);
    }

    #[test]
    fn child_selector_matches_only_direct_child() {
        let dom = parse_html("<nav><div><a>link</a></div></nav>");
        let ss = parse_css("nav > a { color: blue; }");
        let styled = style_tree(&dom, &ss);
        let a = &styled.children[0].children[0].children[0];
        assert_eq!(a.value("color"), None);
    }

    #[test]
    fn child_selector_matches_direct_child() {
        let dom = parse_html("<nav><a>link</a></nav>");
        let ss = parse_css("nav > a { color: green; }");
        let styled = style_tree(&dom, &ss);
        let a = &styled.children[0].children[0];
        assert_eq!(a.value("color"), Some(&Value::Color(Color::rgb(0, 128, 0))));
    }

    #[test]
    fn color_inherits_to_children() {
        let dom = parse_html("<div><p>text</p></div>");
        let ss = parse_css("div { color: red; }");
        let styled = style_tree(&dom, &ss);
        let p = &styled.children[0].children[0];
        assert_eq!(p.value("color"), Some(&Value::Color(Color::rgb(255, 0, 0))));
    }

    #[test]
    fn explicit_value_overrides_inheritance() {
        let dom = parse_html("<div><p>text</p></div>");
        let ss = parse_css("div { color: red; } p { color: blue; }");
        let styled = style_tree(&dom, &ss);
        let p = &styled.children[0].children[0];
        assert_eq!(p.value("color"), Some(&Value::Color(Color::rgb(0, 0, 255))));
    }

    // ── Pseudo-class tests ────────────────────────────────────────────────

    #[test]
    fn first_child_matches() {
        let dom = parse_html("<ul><li>a</li><li>b</li><li>c</li></ul>");
        let ss = parse_css("li:first-child { color: red; }");
        let styled = style_tree(&dom, &ss);
        let ul = &styled.children[0];
        let li0 = &ul.children[0];
        let li1 = &ul.children[1];
        assert_eq!(
            li0.value("color"),
            Some(&Value::Color(Color::rgb(255, 0, 0)))
        );
        assert_eq!(li1.value("color"), None);
    }

    #[test]
    fn last_child_matches() {
        let dom = parse_html("<ul><li>a</li><li>b</li><li>c</li></ul>");
        let ss = parse_css("li:last-child { color: blue; }");
        let styled = style_tree(&dom, &ss);
        let ul = &styled.children[0];
        let li2 = &ul.children[2];
        assert_eq!(
            li2.value("color"),
            Some(&Value::Color(Color::rgb(0, 0, 255)))
        );
        assert_eq!(ul.children[0].value("color"), None);
    }

    #[test]
    fn nth_child_matches_second() {
        let dom = parse_html("<ul><li>a</li><li>b</li><li>c</li></ul>");
        let ss = parse_css("li:nth-child(2) { color: blue; }");
        let styled = style_tree(&dom, &ss);
        let ul = &styled.children[0];
        assert_eq!(ul.children[0].value("color"), None);
        assert_eq!(
            ul.children[1].value("color"),
            Some(&Value::Color(Color::rgb(0, 0, 255)))
        );
        assert_eq!(ul.children[2].value("color"), None);
    }

    #[test]
    fn nth_child_odd_matches() {
        let dom = parse_html("<ul><li>a</li><li>b</li><li>c</li></ul>");
        let ss = parse_css("li:nth-child(odd) { color: red; }");
        let styled = style_tree(&dom, &ss);
        let ul = &styled.children[0];
        assert_eq!(
            ul.children[0].value("color"),
            Some(&Value::Color(Color::rgb(255, 0, 0)))
        );
        assert_eq!(ul.children[1].value("color"), None);
        assert_eq!(
            ul.children[2].value("color"),
            Some(&Value::Color(Color::rgb(255, 0, 0)))
        );
    }

    #[test]
    fn only_child_matches() {
        let dom = parse_html("<div><p>only</p></div>");
        let ss = parse_css("p:only-child { color: red; }");
        let styled = style_tree(&dom, &ss);
        let p = &styled.children[0].children[0];
        assert_eq!(p.value("color"), Some(&Value::Color(Color::rgb(255, 0, 0))));
    }

    #[test]
    fn not_pseudo_class_excludes_matched() {
        let dom = parse_html(r#"<ul><li class="special">a</li><li>b</li></ul>"#);
        let ss = parse_css("li:not(.special) { color: red; }");
        let styled = style_tree(&dom, &ss);
        let ul = &styled.children[0];
        assert_eq!(ul.children[0].value("color"), None); // .special excluded
        assert_eq!(
            ul.children[1].value("color"),
            Some(&Value::Color(Color::rgb(255, 0, 0)))
        );
    }

    #[test]
    fn first_of_type_matches() {
        let dom = parse_html("<div><p>a</p><span>b</span><p>c</p></div>");
        let ss = parse_css("p:first-of-type { color: red; }");
        let styled = style_tree(&dom, &ss);
        let div = &styled.children[0];
        assert_eq!(
            div.children[0].value("color"),
            Some(&Value::Color(Color::rgb(255, 0, 0)))
        );
        assert_eq!(div.children[2].value("color"), None);
    }

    // ── CSS variable tests ────────────────────────────────────────────────

    #[test]
    fn css_var_resolves_from_same_element() {
        let dom = parse_html("<div></div>");
        let ss = parse_css("div { --primary: red; color: var(--primary); }");
        let styled = style_tree(&dom, &ss);
        let div = &styled.children[0];
        assert_eq!(
            div.value("color"),
            Some(&Value::Color(Color::rgb(255, 0, 0)))
        );
    }

    #[test]
    fn css_var_fallback_used_when_undefined() {
        let dom = parse_html("<div></div>");
        let ss = parse_css("div { color: var(--undefined, blue); }");
        let styled = style_tree(&dom, &ss);
        let div = &styled.children[0];
        assert_eq!(
            div.value("color"),
            Some(&Value::Color(Color::rgb(0, 0, 255)))
        );
    }

    #[test]
    fn css_var_inherits_from_parent() {
        let dom = parse_html("<div><p>text</p></div>");
        let ss = parse_css("div { --accent: green; } p { color: var(--accent); }");
        let styled = style_tree(&dom, &ss);
        let p = &styled.children[0].children[0];
        assert_eq!(p.value("color"), Some(&Value::Color(Color::rgb(0, 128, 0))));
    }

    #[test]
    fn css_var_on_root_inherits_everywhere() {
        let dom = parse_html("<div><p>text</p></div>");
        let ss = parse_css(":root { --text: navy; } p { color: var(--text); }");
        let styled = style_tree(&dom, &ss);
        // :root matches the document root (first element) — the div here
        let p = &styled.children[0].children[0];
        assert_eq!(p.value("color"), Some(&Value::Color(Color::rgb(0, 0, 128))));
    }

    // ── @media tests ─────────────────────────────────────────────────────

    #[test]
    fn media_max_width_applies_when_narrow() {
        let dom = parse_html("<div></div>");
        let ss = parse_css("@media (max-width: 600px) { div { color: red; } }");
        let narrow = style_tree_for_viewport(&dom, &ss, 400.0);
        let wide = style_tree_for_viewport(&dom, &ss, 800.0);
        assert_eq!(
            narrow.children[0].value("color"),
            Some(&Value::Color(Color::rgb(255, 0, 0)))
        );
        assert_eq!(wide.children[0].value("color"), None);
    }

    #[test]
    fn media_min_width_applies_when_wide() {
        let dom = parse_html("<div></div>");
        let ss = parse_css("@media (min-width: 768px) { div { color: blue; } }");
        let narrow = style_tree_for_viewport(&dom, &ss, 400.0);
        let wide = style_tree_for_viewport(&dom, &ss, 1024.0);
        assert_eq!(narrow.children[0].value("color"), None);
        assert_eq!(
            wide.children[0].value("color"),
            Some(&Value::Color(Color::rgb(0, 0, 255)))
        );
    }

    #[test]
    fn media_print_never_applies() {
        let dom = parse_html("<div></div>");
        let ss = parse_css("@media print { div { color: red; } }");
        let styled = style_tree_for_viewport(&dom, &ss, 800.0);
        assert_eq!(styled.children[0].value("color"), None);
    }

    #[test]
    fn media_screen_always_applies() {
        let dom = parse_html("<div></div>");
        let ss = parse_css("@media screen { div { color: red; } }");
        let styled = style_tree_for_viewport(&dom, &ss, 800.0);
        assert_eq!(
            styled.children[0].value("color"),
            Some(&Value::Color(Color::rgb(255, 0, 0)))
        );
    }

    // ── Sibling combinator tests ──────────────────────────────────────────

    #[test]
    fn adjacent_sibling_matches_immediately_following() {
        // h1 + p should color the p that immediately follows h1
        let dom = parse_html("<div><h1>head</h1><p>para</p><p>other</p></div>");
        let ss = parse_css("h1 + p { color: red; }");
        let styled = style_tree(&dom, &ss);
        let div = &styled.children[0];
        // div.children: [h1, p, p]
        let p0 = &div.children[1]; // immediately after h1
        let p1 = &div.children[2]; // second p — should NOT match
        assert_eq!(
            p0.value("color"),
            Some(&Value::Color(Color::rgb(255, 0, 0)))
        );
        assert_eq!(p1.value("color"), None);
    }

    #[test]
    fn adjacent_sibling_does_not_match_non_adjacent() {
        // h1 + p should NOT match a p that has another element between them
        let dom = parse_html("<div><h1>head</h1><span>x</span><p>para</p></div>");
        let ss = parse_css("h1 + p { color: red; }");
        let styled = style_tree(&dom, &ss);
        let p = &styled.children[0].children[2];
        assert_eq!(p.value("color"), None);
    }

    #[test]
    fn general_sibling_matches_any_following() {
        // h1 ~ p should color ALL p elements that follow h1
        let dom = parse_html("<div><h1>head</h1><span>x</span><p>one</p><p>two</p></div>");
        let ss = parse_css("h1 ~ p { color: blue; }");
        let styled = style_tree(&dom, &ss);
        let div = &styled.children[0];
        // div.children: [h1, span, p, p]
        let p0 = &div.children[2];
        let p1 = &div.children[3];
        assert_eq!(
            p0.value("color"),
            Some(&Value::Color(Color::rgb(0, 0, 255)))
        );
        assert_eq!(
            p1.value("color"),
            Some(&Value::Color(Color::rgb(0, 0, 255)))
        );
    }

    #[test]
    fn general_sibling_does_not_match_when_order_reversed() {
        // p ~ h1 should NOT match h1 that has no p among its preceding siblings.
        // DOM: h1 comes first, then p — so h1's preceding_siblings = []; no p found.
        let dom = parse_html("<div><h1>head</h1><p>para</p></div>");
        let ss = parse_css("p ~ h1 { color: red; }");
        let styled = style_tree(&dom, &ss);
        let h1 = &styled.children[0].children[0]; // first child, no preceding siblings
        assert_eq!(h1.value("color"), None);
    }

    // ── em font-size tests ────────────────────────────────────────────────

    #[test]
    fn em_font_size_resolves_against_parent() {
        // Parent has font-size: 20px; child has font-size: 1.5em → should resolve to 30px
        let dom = parse_html("<div><p>text</p></div>");
        let ss = parse_css("div { font-size: 20px; } p { font-size: 1.5em; }");
        let styled = style_tree(&dom, &ss);
        let p = &styled.children[0].children[0];
        assert_eq!(p.value("font-size"), Some(&Value::Length(30.0, Unit::Px)));
    }

    #[test]
    fn em_font_size_uses_default_16_when_no_parent() {
        // Root element with em font-size: 2em → 2 × 16px = 32px
        let dom = parse_html("<div></div>");
        let ss = parse_css("div { font-size: 2em; }");
        let styled = style_tree(&dom, &ss);
        let div = &styled.children[0];
        assert_eq!(div.value("font-size"), Some(&Value::Length(32.0, Unit::Px)));
    }

    #[test]
    fn em_font_size_inherited_as_px_by_grandchild() {
        // div: 20px, p: 1.5em→30px, span: inherits 30px (not 1.5em applied again)
        let dom = parse_html("<div><p><span>text</span></p></div>");
        let ss = parse_css("div { font-size: 20px; } p { font-size: 1.5em; }");
        let styled = style_tree(&dom, &ss);
        let span = &styled.children[0].children[0].children[0];
        assert_eq!(
            span.value("font-size"),
            Some(&Value::Length(30.0, Unit::Px))
        );
    }

    #[test]
    fn nested_css_var_fallback_resolves() {
        let dom = parse_html("<div><p>text</p></div>");
        let ss = parse_css(
            "div { --a: var(--b, #00ff00); } p { color: var(--missing, var(--a, #ff0000)); }",
        );
        let styled = style_tree(&dom, &ss);
        let p = &styled.children[0].children[0];
        assert_eq!(
            p.value("color"),
            Some(&Value::Color(crate::css::parser::Color::rgb(0, 255, 0)))
        );
    }

    #[test]
    fn hover_selector_matches_with_interaction() {
        let dom = parse_html("<button>Click</button>");
        let ss = parse_css("button:hover { color: red; }");
        let btn = &dom.children[0];
        let interaction = InteractionState {
            hovered_node: Some(btn),
            active_node: None,
            focused_node: None,
        };
        let styled = style_tree_with_interaction(&dom, &ss, &interaction);
        let styled_btn = &styled.children[0];
        assert_eq!(
            styled_btn.value("color"),
            Some(&Value::Color(Color::rgb(255, 0, 0)))
        );
    }

    #[test]
    fn focus_selector_matches_with_interaction() {
        let dom = parse_html("<input type=\"text\" id=\"inp\" />");
        let ss = parse_css("input:focus { color: blue; }");
        let inp = &dom.children[0];
        let interaction = InteractionState {
            hovered_node: None,
            active_node: None,
            focused_node: Some(inp),
        };
        let styled = style_tree_with_interaction(&dom, &ss, &interaction);
        let styled_inp = &styled.children[0];
        assert_eq!(
            styled_inp.value("color"),
            Some(&Value::Color(Color::rgb(0, 0, 255)))
        );
    }

    // ── !important ────────────────────────────────────────────────────────

    #[test]
    fn important_beats_a_more_specific_rule() {
        let dom = parse_html(r#"<p id="x">t</p>"#);
        let ss = parse_css("p { color: red !important; } #x { color: blue; }");
        let styled = style_tree(&dom, &ss);
        assert_eq!(
            styled.children[0].value("color"),
            Some(&Value::Color(Color::rgb(255, 0, 0)))
        );
    }

    #[test]
    fn important_beats_the_inline_style_attribute() {
        let dom = parse_html(r#"<p style="color: blue">t</p>"#);
        let ss = parse_css("p { color: red !important; }");
        let styled = style_tree(&dom, &ss);
        assert_eq!(
            styled.children[0].value("color"),
            Some(&Value::Color(Color::rgb(255, 0, 0)))
        );
    }

    #[test]
    fn important_inline_style_beats_an_important_rule() {
        let dom = parse_html(r#"<p style="color: blue !important">t</p>"#);
        let ss = parse_css("p { color: red !important; }");
        let styled = style_tree(&dom, &ss);
        assert_eq!(
            styled.children[0].value("color"),
            Some(&Value::Color(Color::rgb(0, 0, 255)))
        );
    }

    #[test]
    fn later_important_rule_wins_over_earlier_one() {
        let dom = parse_html("<p>t</p>");
        let ss = parse_css("p { color: red !important; } p { color: green !important; }");
        let styled = style_tree(&dom, &ss);
        assert_eq!(
            styled.children[0].value("color"),
            Some(&Value::Color(Color::rgb(0, 128, 0)))
        );
    }

    #[test]
    fn important_applies_across_a_shorthand() {
        let dom = parse_html("<div></div>");
        let ss = parse_css("div { margin: 4px; } div { margin: 9px !important; }");
        let styled = style_tree(&dom, &ss);
        let div = &styled.children[0];
        assert_eq!(div.value("margin-top"), Some(&Value::Length(9.0, Unit::Px)));
        assert_eq!(
            div.value("margin-left"),
            Some(&Value::Length(9.0, Unit::Px))
        );
    }

    // ── Inline `style` attribute ──────────────────────────────────────────

    #[test]
    fn inline_style_attribute_applies() {
        let dom = parse_html(r#"<div style="color: red; width: 40px"></div>"#);
        let styled = style_tree(&dom, &parse_css(""));
        let div = &styled.children[0];
        assert_eq!(
            div.value("color"),
            Some(&Value::Color(Color::rgb(255, 0, 0)))
        );
        assert_eq!(div.value("width"), Some(&Value::Length(40.0, Unit::Px)));
    }

    #[test]
    fn inline_style_beats_an_id_rule() {
        let dom = parse_html(r#"<p id="x" style="color: blue">t</p>"#);
        let ss = parse_css("#x { color: red; }");
        let styled = style_tree(&dom, &ss);
        assert_eq!(
            styled.children[0].value("color"),
            Some(&Value::Color(Color::rgb(0, 0, 255)))
        );
    }

    #[test]
    fn inline_style_inherits_to_children() {
        let dom = parse_html(r#"<div style="color: green"><span>t</span></div>"#);
        let styled = style_tree(&dom, &parse_css(""));
        let span = &styled.children[0].children[0];
        assert_eq!(
            span.value("color"),
            Some(&Value::Color(Color::rgb(0, 128, 0)))
        );
    }

    #[test]
    fn attribute_selector_matches() {
        let dom = parse_html(r#"<input type="text" value="hi">"#);
        let ss = parse_css(r#"input[type="text"] { background-color: green; }"#);
        let styled = style_tree(&dom, &ss);
        let input = &styled.children[0];
        assert_eq!(
            input.value("background-color"),
            Some(&Value::Color(Color::rgb(0, 128, 0)))
        );
    }
}
