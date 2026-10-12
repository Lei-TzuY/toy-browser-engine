//! Fetch Metadata request-header semantics.
//!
//! `Sec-Fetch-*` fields are browser-owned request metadata. This module keeps
//! their classification and header mutation transport-neutral so navigation,
//! subresource and script Fetch paths can share one implementation.

use crate::net::fetch::{FetchRequest, Origin};
use crate::net::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchMetadataSite {
    None,
    SameOrigin,
    SameSite,
    CrossSite,
}

impl FetchMetadataSite {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::SameOrigin => "same-origin",
            Self::SameSite => "same-site",
            Self::CrossSite => "cross-site",
        }
    }
}

/// Classify `Sec-Fetch-Site` for an outgoing request.
///
/// Site computation itself remains a browser-layer responsibility. The
/// `same_site` argument lets callers reuse their existing schemeful-site / cookie
/// classification while this helper gives exact-origin precedence as Fetch
/// Metadata requires. Requests with no web initiator use `none`.
pub fn classify_fetch_metadata_site(
    source: Option<&Url>,
    target: &Url,
    same_site: bool,
) -> FetchMetadataSite {
    let Some(source) = source else {
        return FetchMetadataSite::None;
    };
    if Origin::of(source).can_fetch(target) {
        FetchMetadataSite::SameOrigin
    } else if same_site {
        FetchMetadataSite::SameSite
    } else {
        FetchMetadataSite::CrossSite
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchMetadataMode {
    Cors,
    Navigate,
    NoCors,
    SameOrigin,
    WebSocket,
}

impl FetchMetadataMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cors => "cors",
            Self::Navigate => "navigate",
            Self::NoCors => "no-cors",
            Self::SameOrigin => "same-origin",
            Self::WebSocket => "websocket",
        }
    }
}

/// Fetch request destination values used by `Sec-Fetch-Dest`.
///
/// This intentionally follows Fetch's destination vocabulary rather than only
/// the destinations currently consumed by the engine. Keeping the enum complete
/// prevents future loaders from inventing ad-hoc strings and makes unsupported
/// destinations explicit at the integration boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchMetadataDestination {
    Empty,
    Audio,
    AudioWorklet,
    Document,
    Embed,
    Font,
    Frame,
    Iframe,
    Image,
    Json,
    Manifest,
    Object,
    PaintWorklet,
    Report,
    Script,
    ServiceWorker,
    SharedWorker,
    Style,
    Text,
    Track,
    Video,
    WebIdentity,
    Worker,
    Xslt,
}

impl FetchMetadataDestination {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Audio => "audio",
            Self::AudioWorklet => "audioworklet",
            Self::Document => "document",
            Self::Embed => "embed",
            Self::Font => "font",
            Self::Frame => "frame",
            Self::Iframe => "iframe",
            Self::Image => "image",
            Self::Json => "json",
            Self::Manifest => "manifest",
            Self::Object => "object",
            Self::PaintWorklet => "paintworklet",
            Self::Report => "report",
            Self::Script => "script",
            Self::ServiceWorker => "serviceworker",
            Self::SharedWorker => "sharedworker",
            Self::Style => "style",
            Self::Text => "text",
            Self::Track => "track",
            Self::Video => "video",
            Self::WebIdentity => "webidentity",
            Self::Worker => "worker",
            Self::Xslt => "xslt",
        }
    }

    /// Whether Fetch classifies this destination as a subresource request.
    pub const fn is_subresource(self) -> bool {
        matches!(
            self,
            Self::Empty
                | Self::Audio
                | Self::AudioWorklet
                | Self::Font
                | Self::Image
                | Self::Json
                | Self::Manifest
                | Self::PaintWorklet
                | Self::Script
                | Self::Style
                | Self::Text
                | Self::Track
                | Self::Video
                | Self::Xslt
        )
    }

    /// Whether Fetch classifies this destination as navigation-like.
    pub const fn is_navigation(self) -> bool {
        matches!(
            self,
            Self::Document | Self::Embed | Self::Frame | Self::Iframe | Self::Object
        )
    }

    /// Fetch's script-like destination classification.
    pub const fn is_script_like(self) -> bool {
        matches!(
            self,
            Self::AudioWorklet
                | Self::PaintWorklet
                | Self::Script
                | Self::ServiceWorker
                | Self::SharedWorker
                | Self::Worker
        )
    }
}

