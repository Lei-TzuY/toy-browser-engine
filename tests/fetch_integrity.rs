use std::rc::Rc;

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};
use browser_engine::Browser;

const SHA256_OK: &str = "Jok2eyBcFs4y7UIAlCuLix4mLfxw2byfvHfElpmk8d8=";
const SHA512_OK: &str =
    "n7u7Wg8yn5eC4jVvpB2Jz5s2lDJ8GpNNavKp3y1/k2zoNxf7UTGWpM5VSEcXCM1xNMKumbPDV7yrsur8e5t1cA==";
const SHA256_FINAL: &str = "JENjC0YgFlyLFz5yZeF1Jv4nh65ZQ2TdbYOa1Y8vwAc=";
const SHA256_ACTUAL: &str = "5cb96GkQ3tcttcx6/DL4UEQNTvfKpdu2n1vcDT45yzs=";
const SHA256_EMPTY: &str = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=";

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn browser_for_page(page: &str, script: &str, transport: Rc<ManualNetwork>) -> Browser {
    let mut loader = MemoryLoader::new();
    loader.insert(page, format!("<script>{script}</script>"));
    Browser::open_with_single_hop_network(
        Box::new(loader),
        transport,
        &url(page),
        Rc::new(ManualClock::new()),
    )
    .expect("browser opens")
}

fn browser_for(script: &str, transport: Rc<ManualNetwork>) -> Browser {
    browser_for_page("http://page.test/index.html", script, transport)
}

fn response(endpoint: &str, status: u16, body: &[u8]) -> FetchResponse {
    FetchResponse::synthetic(url(endpoint), status, Some("text/plain"), body.to_vec())
}

#[test]
fn request_integrity_is_visible_cloned_and_overridable() {
    let transport = Rc::new(ManualNetwork::new());
    let browser = browser_for(
        &format!(
            r#"
                var original = new Request('/data', {{ integrity: 'sha256-{SHA256_OK}' }});
                var clone = new Request(original);
                var changed = new Request(original, {{ integrity: '' }});
                console.log(original.integrity);
                console.log(clone.integrity);
                console.log(changed.integrity);
                console.log(original.integrity);
            "#
        ),
        transport,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec![
            format!("sha256-{SHA256_OK}"),
            format!("sha256-{SHA256_OK}"),
            String::new(),
            format!("sha256-{SHA256_OK}"),
        ]
    );
}

#[test]
fn matching_integrity_resolves_and_mismatch_rejects() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/good",
        response("http://page.test/good", 200, b"ok"),
    );
    transport.respond(
        "http://page.test/bad",
        response("http://page.test/bad", 200, b"changed"),
    );

    let mut browser = browser_for(
        &format!(
            r#"
                fetch('/good', {{ integrity: 'sha256-{SHA256_OK}' }})
                  .then(function () {{ console.log('good'); }});
                fetch('/bad', {{ integrity: 'sha256-{SHA256_OK}' }})
                  .then(function () {{ console.log('unexpected'); }})
                  .catch(function () {{ console.log('blocked'); }});
            "#
        ),
        transport,
    );
    browser.settle_network(20);

    assert_eq!(browser.document().runtime.console, vec!["good", "blocked"]);
}

#[test]
fn strongest_algorithm_wins_and_same_strength_is_any_match() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/one",
        response("http://page.test/one", 200, b"ok"),
    );
    transport.respond(
        "http://page.test/two",
        response("http://page.test/two", 200, b"ok"),
    );

    let mut browser = browser_for(
        &format!(
            r#"
                fetch('/one', {{ integrity: 'sha256-{SHA256_OK} sha512-wrong' }})
                  .then(function () {{ console.log('unexpected'); }})
                  .catch(function () {{ console.log('strong-blocked'); }});
                fetch('/two', {{ integrity: 'sha512-wrong sha512-{SHA512_OK}' }})
                  .then(function () {{ console.log('same-strength-ok'); }});
            "#
        ),
        transport,
    );
    browser.settle_network(20);

    assert_eq!(
        browser.document().runtime.console,
        vec!["strong-blocked", "same-strength-ok"]
    );
}

#[test]
fn unsupported_algorithms_do_not_create_a_false_failure() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/data",
        response("http://page.test/data", 200, b"ok"),
    );

    let mut browser = browser_for(
        r#"
            fetch('/data', { integrity: 'md5-deadbeef sha999-anything' })
              .then(function () { console.log('done'); })
              .catch(function () { console.log('blocked'); });
        "#,
        transport,
    );
    browser.settle_network(10);

    assert_eq!(browser.document().runtime.console, vec!["done"]);
}

#[test]
fn cross_origin_no_cors_integrity_fails_before_transport() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    let mut browser = browser_for(
        &format!(
            r#"
                fetch('http://api.test/data', {{
                    mode: 'no-cors',
                    integrity: 'sha256-{SHA256_OK}'
                }}).catch(function () {{ console.log('blocked'); }});
            "#
        ),
        transport.clone(),
    );
    browser.settle_network(5);

    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
    assert!(transport.requests().is_empty());
}

#[test]
fn redirect_chain_checks_only_the_final_response_body() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    let mut first = response(
        "http://page.test/start",
        302,
        b"redirect-body-does-not-match",
    );
    first.headers.insert_raw("location", "/final");
    transport.respond("http://page.test/start", first);
    transport.respond(
        "http://page.test/final",
        response("http://page.test/final", 200, b"final"),
    );

    let mut browser = browser_for(
        &format!(
            r#"
                fetch('/start', {{ integrity: 'sha256-{SHA256_FINAL}' }})
                  .then(function (r) {{ return r.text(); }})
                  .then(function (text) {{ console.log(text); }})
                  .catch(function () {{ console.log('blocked'); }});
            "#
        ),
        transport.clone(),
    );
    browser.settle_network(20);

    assert_eq!(browser.document().runtime.console, vec!["final"]);
    assert_eq!(transport.requests().len(), 2);
}

#[test]
fn cors_preflight_is_not_integrity_checked_but_actual_response_is() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    let mut allowed = response("http://api.test/data", 200, b"actual");
    allowed
        .headers
        .insert_raw("access-control-allow-origin", "http://page.test");
    allowed
        .headers
        .insert_raw("access-control-allow-methods", "PUT");
    allowed
        .headers
        .insert_raw("access-control-allow-headers", "x-token");
    transport.respond("http://api.test/data", allowed);

    let mut browser = browser_for(
        &format!(
            r#"
                fetch('http://api.test/data', {{
                    method: 'PUT',
                    headers: {{ 'X-Token': 'secret' }},
                    body: 'payload',
                    integrity: 'sha256-{SHA256_ACTUAL}'
                }}).then(function () {{ console.log('done'); }})
                  .catch(function () {{ console.log('blocked'); }});
            "#
        ),
        transport.clone(),
    );
    browser.settle_network(20);

    assert_eq!(browser.document().runtime.console, vec!["done"]);
    let seen = transport.requests();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].method.as_str(), "OPTIONS");
    assert_eq!(seen[1].method.as_str(), "PUT");
}

#[test]
fn null_body_status_rejects_even_if_empty_bytes_would_hash_match() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/empty",
        response("http://page.test/empty", 204, b""),
    );

    let mut browser = browser_for(
        &format!(
            r#"
                fetch('/empty', {{ integrity: 'sha256-{SHA256_EMPTY}' }})
                  .then(function () {{ console.log('unexpected'); }})
                  .catch(function () {{ console.log('blocked'); }});
            "#
        ),
        transport,
    );
    browser.settle_network(10);

    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
}
