use std::rc::Rc;

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};
use browser_engine::Browser;

fn url(input: &str) -> Url { Url::parse(input).expect("valid URL") }

fn redirect(url_text: &str, location: &str) -> FetchResponse {
    let mut response = FetchResponse::synthetic(url(url_text), 302, Some("text/plain"), Vec::new());
    response.headers.insert_raw("location", location);
    response
}

fn browser_for(script: &str, transport: Rc<ManualNetwork>) -> Browser {
    let mut loader = MemoryLoader::new();
    loader.insert("http://page.test/index.html", format!("<script>{script}</script>"));
    transport.set_auto_complete(true);
    Browser::open_with_single_hop_network(
        Box::new(loader), transport, &url("http://page.test/index.html"), Rc::new(ManualClock::new())
    ).expect("browser opens")
}

#[test]
fn manual_redirect_returns_opaqueredirect_without_following() {
    let transport = Rc::new(ManualNetwork::new());
    transport.respond("http://page.test/start", redirect("http://page.test/start", "/final"));
    transport.respond_text("http://page.test/final", "should-not-load");
    let mut browser = browser_for(
        "let q = new Request('/start', { redirect: 'manual' });\n         console.log(q.redirect);\n         fetch(q).then(function (r) {\n           console.log(r.type); console.log(r.status); console.log(r.url);\n           console.log(r.redirected); return r.text();\n         }).then(function (t) { console.log(t); });",
        transport.clone(),
    );
    browser.settle_network(10);
    assert_eq!(browser.document().runtime.console, vec!["manual", "opaqueredirect", "0", "", "false", ""]);
    let seen = transport.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].url.path(), "/start");
}

#[test]
fn error_redirect_rejects_without_following() {
    let transport = Rc::new(ManualNetwork::new());
    transport.respond("http://page.test/start", redirect("http://page.test/start", "/final"));
    transport.respond_text("http://page.test/final", "should-not-load");
    let mut browser = browser_for(
        "fetch('/start', { redirect: 'error' })\n           .then(function () { console.log('resolved'); })\n           .catch(function () { console.log('rejected'); });",
        transport.clone(),
    );
    browser.settle_network(10);
    assert_eq!(browser.document().runtime.console, vec!["rejected"]);
    assert_eq!(transport.requests().len(), 1);
}

#[test]
fn follow_redirect_remains_default_and_marks_final_response_redirected() {
    let transport = Rc::new(ManualNetwork::new());
    transport.respond("http://page.test/start", redirect("http://page.test/start", "/final"));
    transport.respond_text("http://page.test/final", "final-body");
    let mut browser = browser_for(
        "fetch('/start').then(function (r) { console.log(r.redirected); return r.text(); })\n          .then(function (t) { console.log(t); });",
        transport.clone(),
    );
    browser.settle_network(12);
    assert_eq!(browser.document().runtime.console, vec!["true", "final-body"]);
    assert_eq!(transport.requests().len(), 2);
}

#[test]
fn request_redirect_is_cloned_and_init_can_override_it() {
    let transport = Rc::new(ManualNetwork::new());
    let mut browser = browser_for(
        "let a = new Request('/x', { redirect: 'manual' });\n         let b = new Request(a);\n         let c = new Request(a, { redirect: 'error' });\n         console.log(a.redirect); console.log(b.redirect); console.log(c.redirect);",
        transport,
    );
    browser.settle_network(2);
    assert_eq!(browser.document().runtime.console, vec!["manual", "manual", "error"]);
}

#[test]
fn no_cors_rejects_non_follow_redirect_mode_before_transport() {
    let transport = Rc::new(ManualNetwork::new());
    let mut browser = browser_for(
        "try { new Request('/x', { mode: 'no-cors', redirect: 'manual' }); console.log('bad'); }\n         catch (e) { console.log('rejected'); }",
        transport.clone(),
    );
    browser.settle_network(2);
    assert_eq!(browser.document().runtime.console, vec!["rejected"]);
    assert!(transport.requests().is_empty());
}
