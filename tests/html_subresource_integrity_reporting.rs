use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use browser_engine::cookie::CookieJar;
use browser_engine::eventloop::ManualClock;
use browser_engine::hsts::HstsCache;
use browser_engine::net::{FetchError, FetchRequest, FetchResponse, LoadError, Resource, ResourceLoader};
use browser_engine::{
    fetch_html_subresource_with_integrity_reporting, DocumentReferrerContext,
    HtmlSubresourceIntegrityError, IntegrityPolicy, IntegrityPolicyContainer,
    IntegrityPolicyDestination, IntegrityReportQueue, NavigationNetwork, Url,
};

const OK_SHA256: &str = "sha256-Jok2eyBcFs4y7UIAlCuLix4mLfxw2byfvHfElpmk8d8=";

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

struct RecordingLoader {
    requests: Arc<Mutex<Vec<FetchRequest>>>,
}

impl RecordingLoader {
    fn new() -> (Self, Arc<Mutex<Vec<FetchRequest>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                requests: requests.clone(),
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
        response
            .headers
            .insert_raw("access-control-allow-origin", "https://page.test");
        Ok(response)
    }
}

fn network() -> (NavigationNetwork, Arc<Mutex<Vec<FetchRequest>>>) {
    let (loader, requests) = RecordingLoader::new();
    let loader: Arc<dyn ResourceLoader> = Arc::new(loader);
    let clock = Rc::new(ManualClock::new());
    let jar = Rc::new(RefCell::new(CookieJar::with_clock(clock.clone())));
    let hsts = Rc::new(RefCell::new(HstsCache::new()));
    (NavigationNetwork::new(loader, jar, hsts, clock), requests)
}

#[test]
fn enforced_violation_is_queued_even_when_transport_is_blocked() {
    let (network, requests) = network();
    let document = DocumentReferrerContext::from_url(url("https://page.test/index.html#secret"));
    let container = IntegrityPolicyContainer {
        enforced: IntegrityPolicy::parse(
            "blocked-destinations=(script), endpoints=(primary backup)",
        ),
        report_only: IntegrityPolicy::default(),
    };
    let mut queue = IntegrityReportQueue::new();

    let result = fetch_html_subresource_with_integrity_reporting(
        &network,
        &document,
        &container,
        &mut queue,
        IntegrityPolicyDestination::Script,
        &url("https://cdn.test/app.js#fragment"),
        Some("anonymous"),
        None,
        "",
    );

    assert!(matches!(result, Err(HtmlSubresourceIntegrityError::PolicyBlocked)));
    assert!(requests.lock().unwrap().is_empty());
    let reports = queue.drain();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0].endpoint, "primary");
    assert_eq!(reports[1].endpoint, "backup");
    assert!(!reports[0].body.report_only);
    assert_eq!(reports[0].body.document_url, "https://page.test/index.html");
    assert_eq!(reports[0].body.blocked_url, "https://cdn.test/app.js");
}

#[test]
fn report_only_violation_queues_work_but_allows_loading() {
    let (network, requests) = network();
    let document = DocumentReferrerContext::from_url(url("https://page.test/index.html"));
    let container = IntegrityPolicyContainer {
        enforced: IntegrityPolicy::default(),
        report_only: IntegrityPolicy::parse(
            "blocked-destinations=(script), endpoints=(observe)",
        ),
    };
    let mut queue = IntegrityReportQueue::new();

    let loaded = fetch_html_subresource_with_integrity_reporting(
        &network,
        &document,
        &container,
        &mut queue,
        IntegrityPolicyDestination::Script,
        &url("https://page.test/app.js"),
        None,
        None,
        "",
    )
    .expect("report-only violation must not block");

    assert!(loaded.report_only_violation);
    assert_eq!(requests.lock().unwrap().len(), 1);
    let report = queue.pop_front().expect("report-only work should be queued");
    assert_eq!(report.endpoint, "observe");
    assert!(report.body.report_only);
    assert!(queue.is_empty());
}

#[test]
fn satisfying_integrity_policy_does_not_queue_a_report() {
    let (network, requests) = network();
    let document = DocumentReferrerContext::from_url(url("https://page.test/index.html"));
    let container = IntegrityPolicyContainer {
        enforced: IntegrityPolicy::parse(
            "blocked-destinations=(script), endpoints=(primary)",
        ),
        report_only: IntegrityPolicy::parse(
            "blocked-destinations=(script), endpoints=(observe)",
        ),
    };
    let mut queue = IntegrityReportQueue::new();

    let loaded = fetch_html_subresource_with_integrity_reporting(
        &network,
        &document,
        &container,
        &mut queue,
        IntegrityPolicyDestination::Script,
        &url("https://cdn.test/app.js"),
        Some("anonymous"),
        None,
        OK_SHA256,
    )
    .expect("valid CORS SRI should satisfy both policies");

    assert_eq!(loaded.response.body, b"ok");
    assert!(!loaded.report_only_violation);
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert!(queue.is_empty());
}

#[test]
fn violation_without_endpoints_does_not_create_pending_delivery_work() {
    let (network, requests) = network();
    let document = DocumentReferrerContext::from_url(url("https://page.test/index.html"));
    let container = IntegrityPolicyContainer {
        enforced: IntegrityPolicy::parse("blocked-destinations=(style)"),
        report_only: IntegrityPolicy::default(),
    };
    let mut queue = IntegrityReportQueue::new();

    let result = fetch_html_subresource_with_integrity_reporting(
        &network,
        &document,
        &container,
        &mut queue,
        IntegrityPolicyDestination::Style,
        &url("https://cdn.test/style.css"),
        Some("anonymous"),
        None,
        "",
    );

    assert!(matches!(result, Err(HtmlSubresourceIntegrityError::PolicyBlocked)));
    assert!(requests.lock().unwrap().is_empty());
    assert!(queue.is_empty());
}
