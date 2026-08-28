// ============================================================
// browser_chrome.rs — toolkit-neutral browser chrome state
// ============================================================

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::browser::Browser;
use crate::page_icon::{PageIconResolveReport, PageIconResolver, ResolvedPageIcon};

/// Stable-enough identity for deciding whether a frontend must replace its
/// currently displayed page icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageIconFingerprint {
    pub source: String,
    pub width: u32,
    pub height: u32,
    pub pixel_hash: u64,
    pub legacy_fallback: bool,
}

impl PageIconFingerprint {
    pub fn from_resolved(icon: &ResolvedPageIcon) -> Self {
        let mut hasher = DefaultHasher::new();
        icon.image.width.hash(&mut hasher);
        icon.image.height.hash(&mut hasher);
        icon.image.pixels.hash(&mut hasher);
        Self {
            source: icon.source.without_fragment().to_string(),
            width: icon.image.width,
            height: icon.image.height,
            pixel_hash: hasher.finish(),
            legacy_fallback: icon.is_legacy_fallback(),
        }
    }
}

/// Browser-chrome metadata derived from the live session/document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserChromeState {
    pub title: Option<String>,
    pub url: String,
    pub history_index: usize,
    pub history_len: usize,
    pub icon: Option<PageIconFingerprint>,
}

impl BrowserChromeState {
    pub fn status_line(&self) -> String {
        let position = format!("{}/{}", self.history_index + 1, self.history_len);
        match &self.title {
            Some(title) => format!("{title} — {} [{position}]", self.url),
            None => format!("{} [{position}]", self.url),
        }
    }
}

/// Which browser-chrome surfaces changed since the previous poll.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BrowserChromeChanges {
    pub title: bool,
    pub url: bool,
    pub history: bool,
    pub icon: bool,
}

impl BrowserChromeChanges {
    pub fn any(self) -> bool {
        self.title || self.url || self.history || self.icon
    }
}

/// One live chrome-state poll, including the decoded icon when available.
pub struct BrowserChromeUpdate {
    pub state: BrowserChromeState,
    pub changes: BrowserChromeChanges,
    pub icon: Option<ResolvedPageIcon>,
    pub icon_report: PageIconResolveReport,
}

/// Tracks browser-chrome state without depending on a window toolkit.
///
/// Each poll reads the live DOM title and re-discovers icon links. The nested
/// page-icon resolver memoizes resource loads/decodes, so polling can notice
/// script-driven `<title>` and `<link rel=icon>` mutations without repeatedly
/// fetching unchanged resources.
pub struct BrowserChromeTracker {
    icons: PageIconResolver,
    previous: Option<BrowserChromeState>,
    preferred_icon_width: u32,
    preferred_icon_height: u32,
}

impl Default for BrowserChromeTracker {
    fn default() -> Self {
        Self {
            icons: PageIconResolver::new(),
            previous: None,
            preferred_icon_width: 32,
            preferred_icon_height: 32,
        }
    }
}

impl BrowserChromeTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_icon_size(width: u32, height: u32) -> Self {
        Self {
            preferred_icon_width: width.max(1),
            preferred_icon_height: height.max(1),
            ..Self::default()
        }
    }

    pub fn icon_resolver(&self) -> &PageIconResolver {
        &self.icons
    }

    pub fn icon_resolver_mut(&mut self) -> &mut PageIconResolver {
        &mut self.icons
    }

    pub fn previous(&self) -> Option<&BrowserChromeState> {
        self.previous.as_ref()
    }

    pub fn reset(&mut self) {
        self.previous = None;
        self.icons = PageIconResolver::new();
    }

    pub fn poll(&mut self, browser: &Browser) -> BrowserChromeUpdate {
        let icon_resolution = self.icons.resolve(
            browser,
            self.preferred_icon_width,
            self.preferred_icon_height,
        );
        let icon_fingerprint = icon_resolution
            .icon
            .as_ref()
            .map(PageIconFingerprint::from_resolved);
        let state = BrowserChromeState {
            title: browser.document().title(),
            url: browser.url().to_string(),
            history_index: browser.history_index(),
            history_len: browser.history().len(),
            icon: icon_fingerprint,
        };

        let changes = match self.previous.as_ref() {
            None => BrowserChromeChanges {
                title: true,
                url: true,
                history: true,
                icon: true,
            },
            Some(previous) => BrowserChromeChanges {
                title: previous.title != state.title,
                url: previous.url != state.url,
                history: previous.history_index != state.history_index
                    || previous.history_len != state.history_len,
                icon: previous.icon != state.icon,
            },
        };
        self.previous = Some(state.clone());

        BrowserChromeUpdate {
            state,
            changes,
            icon: icon_resolution.icon,
            icon_report: icon_resolution.report,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_formats_the_same_title_bar_shape_as_browser_status() {
        let state = BrowserChromeState {
            title: Some("Example".into()),
            url: "demo:///index.html".into(),
            history_index: 1,
            history_len: 3,
            icon: None,
        };
        assert_eq!(state.status_line(), "Example — demo:///index.html [2/3]");
    }

    #[test]
    fn change_set_any_reports_only_real_changes() {
        assert!(!BrowserChromeChanges::default().any());
        assert!(BrowserChromeChanges {
            icon: true,
            ..BrowserChromeChanges::default()
        }
        .any());
    }
}
