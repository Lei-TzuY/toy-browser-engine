// ============================================================
// cursor_assets.rs — CSS cursor URL resource resolution
// ============================================================

use std::rc::Rc;

use crate::browser::Browser;
use crate::css::parser::Value;
use crate::cursor::{cursor_for_styled_node, styled_node_at_path, CursorIcon};
use crate::document::PointerState;
use crate::image::{CursorCache, CursorImage};
use crate::net::Url;
use crate::script::NodePath;

/// The cursor a frontend should display after CSS/style/resource resolution.
pub enum ResolvedCursor {
    System(CursorIcon),
    Image {
        cursor: Rc<CursorImage>,
        source: Url,
        fallback: CursorIcon,
    },
}

impl ResolvedCursor {
    pub fn fallback_icon(&self) -> CursorIcon {
        match self {
            Self::System(icon) => *icon,
            Self::Image { fallback, .. } => *fallback,
        }
    }

    pub fn image(&self) -> Option<&CursorImage> {
        match self {
            Self::Image { cursor, .. } => Some(cursor.as_ref()),
            Self::System(_) => None,
        }
    }

    pub fn source(&self) -> Option<&Url> {
        match self {
            Self::Image { source, .. } => Some(source),
            Self::System(_) => None,
        }
    }
}

/// Cache-backed resolver for CSS image cursors.
///
/// The current CSS parser preserves unknown functions as `Value::Keyword`, so
/// a declaration such as `cursor: url(pointer.cur)` already reaches computed
/// style intact. This layer recognizes that representation and reuses the CUR
/// decoder/cache added by the image stack.
#[derive(Default, Clone)]
pub struct CursorResolver {
    cache: CursorCache,
}

impl CursorResolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cache(&self) -> &CursorCache {
        &self.cache
    }

    pub fn cache_mut(&mut self) -> &mut CursorCache {
        &mut self.cache
    }

    /// Resolve a cursor for a known DOM path. Relative image references use
    /// the document base URL, including `<base href>` when present.
    pub fn resolve_for_path(
        &mut self,
        browser: &Browser,
        path: &[usize],
        viewport_width: f32,
        pointer: &PointerState,
    ) -> Option<ResolvedCursor> {
        let styled = browser.document().style_tree(viewport_width, pointer);
        let node = styled_node_at_path(&styled, path)?;
        Some(self.resolve_for_styled_node(browser, node))
    }

    /// Hit-test and resolve a cursor resource in one pointer-move operation.
    /// The target is installed as `:hover` before style resolution.
    pub fn resolve_at_point(
        &mut self,
        browser: &Browser,
        x: f32,
        y: f32,
        viewport_width: f32,
        pointer: &PointerState,
    ) -> (Option<NodePath>, ResolvedCursor) {
        let Some(path) = browser.document().hit_test(x, y, viewport_width) else {
            return (None, ResolvedCursor::System(CursorIcon::Default));
        };
        let mut interaction = pointer.clone();
        interaction.hovered = Some(path.clone());
        let resolved = self
            .resolve_for_path(browser, &path, viewport_width, &interaction)
            .unwrap_or(ResolvedCursor::System(CursorIcon::Default));
        (Some(path), resolved)
    }

    fn resolve_for_styled_node(
        &mut self,
        browser: &Browser,
        node: &crate::style::StyledNode<'_>,
    ) -> ResolvedCursor {
        let fallback = cursor_for_styled_node(node);
        let Some(Value::Keyword(raw)) = node.value("cursor") else {
            return ResolvedCursor::System(fallback);
        };
        let Some(reference) = parse_cursor_url(raw) else {
            return ResolvedCursor::System(fallback);
        };
        let Some(url) = browser.document().resolve(&reference) else {
            return ResolvedCursor::System(CursorIcon::Default);
        };

        match self.cache.fetch(&url, browser.loader()) {
            Ok(cursor) => ResolvedCursor::Image {
                cursor,
                source: url,
                fallback: CursorIcon::Default,
            },
            Err(_) => ResolvedCursor::System(CursorIcon::Default),
        }
    }
}

