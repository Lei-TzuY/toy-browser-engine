//! Fetch request redirect-mode semantics.
//!
//! This module models the small, transport-neutral portion of Fetch that
//! decides what a script fetch should do when an HTTP redirect response is
//! encountered. Keeping it separate from the wire backend lets Request/Fetch
//! plumbing reuse one standards-focused decision point.

/// Fetch's request redirect mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FetchRedirectMode {
    /// Follow redirect responses. This is Fetch's default.
    #[default]
    Follow,
    /// Turn a redirect response into a network error.
    Error,
    /// Expose an opaque-redirect filtered response instead of following it.
    Manual,
}

impl FetchRedirectMode {
    /// Parse the Web IDL string value accepted by `RequestInit.redirect`.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "follow" => Some(Self::Follow),
            "error" => Some(Self::Error),
            "manual" => Some(Self::Manual),
            _ => None,
        }
    }

    /// Return the script-visible string value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Follow => "follow",
            Self::Error => "error",
            Self::Manual => "manual",
        }
    }
}

/// The action Fetch takes when a redirect response is encountered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectResponseDisposition {
    Follow,
    NetworkError,
    OpaqueRedirect,
}

/// Return whether `status` is an HTTP redirect status handled by Fetch.
pub const fn is_fetch_redirect_status(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// Decide how a redirect response is handled for a request redirect mode.
///
/// Non-redirect statuses return `None`, so callers can keep their ordinary
/// response path unchanged.
pub const fn redirect_response_disposition(
    mode: FetchRedirectMode,
    status: u16,
) -> Option<RedirectResponseDisposition> {
    if !is_fetch_redirect_status(status) {
        return None;
    }

    Some(match mode {
        FetchRedirectMode::Follow => RedirectResponseDisposition::Follow,
        FetchRedirectMode::Error => RedirectResponseDisposition::NetworkError,
        FetchRedirectMode::Manual => RedirectResponseDisposition::OpaqueRedirect,
    })
}

/// Fetch forbids combining `mode: "no-cors"` with `redirect` modes other than
/// `follow`.
pub const fn no_cors_redirect_mode_is_valid(mode: FetchRedirectMode) -> bool {
    matches!(mode, FetchRedirectMode::Follow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_mode_defaults_to_follow() {
        assert_eq!(FetchRedirectMode::default(), FetchRedirectMode::Follow);
    }

    #[test]
    fn parses_only_exact_web_idl_values() {
        assert_eq!(FetchRedirectMode::parse("follow"), Some(FetchRedirectMode::Follow));
        assert_eq!(FetchRedirectMode::parse("error"), Some(FetchRedirectMode::Error));
        assert_eq!(FetchRedirectMode::parse("manual"), Some(FetchRedirectMode::Manual));
        assert_eq!(FetchRedirectMode::parse("FOLLOW"), None);
        assert_eq!(FetchRedirectMode::parse(" manual "), None);
        assert_eq!(FetchRedirectMode::parse(""), None);
    }

    #[test]
    fn recognizes_fetch_redirect_statuses() {
        for status in [301, 302, 303, 307, 308] {
            assert!(is_fetch_redirect_status(status));
        }
        for status in [200, 201, 204, 300, 304, 305, 306, 400] {
            assert!(!is_fetch_redirect_status(status));
        }
    }

    #[test]
    fn dispatches_redirects_by_mode() {
        assert_eq!(
            redirect_response_disposition(FetchRedirectMode::Follow, 302),
            Some(RedirectResponseDisposition::Follow)
        );
        assert_eq!(
            redirect_response_disposition(FetchRedirectMode::Error, 307),
            Some(RedirectResponseDisposition::NetworkError)
        );
        assert_eq!(
            redirect_response_disposition(FetchRedirectMode::Manual, 308),
            Some(RedirectResponseDisposition::OpaqueRedirect)
        );
        assert_eq!(
            redirect_response_disposition(FetchRedirectMode::Manual, 200),
            None
        );
    }

    #[test]
    fn no_cors_requires_follow_redirect_mode() {
        assert!(no_cors_redirect_mode_is_valid(FetchRedirectMode::Follow));
        assert!(!no_cors_redirect_mode_is_valid(FetchRedirectMode::Error));
        assert!(!no_cors_redirect_mode_is_valid(FetchRedirectMode::Manual));
    }
}
