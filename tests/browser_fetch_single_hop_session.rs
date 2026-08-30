use std::rc::Rc;
use std::sync::{Arc, Mutex};

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{
    FetchError, FetchRequest, FetchResponse, LoadError, ManualNetwork, MemoryLoader, Resource,
    ResourceLoader, Url,
};
use browser_engine::Browser;

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

#[derive(Clone)]
struct RedirectingLoader {
    requests: Arc<Mutex<Vec<FetchRequest>>>,
}

impl RedirectingLoader {
    fn new(requests: Arc<Mutex<Vec<FetchRequest>>>) -> Self {
        Self { requests }
    }
}

impl ResourceLoader for RedirectingLoader {
    fn load(&self, target: &Url) -> Result<Resource, LoadError> {
        if target.path() == "/index.html" {
            return Ok(Resource::new(
                target.clone(),
                Some("text/html".into()),
                br#"<script>
                    fetch('/start')
                        .then(function (response) { return response.text(); })
                        .then(function (text) { console.log(text); });
                </script>"#
                    .to_vec(),
            ));
        }
        Err(LoadError::NotFound(target.to_string()))
    }

    fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.requests.lock().unwrap().push(request.clone());
        match request.url.path() {
            "/start" => {
                let mut response = FetchResponse::synthetic(
                    request.url.clone(),
                    302,
                    Some("text/plain"),
                    Vec::new(),
                );
                response.headers.insert_raw("location", "/final");
                response.headers.append_raw("set-cookie", "hop=1; Path=/");
                Ok(response)
            }
            "/final" => Ok(FetchResponse::synthetic(
                request.url.clone(),
                200,
                Some("text/plain"),
                b"redirected body".to_vec(),
            )),
            _ => Ok(FetchResponse::synthetic(
                request.url.clone(),
                404,
                Some("text/plain"),
                Vec::new(),
            )),
        }
    }
}

#[test]
fn default_browser_fetch_observes_redirect_hops_and_intermediate_cookie_state() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let loader = RedirectingLoader::new(requests.clone());
    let page = url("http://page.test/index.html");
    let mut browser = Browser::open_with_clock(
        Box::new(loader),
        &page,
        Rc::new(ManualClock::new()),
    )
    .expect("browser opens");

    browser.settle_network(12);

    assert_eq!(browser.document().runtime.console, vec!["redirected body"]);
    let seen = requests.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].url.path(), "/start");
    assert_eq!(seen[1].url.path(), "/final");
    assert_eq!(seen[1].headers.get("cookie").as_deref(), Some("hop=1"));
}

#[test]
fn explicit_single_hop_browser_constructor_follows_raw_redirect_responses() {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "http://page.test/index.html",
        "<script>fetch('/start').then(function (r) { return r.text(); }).then(function (t) { console.log(t); });</script>",
    );
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    let mut redirect = FetchResponse::synthetic(
        url("http://page.test/start"),
        302,
        Some("text/plain"),
        Vec::new(),
    );
    redirect.headers.insert_raw("location", "/final");
    transport.respond("http://page.test/start", redirect);
    transport.respond(
        "http://page.test/final",
        FetchResponse::synthetic(
            url("http://page.test/final"),
            200,
            Some("text/plain"),
            b"manual final".to_vec(),
        ),
    );

    let clock = Rc::new(ManualClock::new());
    let mut browser = Browser::open_with_single_hop_network(
        Box::new(loader),
        transport.clone(),
        &url("http://page.test/index.html"),
        clock,
    )
    .expect("browser opens");

    browser.settle_network(12);

    assert_eq!(browser.document().runtime.console, vec!["manual final"]);
    let seen = transport.requests();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].url.path(), "/start");
    assert_eq!(seen[1].url.path(), "/final");
}

#[test]
fn legacy_network_constructor_keeps_redirect_following_transport_compatibility() {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "http://page.test/index.html",
        "<script>fetch('/start').then(function (r) { console.log(r.redirected); return r.text(); }).then(function (t) { console.log(t); });</script>",
    );
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    let mut final_response = FetchResponse::synthetic(
        url("http://page.test/final"),
        200,
        Some("text/plain"),
        b"legacy final".to_vec(),
    );
    final_response.redirected = true;
    transport.respond("http://page.test/start", final_response);

    let clock = Rc::new(ManualClock::new());
    let mut browser = Browser::open_with_network(
        Box::new(loader),
        transport,
        &url("http://page.test/index.html"),
        clock,
    )
    .expect("browser opens");

    browser.settle_network(8);
    assert_eq!(browser.document().runtime.console, vec!["true", "legacy final"]);
}

#[test]
fn default_browser_redirect_loop_still_terminates() {
    #[derive(Clone)]
    struct LoopLoader;

    impl ResourceLoader for LoopLoader {
        fn load(&self, target: &Url) -> Result<Resource, LoadError> {
            if target.path() == "/index.html" {
                Ok(Resource::new(
                    target.clone(),
                    Some("text/html".into()),
                    b"<script>fetch('/loop').catch(function (e) { console.log('blocked'); });</script>".to_vec(),
                ))
            } else {
                Err(LoadError::NotFound(target.to_string()))
            }
        }

        fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
            let mut response = FetchResponse::synthetic(
                request.url.clone(),
                302,
                Some("text/plain"),
                Vec::new(),
            );
            response.headers.insert_raw("location", "/loop");
            Ok(response)
        }
    }

    let mut browser = Browser::open_with_clock(
        Box::new(LoopLoader),
        &url("http://page.test/index.html"),
        Rc::new(ManualClock::new()),
    )
    .expect("browser opens");

    browser.settle_network(32);
    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
    assert!(!browser.has_pending_tasks());
}