/// Parse the single-image subset currently preserved by the CSS value parser:
/// `url(foo.cur)`, `url("foo.cur")`, or `url('foo.cur')`.
///
/// Full CSS cursor candidate lists (`url(a.cur), url(b.cur), pointer`) need a
/// dedicated list-valued CSS AST and remain a follow-up rather than being
/// silently approximated here.
pub fn parse_cursor_url(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() < 5 || !value[..4].eq_ignore_ascii_case("url(") || !value.ends_with(')') {
        return None;
    }
    let mut inner = value[4..value.len() - 1].trim();
    if inner.len() >= 2 {
        let first = inner.as_bytes()[0];
        let last = inner.as_bytes()[inner.len() - 1];
        if (first == b'\'' && last == b'\'') || (first == b'"' && last == b'"') {
            inner = &inner[1..inner.len() - 1];
        }
    }
    let inner = inner.trim();
    if inner.is_empty() || inner.contains('\n') || inner.contains('\r') {
        return None;
    }
    Some(inner.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::MemoryLoader;
    use crate::script::dom_api;

    fn dib24(rgb: [u8; 3]) -> Vec<u8> {
        let mut out = vec![0u8; 48];
        out[0..4].copy_from_slice(&40u32.to_le_bytes());
        out[4..8].copy_from_slice(&1i32.to_le_bytes());
        out[8..12].copy_from_slice(&2i32.to_le_bytes());
        out[12..14].copy_from_slice(&1u16.to_le_bytes());
        out[14..16].copy_from_slice(&24u16.to_le_bytes());
        out[40..43].copy_from_slice(&[rgb[2], rgb[1], rgb[0]]);
        out
    }

    fn cur(rgb: [u8; 3]) -> Vec<u8> {
        let payload = dib24(rgb);
        let mut out = vec![0u8; 22];
        out[2..4].copy_from_slice(&2u16.to_le_bytes());
        out[4..6].copy_from_slice(&1u16.to_le_bytes());
        out[6] = 1;
        out[7] = 1;
        out[10..12].copy_from_slice(&0u16.to_le_bytes());
        out[12..14].copy_from_slice(&0u16.to_le_bytes());
        out[14..18].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        out[18..22].copy_from_slice(&22u32.to_le_bytes());
        out.extend_from_slice(&payload);
        out
    }

    fn browser_with_cursor(css_value: &str, include_cursor: bool) -> Browser {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///index.html",
            format!("<style>#target {{ cursor: {css_value}; }}</style><div id='target'>target</div>").into_bytes(),
        );
        if include_cursor {
            loader.insert("demo:///pointer.cur", cur([10, 20, 30]));
        }
        Browser::open(
            Box::new(loader),
            &Url::parse("demo:///index.html").unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn parses_quoted_and_unquoted_cursor_urls() {
        assert_eq!(parse_cursor_url("url(pointer.cur)"), Some("pointer.cur".into()));
        assert_eq!(parse_cursor_url("URL(\"pointer.cur\")"), Some("pointer.cur".into()));
        assert_eq!(parse_cursor_url("url('icons/p.cur')"), Some("icons/p.cur".into()));
        assert_eq!(parse_cursor_url("pointer"), None);
        assert_eq!(parse_cursor_url("url()"), None);
    }

    #[test]
    fn resolves_relative_cur_resource_and_preserves_hotspot() {
        let browser = browser_with_cursor("url(pointer.cur)", true);
        let path = dom_api::query_selector(&browser.document().dom, &[], "#target").unwrap();
        let mut resolver = CursorResolver::new();
        let resolved = resolver
            .resolve_for_path(&browser, &path, 800.0, &PointerState::default())
            .unwrap();
        let image = resolved.image().expect("custom cursor image");
        assert_eq!(image.hotspot(), (0, 0));
        assert_eq!(image.image.pixel(0, 0), [10, 20, 30, 255]);
        assert_eq!(resolved.source().unwrap().to_string(), "demo:///pointer.cur");
        assert_eq!(resolver.cache().len(), 1);

        // A second resolution is served by CursorCache rather than re-decoding.
        let _ = resolver.resolve_for_path(&browser, &path, 800.0, &PointerState::default());
        assert_eq!(resolver.cache().len(), 1);
    }

    #[test]
    fn missing_cursor_resource_falls_back_safely() {
        let browser = browser_with_cursor("url(pointer.cur)", false);
        let path = dom_api::query_selector(&browser.document().dom, &[], "#target").unwrap();
        let mut resolver = CursorResolver::new();
        let resolved = resolver
            .resolve_for_path(&browser, &path, 800.0, &PointerState::default())
            .unwrap();
        assert!(resolved.image().is_none());
        assert_eq!(resolved.fallback_icon(), CursorIcon::Default);
        assert_eq!(resolver.cache().len(), 1);
    }
}
