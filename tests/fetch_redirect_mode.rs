use browser_engine::{
    is_fetch_redirect_status, no_cors_redirect_mode_is_valid, redirect_response_disposition,
    FetchRedirectMode, RedirectResponseDisposition,
};

#[test]
fn public_redirect_mode_api_matches_fetch_values() {
    assert_eq!(FetchRedirectMode::default().as_str(), "follow");
    assert_eq!(FetchRedirectMode::parse("error").unwrap().as_str(), "error");
    assert_eq!(FetchRedirectMode::parse("manual").unwrap().as_str(), "manual");
    assert_eq!(FetchRedirectMode::parse("Manual"), None);
}

#[test]
fn public_redirect_status_set_matches_fetch_http_redirects() {
    assert!([301, 302, 303, 307, 308]
        .into_iter()
        .all(is_fetch_redirect_status));
    assert!([200, 204, 300, 304, 305, 306, 404]
        .into_iter()
        .all(|status| !is_fetch_redirect_status(status)));
}

#[test]
fn manual_redirect_produces_opaque_redirect_disposition() {
    assert_eq!(
        redirect_response_disposition(FetchRedirectMode::Manual, 302),
        Some(RedirectResponseDisposition::OpaqueRedirect)
    );
}

#[test]
fn redirect_error_mode_produces_network_error_disposition() {
    assert_eq!(
        redirect_response_disposition(FetchRedirectMode::Error, 307),
        Some(RedirectResponseDisposition::NetworkError)
    );
}

#[test]
fn ordinary_responses_do_not_enter_redirect_handling() {
    assert_eq!(
        redirect_response_disposition(FetchRedirectMode::Follow, 200),
        None
    );
}

#[test]
fn no_cors_accepts_only_follow_redirect_mode() {
    assert!(no_cors_redirect_mode_is_valid(FetchRedirectMode::Follow));
    assert!(!no_cors_redirect_mode_is_valid(FetchRedirectMode::Error));
    assert!(!no_cors_redirect_mode_is_valid(FetchRedirectMode::Manual));
}
