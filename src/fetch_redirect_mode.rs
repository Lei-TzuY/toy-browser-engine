//! Fetch request redirect-mode semantics.
//!
//! This module models the transport-neutral portion of Fetch that decides what
//! a script fetch should do when an HTTP redirect response is encountered.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FetchRedirectMode {
    #[default]
    Follow,
    Error,
    Manual,
}

impl FetchRedirectMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "follow" => Some(Self::Follow),
            "error" => Some(Self::Error),
            "manual" => Some(Self::Manual),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Follow => "follow",
            Self::Error => "error",
            Self::Manual => "manual",
        }
    }
}

pub const fn is_fetch_redirect_status(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

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
    fn no_cors_requires_follow_redirect_mode() {
        assert!(no_cors_redirect_mode_is_valid(FetchRedirectMode::Follow));
        assert!(!no_cors_redirect_mode_is_valid(FetchRedirectMode::Error));
        assert!(!no_cors_redirect_mode_is_valid(FetchRedirectMode::Manual));
    }
}
