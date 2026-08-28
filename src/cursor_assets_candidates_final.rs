// ============================================================
// cursor_assets_candidates_final.rs — full CSS cursor candidates
// ============================================================

use std::collections::HashSet;
use std::rc::Rc;

use crate::browser::Browser;
use crate::css::parser::{decode_preserved_cursor_value, Value};
use crate::cursor::{cursor_for_styled_node, styled_node_at_path, CursorIcon};
use crate::document::PointerState;
use crate::image::{CursorCache, CursorImage};
use crate::net::Url;
use crate::script::NodePath;

/// One image candidate in a CSS `cursor` value.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorImageCandidate {
    pub reference: String,
    /// Optional authored CSS hotspot coordinates.
    pub hotspot: Option<(f32, f32)>,
}

/// Parsed CSS cursor candidate list.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorValue {
    pub images: Vec<CursorImageCandidate>,
    pub fallback: CursorIcon,
}

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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CursorPreloadReport {
    pub discovered: usize,
    pub loaded: usize,
    pub failed: usize,
}

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

    /// Preload every unique URL candidate in every authored cursor list.
    pub fn preload_stylesheet(&mut self, browser: &Browser) -> CursorPreloadReport {
        let mut report = CursorPreloadReport::default();
        let mut seen = HashSet::new();

        for rule in &browser.document().stylesheet.rules {
            for declaration in &rule.declarations {
                if declaration.name != "cursor" {
                    continue;
                }
                let Value::Keyword(raw) = &declaration.value else {
                    continue;
                };
                let Some(value) = parse_cursor_value(raw) else {
                    continue;
                };
                for candidate in value.images {
                    let Some(url) = browser.document().resolve(&candidate.reference) else {
                        report.discovered += 1;
                        report.failed += 1;
                        continue;
                    };
                    let key = url.without_fragment().to_string();
                    if !seen.insert(key) {
                        continue;
                    }
                    report.discovered += 1;
                    match self.cache.fetch(&url, browser.loader()) {
                        Ok(_) => report.loaded += 1,
                        Err(_) => report.failed += 1,
                    }
                }
            }
        }
        report
    }

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
        let computed = cursor_for_styled_node(node);
        let Some(Value::Keyword(raw)) = node.value("cursor") else {
            return ResolvedCursor::System(computed);
        };
        let Some(value) = parse_cursor_value(raw) else {
            return ResolvedCursor::System(computed);
        };

        let fallback = if value.fallback == CursorIcon::Auto {
            computed
        } else {
            value.fallback
        };

        for candidate in value.images {
            let Some(url) = browser.document().resolve(&candidate.reference) else {
                continue;
            };
            let Ok(cursor) = self.cache.fetch(&url, browser.loader()) else {
                continue;
            };
            let cursor = apply_hotspot_override(cursor, candidate.hotspot);
            return ResolvedCursor::Image {
                cursor,
                source: url,
                fallback,
            };
        }

        ResolvedCursor::System(fallback)
    }
}

/// Parse both the legacy single-url subset and a full comma-separated cursor
/// candidate list preserved by the cursor-aware CSS facade.
pub fn parse_cursor_value(value: &str) -> Option<CursorValue> {
    let owned;
    let raw = if let Some(decoded) = decode_preserved_cursor_value(value) {
        owned = decoded;
        owned.as_str()
    } else {
        value.trim()
    };

    let parts = split_top_level_commas(raw)?;
    if parts.is_empty() {
        return None;
    }

    if parts.len() == 1 {
        if let Some(image) = parse_image_candidate(parts[0]) {
            return Some(CursorValue {
                images: vec![image],
                fallback: CursorIcon::Default,
            });
        }
        return CursorIcon::from_css_keyword(parts[0]).map(|fallback| CursorValue {
            images: Vec::new(),
            fallback,
        });
    }

    let fallback = CursorIcon::from_css_keyword(parts.last()?.trim())?;
    let mut images = Vec::with_capacity(parts.len() - 1);
    for part in &parts[..parts.len() - 1] {
        images.push(parse_image_candidate(part)?);
    }
    if images.is_empty() {
        return None;
    }
    Some(CursorValue { images, fallback })
}

/// Compatibility helper retained for callers of the old single-url facade.
pub fn parse_cursor_url(value: &str) -> Option<String> {
    let parsed = parse_cursor_value(value)?;
    (parsed.images.len() == 1)
        .then(|| parsed.images[0].reference.clone())
}

