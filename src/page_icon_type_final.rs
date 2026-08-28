// ============================================================
// page_icon_type_final.rs — favicon MIME-hint aware resolver facade
// ============================================================

use std::collections::HashSet;

use crate::browser::Browser;
use crate::image::ImageCache;

pub use crate::page_icon_prev::{
    discover_page_icon_candidates, parse_icon_sizes, IconSizeHint, PageIconCandidate,
    PageIconResolution, PageIconResolveReport, ResolvedPageIcon,
};

/// How useful an authored `<link rel="icon" type="…">` hint is to this engine.
///
/// This is a ranking hint, not a hard filter. Sites sometimes send incorrect
/// MIME metadata, while the image stack already sniffs actual bytes. Even a
/// candidate declared as unsupported remains eligible after better hints fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IconTypeSupport {
    /// The declared MIME maps to a format the current raster stack decodes.
    Supported,
    /// No type was authored, or an image subtype is unknown to this engine.
    UnspecifiedOrUnknown,
    /// A known unsupported image format, or a non-image MIME type.
    Unsupported,
}

/// Classify an authored icon MIME hint. Parameters are ignored and the MIME
/// essence is matched case-insensitively.
pub fn icon_type_support(media_type: Option<&str>) -> IconTypeSupport {
    let Some(raw) = media_type.map(str::trim).filter(|value| !value.is_empty()) else {
        return IconTypeSupport::UnspecifiedOrUnknown;
    };
    let essence = raw
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();

    match essence.as_str() {
        // Formats currently accepted by the image facade. Some have multiple
        // historical MIME spellings in real favicon markup.
        "image/png"
        | "image/jpeg"
        | "image/jpg"
        | "image/bmp"
        | "image/x-bmp"
        | "image/x-ms-bmp"
        | "image/vnd.microsoft.icon"
        | "image/x-icon"
        | "image/x-ico"
        | "image/x-portable-pixmap"
        | "image/x-portable-graymap"
        | "image/x-portable-greymap"
        | "image/x-portable-bitmap"
        | "image/x-portable-anymap"
        | "image/x-portable-arbitrarymap"
        | "image/x-portable-floatmap" => IconTypeSupport::Supported,

        // Common browser-image MIME types that this engine does not yet decode.
        "image/svg+xml" | "image/gif" | "image/webp" | "image/avif" => {
            IconTypeSupport::Unsupported
        }
        other if other.starts_with("image/") => IconTypeSupport::UnspecifiedOrUnknown,
        _ => IconTypeSupport::Unsupported,
    }
}

/// Cache-backed page-icon resolver that considers MIME support before size.
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
        candidates.sort_by_key(|candidate| candidate_rank(candidate, preferred_width, preferred_height));

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

fn candidate_rank(
    candidate: &PageIconCandidate,
    preferred_width: u32,
    preferred_height: u32,
) -> (IconTypeSupport, u8, u64, usize) {
    let (size_class, distance) = size_rank(candidate, preferred_width, preferred_height);
    (
        icon_type_support(candidate.media_type.as_deref()),
        size_class,
        distance,
        candidate.ordinal,
    )
}

fn size_rank(candidate: &PageIconCandidate, preferred_width: u32, preferred_height: u32) -> (u8, u64) {
    let preferred_width = preferred_width.max(1);
    let preferred_height = preferred_height.max(1);

    if candidate.sizes.iter().any(|size| {
        matches!(
            size,
            IconSizeHint::Pixels { width, height }
                if *width == preferred_width && *height == preferred_height
        )
    }) {
        return (0, 0);
    }
    if candidate.sizes.contains(&IconSizeHint::Any) {
        return (1, 0);
    }

    let mut best_larger: Option<u64> = None;
    let mut best_smaller: Option<u64> = None;
    for size in &candidate.sizes {
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
        (2, distance)
    } else if candidate.sizes.is_empty() {
        (3, 0)
    } else {
        (4, best_smaller.unwrap_or(u64::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(media_type: Option<&str>, sizes: Vec<IconSizeHint>, ordinal: usize) -> PageIconCandidate {
        PageIconCandidate {
            href: format!("{ordinal}.png"),
            sizes,
            media_type: media_type.map(str::to_string),
            ordinal,
        }
    }

    #[test]
    fn classifies_supported_unknown_and_unsupported_hints() {
        assert_eq!(icon_type_support(Some("IMAGE/PNG; charset=binary")), IconTypeSupport::Supported);
        assert_eq!(icon_type_support(Some("image/vnd.microsoft.icon")), IconTypeSupport::Supported);
        assert_eq!(icon_type_support(None), IconTypeSupport::UnspecifiedOrUnknown);
        assert_eq!(icon_type_support(Some("image/x-future-format")), IconTypeSupport::UnspecifiedOrUnknown);
        assert_eq!(icon_type_support(Some("image/svg+xml")), IconTypeSupport::Unsupported);
        assert_eq!(icon_type_support(Some("text/plain")), IconTypeSupport::Unsupported);
    }

    #[test]
    fn type_support_precedes_size_within_candidate_ranking() {
        let unsupported_exact = candidate(
            Some("image/svg+xml"),
            vec![IconSizeHint::Pixels { width: 32, height: 32 }],
            0,
        );
        let supported_larger = candidate(
            Some("image/png"),
            vec![IconSizeHint::Pixels { width: 64, height: 64 }],
            1,
        );
        assert!(candidate_rank(&supported_larger, 32, 32) < candidate_rank(&unsupported_exact, 32, 32));
    }
}
