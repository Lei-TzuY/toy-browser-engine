use std::sync::{Arc, Mutex};

use browser_engine::browser::Browser;
use browser_engine::net::{
    FetchRequest, FetchResponse, LoadError, Resource, ResourceLoader, Url,
};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

#[derive(Clone)]
struct RedirectingDocumentLoader {
    requests: Arc<Mutex<Vec<FetchRequest>>>,
    final_status: u16,
}

impl RedirectingDocumentLoader {
    fn new(final_status: u16) -> (Self, Arc<Mutex<Vec<FetchRequest>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                requests: requests.clone(),
                final_status,
            },
            requests,
        )
    }
}

impl ResourceLoader for RedirectingDocumentLoader {
    fn load(&self, target: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::Io {
            url: target.to_string(),
            message: "legacy load path must not run for opted-in initial redirects".into(),
        })
    }

    fn load_response_once(
        &self,
        request: &FetchRequest,
    ) -> Result<Option<FetchResponse>, LoadError> {
        self.requests.lock().unwrap().push(request.clone());

        let mut response = match request.url.path() {
            "/start" => {
                let mut response = FetchResponse::synthetic(
                    request.url.clone(),
                    302,
                    Some("text/html"),
                    Vec::new(),
                );
                response.headers.insert_raw("location", "/final");
                response.headers.insert_raw(
                    "set-cookie",
                    "bootstrap=ready; Path=/; SameSite=Strict",
                );
                response
            }
            "/final" if self.final_status == 200 => FetchResponse::synthetic(
                request.url.clone(),
                200,
                Some("text/html"),
                b"<!doctype html><title>final</title><p>ok</p>".to_vec(),
            ),
            "/final" => FetchResponse::synthetic(
                request.url.clone(),
                self.final_status,
                Some("text/plain"),
                b"missing".to_vec(),
            ),
            other => panic!("unexpected document request path: {other}"),
        };
        response.redirected = false;
        Ok(Some(response))
    }
}

#[test]
fn browser_open_applies_redirect_set_cookie_before_next_initial_hop() {
    let (loader, requests) = RedirectingDocumentLoader::new(200);
    let start = url("http://example.test/start");
    let browser = Browser::open(Box::new(loader), &start).expect("open browser");

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].url.to_string(), "http://example.test/start");
    assert!(
        requests[0].headers.get("cookie").is_none(),
        "fresh Browser initial request must not invent cookies"
    );
    assert_eq!(requests[1].url.to_string(), "http://example.test/final");
    assert_eq!(
        requests[1].headers.get("cookie").as_deref(),
        Some("bootstrap=ready"),
        "redirect-set cookie must be selected for the next initial hop"
    );

    assert_eq!(browser.url().to_string(), "http://example.test/final");
    assert_eq!(browser.history().len(), 1);
    assert_eq!(browser.history()[0], *browser.url());
}

#[test]
fn browser_open_preserves_load_error_semantics_after_redirect_chain() {
    let (loader, requests) = RedirectingDocumentLoader::new(404);
    let start = url("http://example.test/start");
    let error = match Browser::open(Box::new(loader), &start) {
        Ok(_) => panic!("redirected 404 must preserve document-load error semantics"),
        Err(error) => error,
    };

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].url.to_string(), "http://example.test/final");
    assert_eq!(
        error,
        LoadError::HttpStatus {
            url: "http://example.test/final".into(),
            status: 404,
        }
    );
}
