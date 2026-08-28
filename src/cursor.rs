// ============================================================
// cursor.rs — computed CSS cursor resolution
// ============================================================

use crate::css::parser::Value;
use crate::document::{Document, PointerState};
use crate::dom::NodeType;
use crate::script::NodePath;
use crate::style::StyledNode;

/// A platform-neutral cursor requested by computed CSS.
///
/// This intentionally models CSS cursor keywords rather than any particular
/// window-system cursor enum. Frontends can translate it to their native UI
/// toolkit without leaking platform details into the style/layout layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorIcon {
    #[default]
    Auto,
    Default,
    None,
    ContextMenu,
    Help,
    Pointer,
    Progress,
    Wait,
    Cell,
    Crosshair,
    Text,
    VerticalText,
    Alias,
    Copy,
    Move,
    AllScroll,
    NoDrop,
    NotAllowed,
    Grab,
    Grabbing,
    ColResize,
    RowResize,
    NResize,
    EResize,
    SResize,
    WResize,
    NeResize,
    NwResize,
    SeResize,
    SwResize,
    EwResize,
    NsResize,
    NeswResize,
    NwseResize,
    ZoomIn,
    ZoomOut,
}

impl CursorIcon {
    pub fn from_css_keyword(keyword: &str) -> Option<Self> {
        Some(match keyword.trim().to_ascii_lowercase().as_str() {
            "auto" => Self::Auto,
            "default" => Self::Default,
            "none" => Self::None,
            "context-menu" => Self::ContextMenu,
            "help" => Self::Help,
            "pointer" => Self::Pointer,
            "progress" => Self::Progress,
            "wait" => Self::Wait,
            "cell" => Self::Cell,
            "crosshair" => Self::Crosshair,
            "text" => Self::Text,
            "vertical-text" => Self::VerticalText,
            "alias" => Self::Alias,
            "copy" => Self::Copy,
            "move" => Self::Move,
            "all-scroll" => Self::AllScroll,
            "no-drop" => Self::NoDrop,
            "not-allowed" => Self::NotAllowed,
            "grab" => Self::Grab,
            "grabbing" => Self::Grabbing,
            "col-resize" => Self::ColResize,
            "row-resize" => Self::RowResize,
            "n-resize" => Self::NResize,
            "e-resize" => Self::EResize,
            "s-resize" => Self::SResize,
            "w-resize" => Self::WResize,
            "ne-resize" => Self::NeResize,
            "nw-resize" => Self::NwResize,
            "se-resize" => Self::SeResize,
            "sw-resize" => Self::SwResize,
            "ew-resize" => Self::EwResize,
            "ns-resize" => Self::NsResize,
            "nesw-resize" => Self::NeswResize,
            "nwse-resize" => Self::NwseResize,
            "zoom-in" => Self::ZoomIn,
            "zoom-out" => Self::ZoomOut,
            _ => return None,
        })
    }

    pub const fn css_keyword(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Default => "default",
            Self::None => "none",
            Self::ContextMenu => "context-menu",
            Self::Help => "help",
            Self::Pointer => "pointer",
            Self::Progress => "progress",
            Self::Wait => "wait",
            Self::Cell => "cell",
            Self::Crosshair => "crosshair",
            Self::Text => "text",
            Self::VerticalText => "vertical-text",
            Self::Alias => "alias",
            Self::Copy => "copy",
            Self::Move => "move",
            Self::AllScroll => "all-scroll",
            Self::NoDrop => "no-drop",
            Self::NotAllowed => "not-allowed",
            Self::Grab => "grab",
            Self::Grabbing => "grabbing",
            Self::ColResize => "col-resize",
            Self::RowResize => "row-resize",
            Self::NResize => "n-resize",
            Self::EResize => "e-resize",
            Self::SResize => "s-resize",
            Self::WResize => "w-resize",
            Self::NeResize => "ne-resize",
            Self::NwResize => "nw-resize",
            Self::SeResize => "se-resize",
            Self::SwResize => "sw-resize",
            Self::EwResize => "ew-resize",
            Self::NsResize => "ns-resize",
            Self::NeswResize => "nesw-resize",
            Self::NwseResize => "nwse-resize",
            Self::ZoomIn => "zoom-in",
            Self::ZoomOut => "zoom-out",
        }
    }
}

pub fn cursor_for_styled_node(node: &StyledNode<'_>) -> CursorIcon {
    let specified = match node.value("cursor") {
        Some(Value::Keyword(keyword)) => CursorIcon::from_css_keyword(keyword).unwrap_or(CursorIcon::Auto),
        _ => CursorIcon::Auto,
    };
    if specified != CursorIcon::Auto {
        return specified;
    }
    auto_cursor_for_node(node)
}

fn auto_cursor_for_node(node: &StyledNode<'_>) -> CursorIcon {
    match &node.node.node_type {
        NodeType::Text(text) if !text.trim().is_empty() => CursorIcon::Text,
        NodeType::Element(element) => match element.tag_name.as_str() {
            "textarea" => CursorIcon::Text,
            "input" => match element.get_attr("type").unwrap_or("text").to_ascii_lowercase().as_str() {
                "text" | "search" | "email" | "url" | "tel" | "password" | "number" => CursorIcon::Text,
                _ => CursorIcon::Default,
            },
            _ => CursorIcon::Default,
        },
        _ => CursorIcon::Default,
    }
}

