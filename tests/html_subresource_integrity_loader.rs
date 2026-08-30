use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use browser_engine::cookie::CookieJar;
use browser_engine::eventloop::ManualClock;
use browser_engine::hsts::HstsCache;
use browser_engine::net::{FetchError, FetchRequest, FetchResponse, LoadError, Resource, ResourceLoader};
use browser_engine::{
    fetch_html_subresource_with_integrity, DocumentReferrerContext, HtmlSubresourceIntegrityError,
    IntegrityPolicy, IntegrityPolicyContainer, IntegrityPolicyDestination, NavigationNetwork, Url,
};

const OK_SHA256: &str = "sha256-Jok2eyBcFs4y7UIAlCuLix4mLfxw2byfvHfElpmk8d8=";

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

struct RecordingLoader {
    requests: Arc<Mutex<Vec<FetchRequest>>>,
    allow_cors: bool,
}

impl RecordingLoader {
    fn new(allow_cors: bool) -> (Self, Arc<Mutex<Vec<FetchRequest>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                requests: requests.clone(),
                allow_cors,
            },
            requests,
        )
    }
}

impl ResourceLoader for RecordingLoader {
    fn load(&self, target: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::NotFound(target.to_string()))
    }

    fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.requests.lock().unwrap().push(request.clone());
        let mut response = FetchResponse::synthetic(
            request.url.clone(),
            200,
            Some("application/javascript"),
            b"ok".to_vec(),
        );
        if self.allow_cors {
            response
                .headers
                .insert_raw("access-control-allow-origin", "https://page.test");
        }
        Ok(response)
    }
}

fn network(allow_cors: bool) -> (NavigationNetwork, Arc<Mutex<Vec<FetchRequest>>>) {
    let (loader, requests) = RecordingLoader::new(allow_cors);
    let loader: Arc<dyn ResourceLoader> = Arc::new(loader);
    let clock = Rc::new(ManualClock::new());
    let jar = Rc::new(RefCell::new(CookieJar::with_clock(clock.clone())));
    let hsts = Rc::new(RefCell::new(HstsCache::new()));
    (NavigationNetwork::new(loader, jar, hsts, clock), requests)
}

fn blocking_container() -> IntegrityPolicyContainer {
    IntegrityPolicyContainer {
        enforced: IntegrityPolicy::parse("blocked-destinations=(script style)"),
        report_only: IntegrityPolicy::default(),
    }
}

#[test]
fn enforced_policy_blocks_before_network_dispatch() {
    let (network, requests) = network(true);
    let document = DocumentReferrerContext::from_url(url("https://page.test/index.html"));

    let result = fetch_html_subresource_with_integrity(
        &network,
        &document,
        &blocking_container(),
        IntegrityPolicyDestination::Script,
        &url("https://cdn.test/app.js"),
        Some("anonymous"),
        None,
        "",
    );

    assert!(matches!(result, Err(HtmlSubresourceIntegrityError::PolicyBlocked)));
    assert!(requests.lock().unwrap().is_empty(), "blocked policy must prevent I/O");
}

#[test]
fn cross_origin_integrity_requires_cors_before_network_dispatch() {
    let (network, requests) = network(false);
    let document = DocumentReferrerContext::from_url(url("https://page.test/index.html"));
    let empty = IntegrityPolicyContainer::default();

    let result = fetch_html_subresource_with_integrity(
        &network,
        &document,
        &empty,
        IntegrityPolicyDestination::Script,
        &url("https://cdn.test/app.js"),
        None,
        None,
        OK_SHA256,
    );

    assert!(matches!(result, Err(HtmlSubresourceIntegrityError::CorsRequired)));
    assert!(requests.lock().unwrap().is_empty(), "invalid SRI mode must fail before I/O");
}

#[test]
fn cors_authorized_resource_with_matching_integrity_passes() {
    let (network, requests) = network(true);
    let document = DocumentReferrerContext::from_url(url("https://page.test/index.html"));

    let result = fetch_html_subresource_with_integrity(
        &network,
        &document,
        &blocking_container(),
        IntegrityPolicyDestination::Script,
        &url("https://cdn.test/app.js"),
        Some("anonymous"),
        Some("no-referrer"),
        OK_SHA256,
    )
    .expect("CORS-approved matching SRI should load");

    assert_eq!(result.response.body, b"ok");
    assert!(!result.report_only_violation);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].headers.get("origin").as_deref(), Some("https://page.test"));
    assert!(!requests[0].headers.has("referer"));
}

#[test]
fn response_bytes_are_verified_after_cors_succeeds() {
    let (network, requests) = network(true);
    let document = DocumentReferrerContext::from_url(url("https://page.test/index.html"));

    let result = fetch_html_subresource_with_integrity(
        &network,
        &document,
        &IntegrityPolicyContainer::default(),
        IntegrityPolicyDestination::Style,
        &url("https://cdn.test/style.css"),
        Some("anonymous"),
        None,
        "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
    );

    assert!(matches!(result, Err(HtmlSubresourceIntegrityError::IntegrityMismatch)));
    assert_eq!(requests.lock().unwrap().len(), 1, "hash mismatch is a post-response failure");
}

#[test]
fn report_only_policy_observes_violation_without_blocking() {
    let (network, requests) = network(false);
    let document = DocumentReferrerContext::from_url(url("https://page.test/index.html"));
    let container = IntegrityPolicyContainer {
        enforced: IntegrityPolicy::default(),
        report_only: IntegrityPolicy::parse("blocked-destinations=(script)"),
    };

    let result = fetch_html_subresource_with_integrity(
        &network,
        &document,
        &container,
        IntegrityPolicyDestination::Script,
        &url("https://page.test/app.js"),
        None,
        None,
        "",
    )
    .expect("report-only policy must not block");

    assert!(result.report_only_violation);
    assert_eq!(requests.lock().unwrap().len(), 1);
}
