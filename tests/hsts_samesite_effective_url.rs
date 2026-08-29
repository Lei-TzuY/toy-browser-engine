use std::sync::{Arc, Mutex};

use browser_engine::net::{
    FetchError, FetchRequest, FetchResponse, LoadError, Resource, ResourceLoader, Url,
};
use browser_engine::Browser;

#[derive(Clone, Default)]
struct HstsLoader {
    requests: Arc<Mutex<Vec<FetchRequest>>>,
}

impl HstsLoader {
    fn requests(&self) -> Vec<FetchRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl ResourceLoader for HstsLoader {
    fn load(&self, url: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::NotFound(url.to_string()))
    }

    fn load_response(&self, _url: &Url) -> Result<FetchResponse, LoadError> {
        let mut response = FetchResponse::synthetic(
            Url::parse("https://example.test/home").unwrap(),
            200,
            Some("text/html"),
            b"<title>Home</title>".to_vec(),
        );
        response
            .headers
            .append_raw("strict-transport-security", "max-age=60");
        response.headers.append_raw(
            "set-cookie",
            "strict=one; Path=/; Secure; SameSite=Strict",
        );
        Ok(response)
    }

    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(FetchResponse::synthetic(
            request.url.clone(),
            200,
            Some("text/html"),
            b"<title>Next</title>".to_vec(),
        ))
    }
}

#[test]
fn hsts_effective_target_is_used_for_schemeful_samesite_classification() {
    let loader = HstsLoader::default();
    let probe = loader.clone();
    let mut browser = Browser::open(
        Box::new(loader),
        &Url::parse("http://example.test/start").unwrap(),
    )
    .unwrap();

    assert_eq!(browser.url().to_string(), "https://example.test/home");

    browser
        .navigate(&Url::parse("http://example.test/next").unwrap())
        .unwrap();

    let requests = probe.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.to_string(), "https://example.test/next");
    assert_eq!(requests[0].headers.get("cookie").as_deref(), Some("strict=one"));
}
