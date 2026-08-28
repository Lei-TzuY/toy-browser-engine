// ============================================================
// page_icon.rs — document favicon discovery and resolution
// ============================================================

use std::collections::HashSet;
use std::rc::Rc;

use crate::browser::Browser;
use crate::document::Document;
use crate::dom::{Node, NodeType};
use crate::image::{ImageCache, RasterImage};
use crate::net::Url;

/// One token from a `<link rel="icon" sizes="…">` declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconSizeHint {
    /// A scalable/unspecified-size resource (`sizes="any"`).
    Any,
    /// A concrete pixel size such as `32x32`.
    Pixels { width: u32, height: u32 },
}

/// A page-icon candidate discovered in the current DOM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageIconCandidate {
    pub href: String,
    pub sizes: Vec<IconSizeHint>,
    pub media_type: Option<String>,
    /// Document order among discovered icon relations.
    pub ordinal: usize,
}

impl PageIconCandidate {
    fn rank(&self, preferred_width: u32, preferred_height: u32) -> (u8, u64, usize) {
        let preferred_width = preferred_width.max(1);
        let preferred_height = preferred_height.max(1);

        if self.sizes.iter().any(|size| {
            matches!(
                size,
                IconSizeHint::Pixels { width, height }
                    if *width == preferred_width && *height == preferred_height
            )
        }) {
            return (0, 0, self.ordinal);
        }
        if self.sizes.contains(&IconSizeHint::Any) {
            return (1, 0, self.ordinal);
        }

        let mut best_larger: Option<u64> = None;
        let mut best_smaller: Option<u64> = None;
        for size in &self.sizes {
            let IconSizeHint::Pixels { width, height } = *size else {
                continue;
            };
            let distance = u64::from(width.abs_diff(preferred_width))
                + u64::from(height.abs_diff(preferred_height));
            if width >= preferred_width && height >= preferred_height {
                best_larger = Some(best_larger.map_or(distance, |old| old.min(distance)));
            } else {
                best_smaller = Some(best_smaller.map_or(distance, |old| old.min(distance)));
            }
        }

        if let Some(distance) = best_larger {
            (2, distance, self.ordinal)
        } else if self.sizes.is_empty() {
            // No sizes declaration is deliberately treated as unknown rather
            // than tiny; it remains preferable to an explicitly undersized icon.
            (3, 0, self.ordinal)
        } else {
            (4, best_smaller.unwrap_or(u64::MAX), self.ordinal)
        }
    }
}

/// A decoded page icon selected for browser chrome.
#[derive(Clone)]
pub struct ResolvedPageIcon {
    pub image: Rc<RasterImage>,
    pub source: Url,
    /// `None` for the legacy `/favicon.ico` fallback.
    pub candidate: Option<PageIconCandidate>,
}

impl ResolvedPageIcon {
    pub fn is_legacy_fallback(&self) -> bool {
        self.candidate.is_none()
    }
}

/// Diagnostics for one page-icon resolution pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PageIconResolveReport {
    pub discovered: usize,
    pub attempted: usize,
    pub failed: usize,
    pub legacy_fallback_attempted: bool,
}

/// Result of resolving browser-chrome icon state for one document.
pub struct PageIconResolution {
    pub icon: Option<ResolvedPageIcon>,
    pub report: PageIconResolveReport,
}

/// Cache-backed page-icon resolver.
///
/// The resolver discovers the *current* DOM each time, so script-added icon
/// links participate without changing `Document`. Decoded bytes are memoized by
/// the existing generic `ImageCache` and therefore reuse the complete PNG/ICO/
/// BMP/JPEG/PNM image stack.
pub struct PageIconResolver {
    cache: ImageCache,
    legacy_fallback: bool,
}

impl Default for PageIconResolver {
    fn default() -> Self {
        Self {
            cache: ImageCache::new(),
            legacy_fallback: true,
        }
    }
}

impl PageIconResolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cache(&self) -> &ImageCache {
        &self.cache
    }

    pub fn cache_mut(&mut self) -> &mut ImageCache {
        &mut self.cache
    }

    pub fn set_legacy_fallback(&mut self, enabled: bool) {
        self.legacy_fallback = enabled;
    }

    pub fn resolve(
        &mut self,
        browser: &Browser,
        preferred_width: u32,
        preferred_height: u32,
    ) -> PageIconResolution {
        let mut candidates = discover_page_icon_candidates(browser.document());
        let mut report = PageIconResolveReport {
            discovered: candidates.len(),
            ..PageIconResolveReport::default()
        };
        candidates.sort_by_key(|candidate| candidate.rank(preferred_width, preferred_height));

        let mut attempted_urls = HashSet::new();
        for candidate in candidates {
            let Some(url) = browser.document().resolve(&candidate.href) else {
                report.attempted += 1;
                report.failed += 1;
                continue;
            };
            let key = url.without_fragment().to_string();
            if !attempted_urls.insert(key) {
                continue;
            }
            report.attempted += 1;
            match self.cache.fetch(&url, browser.loader()) {
                Ok(image) => {
                    return PageIconResolution {
                        icon: Some(ResolvedPageIcon {
                            image,
                            source: url,
                            candidate: Some(candidate),
                        }),
                        report,
                    };
                }
                Err(_) => report.failed += 1,
            }
        }

        if self.legacy_fallback {
            if let Ok(url) = browser.document().url.join("/favicon.ico") {
                let key = url.without_fragment().to_string();
                if attempted_urls.insert(key) {
                    report.legacy_fallback_attempted = true;
                    report.attempted += 1;
                    match self.cache.fetch(&url, browser.loader()) {
                        Ok(image) => {
                            return PageIconResolution {
                                icon: Some(ResolvedPageIcon {
                                    image,
                                    source: url,
                                    candidate: None,
                                }),
                                report,
                            };
                        }
                        Err(_) => report.failed += 1,
                    }
                }
            }
        }

        PageIconResolution { icon: None, report }
    }
}

