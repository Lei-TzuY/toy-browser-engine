use std::rc::Rc;

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};
use browser_engine::Browser;

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn redirect_response(endpoint: &str, location: &str) -> FetchResponse {
    let mut response = FetchResponse::synthetic(
        url(endpoint),
        302,
        Some("text/plain"),
        b"redirect body must stay hidden".to_vec(),
    );
    response.headers.insert_raw("location", location);
    response.headers.insert_raw("x-secret", "hidden");
    response
}

fn browser_for(script: &str, transport: Rc<ManualNetwork>) -> Browser {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "http://page.test/index.html",
        format!("<script>{script}</script>"),
    );
    Browser::open_with_single_hop_network(
        Box::new(loader),
        transport,
        &url("http://page.test/index.html"),
        Rc::new(ManualClock::new()),
    )
    .expect("browser opens")
}

#[test]
fn request_redirect_is_visible_inherited_and_constructor_overridable() {
    let transport = Rc::new(ManualNetwork::new());
    let browser = browser_for(
        r#"
            var original = new Request('/start', { redirect: 'manual' });
            var clone = new Request(original);
            var overridden = new Request(original, { redirect: 'error' });
            console.log(original.redirect);
            console.log(clone.redirect);
            console.log(overridden.redirect);
            console.log(original.redirect);
        "#,
        transport,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec!["manual", "manual", "error", "manual"]
    );
}

#[test]
fn follow_mode_observes_final_response_and_redirected_flag() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/start",
        redirect_response("http://page.test/start", "/final"),
    );
    transport.respond(
        "http://page.test/final",
        FetchResponse::synthetic(
            url("http://page.test/final"),
            200,
            Some("text/plain"),
            b"final body".to_vec(),
        ),
    );

    let mut browser = browser_for(
        r#"
            var request = new Request('/start');
            console.log(request.redirect);
            fetch(request, { redirect: 'follow' }).then(function (response) {
                console.log(response.type);
                console.log(response.redirected);
                console.log(response.url);
                return response.text();
            }).then(function (text) { console.log(text); });
        "#,
        transport.clone(),
    );
    browser.settle_network(12);

    assert_eq!(
        browser.document().runtime.console,
        vec![
            "follow",
            "basic",
            "true",
            "http://page.test/final",
            "final body",
        ]
    );
    let seen = transport.requests();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].url.path(), "/start");
    assert_eq!(seen[1].url.path(), "/final");
}

#[test]
fn error_mode_rejects_at_first_redirect_without_second_dispatch() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/start",
        redirect_response("http://page.test/start", "/final"),
    );
    transport.respond(
        "http://page.test/final",
        FetchResponse::synthetic(
            url("http://page.test/final"),
            200,
            Some("text/plain"),
            b"must not be fetched".to_vec(),
        ),
    );

    let mut browser = browser_for(
        r#"
            fetch('/start', { redirect: 'error' })
                .then(function () { console.log('unexpected'); })
                .catch(function () { console.log('blocked'); });
        "#,
        transport.clone(),
    );
    browser.settle_network(8);

    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
    let seen = transport.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].url.path(), "/start");
}

#[test]
fn manual_mode_returns_opaqueredirect_and_never_follows_location() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/start",
        redirect_response("http://page.test/start", "http://other.test/final"),
    );

    let mut browser = browser_for(
        r#"
            var saved;
            fetch('/start', { redirect: 'manual' }).then(function (response) {
                saved = response;
                console.log(response.type);
                console.log(response.status);
                console.log(response.statusText);
                console.log(response.ok);
                console.log(response.url);
                console.log(response.redirected);
                console.log(response.headers.get('location') === null ? 'hidden' : 'leaked');
                console.log(response.headers.get('x-secret') === null ? 'hidden' : 'leaked');
                console.log(response.bodyUsed);
                return response.text();
            }).then(function (text) {
                console.log(text);
                console.log(saved.bodyUsed);
            });
        "#,
        transport.clone(),
    );
    browser.settle_network(8);

    assert_eq!(
        browser.document().runtime.console,
        vec![
            "opaqueredirect",
            "0",
            "",
            "false",
            "",
            "false",
            "hidden",
            "hidden",
            "false",
            "",
            "false",
        ]
    );
    let seen = transport.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].url.path(), "/start");
}

#[test]
fn fetch_init_overrides_request_redirect_without_mutating_original() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/start",
        redirect_response("http://page.test/start", "/final"),
    );

    let mut browser = browser_for(
        r#"
            var request = new Request('/start', { redirect: 'manual' });
            fetch(request, { redirect: 'error' })
                .catch(function () { console.log('blocked'); });
            console.log(request.redirect);
        "#,
        transport.clone(),
    );
    browser.settle_network(8);

    assert_eq!(
        browser.document().runtime.console,
        vec!["manual", "blocked"]
    );
    assert_eq!(transport.requests().len(), 1);
}

#[test]
fn legacy_redirect_following_transport_rejects_non_follow_modes_before_io() {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "http://page.test/index.html",
        "<script>fetch('/start', { redirect: 'manual' }).catch(function () { console.log('unsupported'); });</script>",
    );
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/start",
        redirect_response("http://page.test/start", "/final"),
    );
    let browser = Browser::open_with_network(
        Box::new(loader),
        transport.clone(),
        &url("http://page.test/index.html"),
        Rc::new(ManualClock::new()),
    )
    .expect("browser opens");

    assert_eq!(browser.document().runtime.console, vec!["unsupported"]);
    assert!(transport.requests().is_empty());
}