fn parse_image_candidate(input: &str) -> Option<CursorImageCandidate> {
    let input = input.trim();
    let prefix = input.get(..4)?;
    if !prefix.eq_ignore_ascii_case("url(") {
        return None;
    }

    let close = find_function_close(input, 3)?;
    let mut inner = input.get(4..close)?.trim();
    if inner.len() >= 2 {
        let bytes = inner.as_bytes();
        if (bytes[0] == b'\'' && bytes[inner.len() - 1] == b'\'')
            || (bytes[0] == b'"' && bytes[inner.len() - 1] == b'"')
        {
            inner = inner.get(1..inner.len() - 1)?;
        }
    }
    if inner.trim().is_empty() || inner.contains('\n') || inner.contains('\r') {
        return None;
    }

    let tail = input.get(close + 1..)?.trim();
    let hotspot = if tail.is_empty() {
        None
    } else {
        let mut numbers = tail.split_whitespace();
        let x: f32 = numbers.next()?.parse().ok()?;
        let y: f32 = numbers.next()?.parse().ok()?;
        if numbers.next().is_some() || !x.is_finite() || !y.is_finite() {
            return None;
        }
        Some((x, y))
    };

    Some(CursorImageCandidate {
        reference: inner.trim().to_string(),
        hotspot,
    })
}

fn find_function_close(input: &str, open: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    if bytes.get(open) != Some(&b'(') {
        return None;
    }
    let mut depth = 1usize;
    let mut quote: Option<u8> = None;
    let mut i = open + 1;
    while i < bytes.len() {
        let byte = bytes[i];
        if let Some(q) = quote {
            if byte == b'\\' {
                i += 2;
                continue;
            }
            if byte == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn split_top_level_commas(input: &str) -> Option<Vec<&str>> {
    let bytes = input.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    let mut quote: Option<u8> = None;
    let mut i = 0usize;

    while i < bytes.len() {
        let byte = bytes[i];
        if let Some(q) = quote {
            if byte == b'\\' {
                i += 2;
                continue;
            }
            if byte == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'(' => depth += 1,
            b')' => depth = depth.checked_sub(1)?,
            b',' if depth == 0 => {
                let part = input.get(start..i)?.trim();
                if part.is_empty() {
                    return None;
                }
                parts.push(part);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if quote.is_some() || depth != 0 {
        return None;
    }
    let part = input.get(start..)?.trim();
    if part.is_empty() {
        return None;
    }
    parts.push(part);
    Some(parts)
}

fn apply_hotspot_override(
    cursor: Rc<CursorImage>,
    hotspot: Option<(f32, f32)>,
) -> Rc<CursorImage> {
    let Some((x, y)) = hotspot else {
        return cursor;
    };
    let max_x = cursor.image.width.saturating_sub(1).min(u32::from(u16::MAX));
    let max_y = cursor.image.height.saturating_sub(1).min(u32::from(u16::MAX));
    let clamp = |value: f32, max: u32| -> u16 {
        value.floor().clamp(0.0, max as f32) as u16
    };
    Rc::new(CursorImage {
        image: cursor.image.clone(),
        hotspot_x: clamp(x, max_x),
        hotspot_y: clamp(y, max_y),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiple_urls_data_commas_hotspot_and_fallback() {
        let value = parse_cursor_value(
            "url(a.cur), url(\"data:image/png;base64,AAAA\") 4.8 5.2, crosshair",
        )
        .unwrap();
        assert_eq!(value.images.len(), 2);
        assert_eq!(value.images[0].reference, "a.cur");
        assert_eq!(value.images[1].reference, "data:image/png;base64,AAAA");
        assert_eq!(value.images[1].hotspot, Some((4.8, 5.2)));
        assert_eq!(value.fallback, CursorIcon::Crosshair);
    }

    #[test]
    fn keeps_legacy_single_url_and_keyword_forms() {
        let one = parse_cursor_value("url(pointer.png)").unwrap();
        assert_eq!(one.images.len(), 1);
        assert_eq!(one.fallback, CursorIcon::Default);
        let keyword = parse_cursor_value("pointer").unwrap();
        assert!(keyword.images.is_empty());
        assert_eq!(keyword.fallback, CursorIcon::Pointer);
    }

    #[test]
    fn rejects_malformed_lists_and_hotspots() {
        assert!(parse_cursor_value("url(a), nope").is_none());
        assert!(parse_cursor_value("url(a) 1, pointer").is_none());
        assert!(parse_cursor_value("url(a),, pointer").is_none());
    }
}
