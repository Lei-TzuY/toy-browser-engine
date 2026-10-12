use browser_engine::fetch_metadata::{
    apply_fetch_metadata_headers, classify_fetch_metadata_site, FetchMetadataDestination,
    FetchMetadataMode, FetchMetadataSite,
};
use browser_engine::net::fetch::{FetchRequest, HeaderMap, Method};
use browser_engine::net::Url;

fn url(value: &str) -> Url {
    Url::parse(value).expect("valid URL")
}

#[test]
fn public_site_classifier_distinguishes_request_relationships() {
    let source = url("https://app.example.test/index.html");

    assert_eq!(
        classify_fetch_metadata_site(
            Some(&source),
            &url("https://app.example.test/data"),
            false,
        ),
        FetchMetadataSite::SameOrigin
    );
    assert_eq!(
        classify_fetch_metadata_site(
            Some(&source),
            &url("https://cdn.example.test/app.js"),
            true,
        ),
        FetchMetadataSite::SameSite
    );
    assert_eq!(
        classify_fetch_metadata_site(
            Some(&source),
            &url("https://other.test/app.js"),
            false,
        ),
        FetchMetadataSite::CrossSite
    );
    assert_eq!(
        classify_fetch_metadata_site(None, &url("https://other.test/"), false),
        FetchMetadataSite::None
    );
}

#[test]
fn request_metadata_is_browser_owned_and_complete() {
    let mut headers = HeaderMap::new();
    headers.append_raw("Sec-Fetch-Site", "same-origin");
    headers.append_raw("Sec-Fetch-Mode", "cors");
    headers.append_raw("Sec-Fetch-Dest", "empty");
    headers.append_raw("Sec-Fetch-User", "?0");
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

    assert_eq!(request.headers.get("Sec-Fetch-Site").as_deref(), Some("cross-site"));
    assert_eq!(request.headers.get("Sec-Fetch-Mode").as_deref(), Some("no-cors"));
    assert_eq!(request.headers.get("Sec-Fetch-Dest").as_deref(), Some("script"));
    assert!(!request.headers.has("Sec-Fetch-User"));
}

#[test]
fn activated_navigation_gets_sec_fetch_user() {
    let mut request = FetchRequest::new(
        url("https://example.test/next"),
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

    assert_eq!(request.headers.get("sec-fetch-site").as_deref(), Some("same-origin"));
    assert_eq!(request.headers.get("sec-fetch-mode").as_deref(), Some("navigate"));
    assert_eq!(request.headers.get("sec-fetch-dest").as_deref(), Some("document"));
    assert_eq!(request.headers.get("sec-fetch-user").as_deref(), Some("?1"));
}