/// Install browser-owned Fetch Metadata fields, replacing any authored copies.
///
/// `Sec-Fetch-User: ?1` is emitted only for user-activated navigations. For all
/// other requests the field is absent rather than set to a false token.
pub fn apply_fetch_metadata_headers(
    request: &mut FetchRequest,
    site: FetchMetadataSite,
    mode: FetchMetadataMode,
    destination: FetchMetadataDestination,
    user_activated_navigation: bool,
) {
    for name in ["sec-fetch-site", "sec-fetch-mode", "sec-fetch-dest", "sec-fetch-user"] {
        request.headers.delete(name);
    }
    request.headers.insert_raw("sec-fetch-site", site.as_str());
    request.headers.insert_raw("sec-fetch-mode", mode.as_str());
    request
        .headers
        .insert_raw("sec-fetch-dest", destination.as_str());
    if user_activated_navigation && mode == FetchMetadataMode::Navigate {
        request.headers.insert_raw("sec-fetch-user", "?1");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::fetch::{FetchRequest, HeaderMap, Method};

    fn url(value: &str) -> Url {
        Url::parse(value).expect("valid URL")
    }

    #[test]
    fn exact_origin_takes_precedence_over_site_hint() {
        assert_eq!(
            classify_fetch_metadata_site(
                Some(&url("https://example.test/page")),
                &url("https://example.test/api"),
                false,
            ),
            FetchMetadataSite::SameOrigin
        );
    }

    #[test]
    fn distinguishes_same_site_cross_origin_and_cross_site() {
        let source = url("https://www.example.test/page");
        let target = url("https://cdn.example.test/app.js");
        assert_eq!(
            classify_fetch_metadata_site(Some(&source), &target, true),
            FetchMetadataSite::SameSite
        );
        assert_eq!(
            classify_fetch_metadata_site(Some(&source), &target, false),
            FetchMetadataSite::CrossSite
        );
    }

    #[test]
    fn no_initiator_is_none() {
        assert_eq!(
            classify_fetch_metadata_site(None, &url("https://example.test/"), false),
            FetchMetadataSite::None
        );
    }

    #[test]
    fn destination_strings_cover_fetch_vocabulary() {
        let cases = [
            (FetchMetadataDestination::Empty, "empty"),
            (FetchMetadataDestination::Audio, "audio"),
            (FetchMetadataDestination::AudioWorklet, "audioworklet"),
            (FetchMetadataDestination::Document, "document"),
            (FetchMetadataDestination::Embed, "embed"),
            (FetchMetadataDestination::Font, "font"),
            (FetchMetadataDestination::Frame, "frame"),
            (FetchMetadataDestination::Iframe, "iframe"),
            (FetchMetadataDestination::Image, "image"),
            (FetchMetadataDestination::Json, "json"),
            (FetchMetadataDestination::Manifest, "manifest"),
            (FetchMetadataDestination::Object, "object"),
            (FetchMetadataDestination::PaintWorklet, "paintworklet"),
            (FetchMetadataDestination::Report, "report"),
            (FetchMetadataDestination::Script, "script"),
            (FetchMetadataDestination::ServiceWorker, "serviceworker"),
            (FetchMetadataDestination::SharedWorker, "sharedworker"),
            (FetchMetadataDestination::Style, "style"),
            (FetchMetadataDestination::Text, "text"),
            (FetchMetadataDestination::Track, "track"),
            (FetchMetadataDestination::Video, "video"),
            (FetchMetadataDestination::WebIdentity, "webidentity"),
            (FetchMetadataDestination::Worker, "worker"),
            (FetchMetadataDestination::Xslt, "xslt"),
        ];
        for (destination, expected) in cases {
            assert_eq!(destination.as_str(), expected);
        }
    }

    #[test]
    fn destination_categories_match_fetch() {
        assert!(FetchMetadataDestination::Image.is_subresource());
        assert!(FetchMetadataDestination::Script.is_subresource());
        assert!(!FetchMetadataDestination::Document.is_subresource());
        assert!(FetchMetadataDestination::Document.is_navigation());
        assert!(FetchMetadataDestination::Iframe.is_navigation());
        assert!(!FetchMetadataDestination::Worker.is_navigation());
        assert!(FetchMetadataDestination::AudioWorklet.is_script_like());
        assert!(FetchMetadataDestination::ServiceWorker.is_script_like());
        assert!(!FetchMetadataDestination::Xslt.is_script_like());
    }

    #[test]
    fn browser_owned_headers_replace_authored_values() {
        let mut headers = HeaderMap::new();
        headers.append_raw("sec-fetch-site", "same-origin");
        headers.append_raw("sec-fetch-mode", "cors");
        headers.append_raw("sec-fetch-user", "?0");
        let mut request = FetchRequest::new(
            url("https://cdn.test/app.js"),
            Method::Get,
            headers,
            None,
        );
        apply_fetch_metadata_headers(
            &mut request,
            FetchMetadataSite::CrossSite,
            FetchMetadataMode::NoCors,
            FetchMetadataDestination::Script,
            false,
        );
        assert_eq!(request.headers.get("sec-fetch-site").as_deref(), Some("cross-site"));
        assert_eq!(request.headers.get("sec-fetch-mode").as_deref(), Some("no-cors"));
        assert_eq!(request.headers.get("sec-fetch-dest").as_deref(), Some("script"));
        assert!(!request.headers.has("sec-fetch-user"));
    }

    #[test]
    fn sec_fetch_user_is_only_sent_for_activated_navigation() {
        let mut request = FetchRequest::new(
            url("https://example.test/"),
            Method::Get,
            HeaderMap::new(),
            None,
        );
        apply_fetch_metadata_headers(
            &mut request,
            FetchMetadataSite::SameOrigin,
            FetchMetadataMode::Navigate,
            FetchMetadataDestination::Document,
            true,
        );
        assert_eq!(request.headers.get("sec-fetch-user").as_deref(), Some("?1"));

        apply_fetch_metadata_headers(
            &mut request,
            FetchMetadataSite::SameOrigin,
            FetchMetadataMode::Cors,
            FetchMetadataDestination::Empty,
            true,
        );
        assert!(!request.headers.has("sec-fetch-user"));
    }
}