pub fn styled_node_at_path<'a, 'n>(root: &'a StyledNode<'n>, path: &[usize]) -> Option<&'a StyledNode<'n>> {
    let mut current = root;
    for &index in path {
        current = current.children.get(index)?;
    }
    Some(current)
}

pub fn cursor_for_path(document: &Document, path: &[usize], viewport_width: f32, pointer: &PointerState) -> Option<CursorIcon> {
    let styled = document.style_tree(viewport_width, pointer);
    styled_node_at_path(&styled, path).map(cursor_for_styled_node)
}

pub fn cursor_at_point(document: &Document, x: f32, y: f32, viewport_width: f32, pointer: &PointerState) -> CursorIcon {
    let Some(path) = document.hit_test(x, y, viewport_width) else {
        return CursorIcon::Default;
    };
    let mut interaction = pointer.clone();
    interaction.hovered = Some(path.clone());
    cursor_for_path(document, &path, viewport_width, &interaction).unwrap_or(CursorIcon::Default)
}

pub fn cursor_hit_test(document: &Document, x: f32, y: f32, viewport_width: f32, pointer: &PointerState) -> (Option<NodePath>, CursorIcon) {
    let path = document.hit_test(x, y, viewport_width);
    let Some(ref path) = path else {
        return (None, CursorIcon::Default);
    };
    let mut interaction = pointer.clone();
    interaction.hovered = Some(path.clone());
    let icon = cursor_for_path(document, path, viewport_width, &interaction).unwrap_or(CursorIcon::Default);
    (Some(path.clone()), icon)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{MemoryLoader, Url};
    use crate::script::dom_api;

    fn document(html: &str) -> Document {
        let url = Url::parse("demo:///cursor.html").unwrap();
        Document::from_html(html, &url, &MemoryLoader::new())
    }

    #[test]
    fn maps_css_cursor_keyword_family() {
        for (css, expected) in [
            ("pointer", CursorIcon::Pointer),
            ("text", CursorIcon::Text),
            ("crosshair", CursorIcon::Crosshair),
            ("move", CursorIcon::Move),
            ("all-scroll", CursorIcon::AllScroll),
            ("not-allowed", CursorIcon::NotAllowed),
            ("grab", CursorIcon::Grab),
            ("nwse-resize", CursorIcon::NwseResize),
            ("zoom-in", CursorIcon::ZoomIn),
        ] {
            assert_eq!(CursorIcon::from_css_keyword(css), Some(expected));
            assert_eq!(expected.css_keyword(), css);
        }
        assert_eq!(CursorIcon::from_css_keyword("definitely-not-a-cursor"), None);
    }

    #[test]
    fn user_agent_rules_resolve_links_and_buttons_to_pointer() {
        let doc = document("<a id='link' href='/next'>next</a><button id='button'>go</button>");
        for selector in ["#link", "#button"] {
            let path = dom_api::query_selector(&doc.dom, &[], selector).expect("element path");
            assert_eq!(cursor_for_path(&doc, &path, 800.0, &PointerState::default()), Some(CursorIcon::Pointer));
        }
    }

    #[test]
    fn auto_cursor_uses_text_shape_for_text_entry_controls() {
        let doc = document("<input id='text' type='text'><input id='box' type='checkbox'><textarea id='ta'></textarea>");
        for selector in ["#text", "#ta"] {
            let path = dom_api::query_selector(&doc.dom, &[], selector).unwrap();
            assert_eq!(cursor_for_path(&doc, &path, 800.0, &PointerState::default()), Some(CursorIcon::Text));
        }
        let checkbox = dom_api::query_selector(&doc.dom, &[], "#box").unwrap();
        assert_eq!(cursor_for_path(&doc, &checkbox, 800.0, &PointerState::default()), Some(CursorIcon::Default));
    }

    #[test]
    fn cursor_inherits_through_the_existing_cascade() {
        let doc = document("<style>#parent { cursor: move; }</style><div id='parent'><span id='child'>child</span></div>");
        let child = dom_api::query_selector(&doc.dom, &[], "#child").unwrap();
        assert_eq!(cursor_for_path(&doc, &child, 800.0, &PointerState::default()), Some(CursorIcon::Move));
    }

    #[test]
    fn all_scroll_keyword_flows_through_the_computed_style_runtime() {
        let doc = document("<style>#target { cursor: all-scroll; }</style><div id='target'>drag</div>");
        let target = dom_api::query_selector(&doc.dom, &[], "#target").unwrap();
        assert_eq!(cursor_for_path(&doc, &target, 800.0, &PointerState::default()), Some(CursorIcon::AllScroll));
    }

    #[test]
    fn hover_state_can_change_the_resolved_cursor() {
        let doc = document("<style>#target { cursor: default; } #target:hover { cursor: crosshair; }</style><div id='target'>target</div>");
        let target = dom_api::query_selector(&doc.dom, &[], "#target").unwrap();
        assert_eq!(cursor_for_path(&doc, &target, 800.0, &PointerState::default()), Some(CursorIcon::Default));
        let pointer = PointerState { hovered: Some(target.clone()), ..PointerState::default() };
        assert_eq!(cursor_for_path(&doc, &target, 800.0, &pointer), Some(CursorIcon::Crosshair));
    }
}
