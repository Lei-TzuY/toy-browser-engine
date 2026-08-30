//! Fetch request cache-mode semantics.
//!
//! This module keeps the Web-exposed `Request.cache` vocabulary and the
//! cross-field `only-if-cached` constraint independent from any concrete HTTP
//! cache implementation. That gives request construction a standards-focused
//! policy primitive before transport/cache wiring is added.

use crate::script::host::RequestMode;

/// Fetch's request cache mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FetchCacheMode {
    #[default]
    Default,
    NoStore,
    Reload,
    NoCache,
    ForceCache,
    OnlyIfCached,
}

impl FetchCacheMode {
    /// Parse the exact Web IDL string accepted by `RequestInit.cache`.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "default" => Some(Self::Default),
            "no-store" => Some(Self::NoStore),
            "reload" => Some(Self::Reload),
            "no-cache" => Some(Self::NoCache),
            "force-cache" => Some(Self::ForceCache),
            "only-if-cached" => Some(Self::OnlyIfCached),
            _ => None,
        }
    }

    /// Return the script-visible `Request.cache` value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::NoStore => "no-store",
            Self::Reload => "reload",
            Self::NoCache => "no-cache",
            Self::ForceCache => "force-cache",
            Self::OnlyIfCached => "only-if-cached",
        }
    }
}

/// Fetch only permits `cache: "only-if-cached"` together with
/// `mode: "same-origin"`.
pub const fn cache_mode_is_valid_for_request_mode(
    cache: FetchCacheMode,
    mode: RequestMode,
) -> bool {
    !matches!(cache, FetchCacheMode::OnlyIfCached) || matches!(mode, RequestMode::SameOrigin)
}

/// Conditional request headers force Fetch's effective cache mode from
/// `default` to `no-store`.
///
/// Header names are compared ASCII-case-insensitively, as HTTP field names are.
pub fn effective_cache_mode_for_headers<'a>(
    cache: FetchCacheMode,
    header_names: impl IntoIterator<Item = &'a str>,
) -> FetchCacheMode {
    if cache != FetchCacheMode::Default {
        return cache;
    }

    if header_names.into_iter().any(is_conditional_request_header) {
        FetchCacheMode::NoStore
    } else {
        FetchCacheMode::Default
    }
}

fn is_conditional_request_header(name: &str) -> bool {
    [
        "if-modified-since",
        "if-none-match",
        "if-unmodified-since",
        "if-match",
        "if-range",
    ]
    .iter()
    .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_default_mode() {
        assert_eq!(FetchCacheMode::default(), FetchCacheMode::Default);
    }

    #[test]
    fn parses_only_exact_web_idl_values() {
        for (text, expected) in [
            ("default", FetchCacheMode::Default),
            ("no-store", FetchCacheMode::NoStore),
            ("reload", FetchCacheMode::Reload),
            ("no-cache", FetchCacheMode::NoCache),
            ("force-cache", FetchCacheMode::ForceCache),
            ("only-if-cached", FetchCacheMode::OnlyIfCached),
        ] {
            assert_eq!(FetchCacheMode::parse(text), Some(expected));
            assert_eq!(expected.as_str(), text);
        }

        assert_eq!(FetchCacheMode::parse("DEFAULT"), None);
        assert_eq!(FetchCacheMode::parse(" force-cache "), None);
        assert_eq!(FetchCacheMode::parse(""), None);
    }

    #[test]
    fn only_if_cached_requires_same_origin_mode() {
        assert!(cache_mode_is_valid_for_request_mode(
            FetchCacheMode::OnlyIfCached,
            RequestMode::SameOrigin
        ));
        assert!(!cache_mode_is_valid_for_request_mode(
            FetchCacheMode::OnlyIfCached,
            RequestMode::Cors
        ));
        assert!(!cache_mode_is_valid_for_request_mode(
            FetchCacheMode::OnlyIfCached,
            RequestMode::NoCors
        ));
        assert!(cache_mode_is_valid_for_request_mode(
            FetchCacheMode::ForceCache,
            RequestMode::Cors
        ));
    }

    #[test]
    fn conditional_headers_force_default_to_no_store() {
        for header in [
            "If-Modified-Since",
            "if-none-match",
            "IF-UNMODIFIED-SINCE",
            "If-Match",
            "if-range",
        ] {
            assert_eq!(
                effective_cache_mode_for_headers(FetchCacheMode::Default, [header]),
                FetchCacheMode::NoStore
            );
        }
    }

    #[test]
    fn explicit_cache_mode_is_not_rewritten_by_conditional_headers() {
        assert_eq!(
            effective_cache_mode_for_headers(FetchCacheMode::Reload, ["If-None-Match"]),
            FetchCacheMode::Reload
        );
    }

    #[test]
    fn ordinary_headers_leave_default_unchanged() {
        assert_eq!(
            effective_cache_mode_for_headers(
                FetchCacheMode::Default,
                ["Accept", "Content-Type", "X-Test"]
            ),
            FetchCacheMode::Default
        );
    }
}