/// Discover `<link rel="icon" href="…">` candidates in DOM/document order.
///
/// `rel` is a whitespace-separated case-insensitive token set, so forms such
/// as `rel="shortcut icon"` work naturally. Non-standard relations such as
/// `apple-touch-icon` are intentionally not treated as the standard `icon`
/// relation here.
pub fn discover_page_icon_candidates(document: &Document) -> Vec<PageIconCandidate> {
    let mut out = Vec::new();
    discover_in_node(&document.dom, &mut out);
    out
}

fn discover_in_node(node: &Node, out: &mut Vec<PageIconCandidate>) {
    if let NodeType::Element(element) = &node.node_type {
        if element.tag_name == "link" && rel_contains_icon(element.get_attr("rel").unwrap_or("")) {
            if let Some(href) = element.get_attr("href").map(str::trim).filter(|href| !href.is_empty()) {
                let ordinal = out.len();
                out.push(PageIconCandidate {
                    href: href.to_string(),
                    sizes: parse_icon_sizes(element.get_attr("sizes").unwrap_or("")),
                    media_type: element
                        .get_attr("type")
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string),
                    ordinal,
                });
            }
        }
    }
    for child in &node.children {
        discover_in_node(child, out);
    }
}

fn rel_contains_icon(rel: &str) -> bool {
    rel.split_ascii_whitespace()
        .any(|token| token.eq_ignore_ascii_case("icon"))
}

/// Parse the useful subset of the HTML `sizes` attribute.
pub fn parse_icon_sizes(value: &str) -> Vec<IconSizeHint> {
    value
        .split_ascii_whitespace()
        .filter_map(|token| {
            if token.eq_ignore_ascii_case("any") {
                return Some(IconSizeHint::Any);
            }
            let (width, height) = token
                .split_once('x')
                .or_else(|| token.split_once('X'))?;
            let width = width.parse::<u32>().ok()?;
            let height = height.parse::<u32>().ok()?;
            (width > 0 && height > 0).then_some(IconSizeHint::Pixels { width, height })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::MemoryLoader;

    fn document(html: &str) -> Document {
        let url = Url::parse("demo:///pages/index.html").unwrap();
        Document::from_html(html, &url, &MemoryLoader::new())
    }

    #[test]
    fn discovers_standard_icon_relations_in_document_order() {
        let doc = document(
            "<head>\
               <link rel='stylesheet' href='site.css'>\
               <link rel='shortcut ICON' href='a.ico' sizes='16x16 32X32'>\
               <link rel='apple-touch-icon' href='touch.png'>\
               <link rel='icon alternate' href='b.png' type='image/png'>\
             </head>",
        );
        let icons = discover_page_icon_candidates(&doc);
        assert_eq!(icons.len(), 2);
        assert_eq!(icons[0].href, "a.ico");
        assert_eq!(
            icons[0].sizes,
            vec![
                IconSizeHint::Pixels { width: 16, height: 16 },
                IconSizeHint::Pixels { width: 32, height: 32 },
            ]
        );
        assert_eq!(icons[1].href, "b.png");
        assert_eq!(icons[1].media_type.as_deref(), Some("image/png"));
    }

    #[test]
    fn size_parser_ignores_invalid_tokens() {
        assert_eq!(
            parse_icon_sizes("any 32x32 nope 0x16 64X48"),
            vec![
                IconSizeHint::Any,
                IconSizeHint::Pixels { width: 32, height: 32 },
                IconSizeHint::Pixels { width: 64, height: 48 },
            ]
        );
    }

    #[test]
    fn candidate_rank_prefers_exact_then_scalable_then_larger() {
        let candidate = |sizes| PageIconCandidate {
            href: "x".into(),
            sizes,
            media_type: None,
            ordinal: 0,
        };
        assert!(
            candidate(vec![IconSizeHint::Pixels { width: 32, height: 32 }]).rank(32, 32)
                < candidate(vec![IconSizeHint::Any]).rank(32, 32)
        );
        assert!(
            candidate(vec![IconSizeHint::Any]).rank(32, 32)
                < candidate(vec![IconSizeHint::Pixels { width: 64, height: 64 }]).rank(32, 32)
        );
        assert!(
            candidate(vec![IconSizeHint::Pixels { width: 64, height: 64 }]).rank(32, 32)
                < candidate(vec![IconSizeHint::Pixels { width: 16, height: 16 }]).rank(32, 32)
        );
    }
}
