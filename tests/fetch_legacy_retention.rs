use std::rc::Rc;

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchError, FetchResponse, ManualNetwork, MemoryLoader, Url};
use browser_engine::Browser;

const SHA256_OK: &str = "Jok2eyBcFs4y7UIAlCuLix4mLfxw2byfvHfElpmk8d8=";
const SHA384_OK: &str = "xGY5+2SLy7sVzWpRRtIYtTmlK8feFq07VSi1ixAtRoi5HuFeR2Lz+5roxSCgi6m1";
const SHA256_EMPTY: &str = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=";
const INTERNAL_ORIGIN: &str = "x-browser-internal-cors-origin";

fn url(value: &str) -> Url {
    Url::parse(value).expect("valid URL")
}

fn browser(page: &str, script: &str, transport: Rc<ManualNetwork>) -> Browser {
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

fn response(endpoint: &str, status: u16, body: &[u8]) -> FetchResponse {
    FetchResponse::synthetic(url(endpoint), status, Some("text/plain"), body.to_vec())
}

fn redirect(endpoint: &str, target: &str) -> FetchResponse {
    let mut result = response(endpoint, 302, b"redirect");
    result.headers.insert_raw("location", target);
    result
}

fn permission(endpoint: &str, origin: &str, body: &[u8]) -> FetchResponse {
    let mut result = response(endpoint, 200, body);
    result
        .headers
        .insert_raw("access-control-allow-origin", origin);
    result
        .headers
        .insert_raw("access-control-allow-methods", "PUT");
    result
        .headers
        .insert_raw("access-control-allow-headers", "x-token");
    result.headers.insert_raw("access-control-max-age", "60");
    result
}

fn dispatch_until(browser: &mut Browser, transport: &ManualNetwork, count: usize) {
    for _ in 0..8 {
        browser.tick();
        if transport.requests().len() >= count {
            break;
        }
    }
    assert_eq!(transport.requests().len(), count);
    assert_eq!(transport.pending_count(), 1);
}

fn complete(transport: &ManualNetwork, result: Result<FetchResponse, FetchError>) {
    let pending = transport.pending();
    assert_eq!(pending.len(), 1);
    assert!(transport.complete_with(pending[0].0, result));
}

#[test]
fn redirects_reselect_credentials_and_samesite_for_the_target_origin() {
    for credentials in ["same-origin", "include", "omit"] {
        let transport = Rc::new(ManualNetwork::new());
        transport.set_auto_complete(true);
        transport.respond(
            "https://page.test/start",
            redirect("https://page.test/start", "https://api.test/data"),
        );
        let mut allowed = permission("https://api.test/data", "https://page.test", b"ok");
        allowed
            .headers
            .insert_raw("access-control-allow-credentials", "true");
        transport.respond("https://api.test/data", allowed);
        let mut browser = browser(
            "https://page.test/index.html",
            &format!("fetch('/start', {{ credentials: '{credentials}' }}).then(function () {{ console.log('done'); }});"),
            transport.clone(),
        );
        for (cookie, origin) in [
            (
                "page=one; Path=/; Secure; SameSite=Strict",
                "https://page.test/",
            ),
            (
                "allowed=two; Path=/; Secure; SameSite=None",
                "https://api.test/",
            ),
            (
                "strict=three; Path=/; Secure; SameSite=Strict",
                "https://api.test/",
            ),
            (
                "lax=four; Path=/; Secure; SameSite=Lax",
                "https://api.test/",
            ),
        ] {
            assert!(browser
                .cookie_jar()
                .borrow_mut()
                .store_set_cookie(cookie, &url(origin), 0));
        }
        browser.settle_network(12);
        assert_eq!(
            browser.document().runtime.console,
            vec!["done"],
            "{credentials}"
        );
        let seen = transport.requests();
        assert_eq!(seen.len(), 2);
        assert_eq!(
            seen[0].headers.get("cookie").as_deref(),
            if credentials == "omit" {
                None
            } else {
                Some("page=one")
            }
        );
        assert_eq!(
            seen[1].headers.get("cookie").as_deref(),
            if credentials == "include" {
                Some("allowed=two")
            } else {
                None
            }
        );
    }
}

#[test]
fn server_cannot_forge_or_expose_the_internal_redirect_origin() {
    for (allowed_origin, expected) in [("http://page.test", "cors:null"), ("null", "blocked")] {
        let transport = Rc::new(ManualNetwork::new());
        transport.set_auto_complete(true);
        transport.respond(
            "http://page.test/start",
            redirect("http://page.test/start", "http://api.test/data"),
        );
        let mut final_response = permission("http://api.test/data", allowed_origin, b"ok");
        final_response.headers.insert_raw(INTERNAL_ORIGIN, "null");
        final_response
            .headers
            .insert_raw("access-control-expose-headers", INTERNAL_ORIGIN);
        transport.respond("http://api.test/data", final_response);
        let mut browser = browser(
            "http://page.test/index.html",
            "fetch('/start').then(function (r) { console.log(r.type + ':' + r.headers.get('x-browser-internal-cors-origin')); }).catch(function () { console.log('blocked'); });",
            transport.clone(),
        );
        browser.settle_network(12);
        assert_eq!(browser.document().runtime.console, vec![expected]);
        assert_eq!(transport.requests().len(), 2);
    }
}

#[test]
fn credentialed_intermediate_redirect_requires_acac_before_following() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    let mut first = redirect("https://api.test/start", "https://cdn.test/data");
    first
        .headers
        .insert_raw("access-control-allow-origin", "https://page.test");
    transport.respond("https://api.test/start", first);
    let mut browser = browser(
        "https://page.test/index.html",
        "fetch('https://api.test/start', { credentials: 'include' }).then(function () { console.log('unexpected'); }).catch(function () { console.log('blocked'); });",
        transport.clone(),
    );
    browser.settle_network(8);
    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
    assert_eq!(transport.requests().len(), 1);
}

#[test]
fn redirect_preflight_errors_never_dispatch_an_actual_or_location_target() {
    for network_error in [false, true] {
        let transport = Rc::new(ManualNetwork::new());
        transport.set_auto_complete(true);
        transport.respond(
            "http://page.test/start",
            redirect("http://page.test/start", "http://api.test/data"),
        );
        if network_error {
            transport.fail(
                "http://api.test/data",
                FetchError::Io("preflight failed".into()),
            );
        } else {
            let mut redirected =
                permission("http://api.test/data", "http://page.test", b"not actual");
            redirected.status = 302;
            redirected
                .headers
                .insert_raw("location", "http://other.test/never");
            transport.respond("http://api.test/data", redirected);
        }
        let mut browser = browser(
            "http://page.test/index.html",
            "fetch('/start', { method: 'PUT', headers: { 'X-Token': 'one' } }).then(function () { console.log('unexpected'); }).catch(function () { console.log('blocked'); });",
            transport.clone(),
        );
        browser.settle_network(12);
        assert_eq!(browser.document().runtime.console, vec!["blocked"]);
        let seen = transport.requests();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[1].method.as_str(), "OPTIONS");
        assert_eq!(seen[1].url.to_string(), "http://api.test/data");
    }
}

#[test]
fn redirected_actual_failure_revokes_the_shared_preflight_grant() {
    let transport = Rc::new(ManualNetwork::new());
    let mut browser = browser(
        "http://page.test/index.html",
        "fetch('/start', { method: 'PUT', headers: { 'X-Token': 'one' } }).catch(function () { return fetch('http://api.test/data', { method: 'PUT', headers: { 'X-Token': 'two' } }); }).then(function () { console.log('done'); });",
        transport.clone(),
    );
    dispatch_until(&mut browser, &transport, 1);
    complete(
        &transport,
        Ok(redirect("http://page.test/start", "http://api.test/data")),
    );
    dispatch_until(&mut browser, &transport, 2);
    complete(
        &transport,
        Ok(permission(
            "http://api.test/data",
            "http://page.test",
            b"preflight",
        )),
    );
    dispatch_until(&mut browser, &transport, 3);
    complete(&transport, Err(FetchError::Io("actual failed".into())));
    dispatch_until(&mut browser, &transport, 4);
    assert_eq!(
        transport
            .requests()
            .iter()
            .map(|r| r.method.as_str())
            .collect::<Vec<_>>(),
        vec!["PUT", "OPTIONS", "PUT", "OPTIONS"]
    );
    complete(
        &transport,
        Ok(permission(
            "http://api.test/data",
            "http://page.test",
            b"preflight",
        )),
    );
    dispatch_until(&mut browser, &transport, 5);
    complete(
        &transport,
        Ok(permission(
            "http://api.test/data",
            "http://page.test",
            b"ok",
        )),
    );
    browser.settle_network(8);
    assert_eq!(browser.document().runtime.console, vec!["done"]);
}

#[test]
fn tainted_redirect_preflight_grant_cannot_authorize_an_untainted_direct_fetch() {
    let transport = Rc::new(ManualNetwork::new());
    let mut browser = browser(
        "http://page.test/index.html",
        "fetch('http://api.test/start', { method: 'PUT', headers: { 'X-Token': 'one' } }).then(function () { return fetch('http://cdn.test/data', { method: 'PUT', headers: { 'X-Token': 'two' } }); }).then(function () { console.log('done'); });",
        transport.clone(),
    );
    let steps = [
        ("http://api.test/start", "OPTIONS", "http://page.test"),
        ("http://api.test/start", "PUT", "http://page.test"),
        ("http://cdn.test/data", "OPTIONS", "null"),
        ("http://cdn.test/data", "PUT", "null"),
        ("http://cdn.test/data", "OPTIONS", "http://page.test"),
        ("http://cdn.test/data", "PUT", "http://page.test"),
    ];
    for (index, (endpoint, method, origin)) in steps.iter().enumerate() {
        dispatch_until(&mut browser, &transport, index + 1);
        let request = transport.requests().last().unwrap().clone();
        assert_eq!(request.url.to_string(), *endpoint);
        assert_eq!(request.method.as_str(), *method);
        assert_eq!(request.headers.get("origin").as_deref(), Some(*origin));
        let mut result = permission(endpoint, origin, b"ok");
        if index == 1 {
            result.status = 302;
            result
                .headers
                .insert_raw("location", "http://cdn.test/data");
        }
        complete(&transport, Ok(result));
    }
    browser.settle_network(8);
    assert_eq!(browser.document().runtime.console, vec!["done"]);
}

#[test]
fn redirect_referrer_uses_the_hsts_effective_target_before_cookie_selection() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    let mut first = redirect("https://page.test/start", "http://page.test/final");
    first
        .headers
        .insert_raw("strict-transport-security", "max-age=600");
    transport.respond("https://page.test/start", first);
    transport.respond(
        "https://page.test/final",
        response("https://page.test/final", 200, b"ok"),
    );
    let mut browser = browser(
        "https://page.test/private/index.html?q=1",
        "fetch('/start').then(function () { console.log('done'); });",
        transport.clone(),
    );
    assert!(browser.cookie_jar().borrow_mut().store_set_cookie(
        "secure=one; Path=/; Secure; SameSite=Strict",
        &url("https://page.test/"),
        0
    ));
    browser.settle_network(12);
    assert_eq!(browser.document().runtime.console, vec!["done"]);
    let seen = transport.requests();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1].url.to_string(), "https://page.test/final");
    assert_eq!(
        seen[1].headers.get("referer").as_deref(),
        Some("https://page.test/private/index.html?q=1")
    );
    assert_eq!(seen[1].headers.get("cookie").as_deref(), Some("secure=one"));
}

#[test]
fn preflight_cannot_mutate_referrer_or_enforce_integrity_on_its_body() {
    for redirected in [false, true] {
        let transport = Rc::new(ManualNetwork::new());
        let target = if redirected {
            "/start"
        } else {
            "http://api.test/data"
        };
        let mut browser = browser(
        "http://page.test/private/index.html?q=1",
        &format!("fetch('{target}', {{ method: 'PUT', headers: {{ 'X-Token': 'one' }}, referrerPolicy: 'unsafe-url', integrity: 'sha256-{SHA256_OK}' }}).then(function (r) {{ return r.text(); }}).then(function (text) {{ console.log(text); }}).catch(function () {{ console.log('blocked'); }});"),
        transport.clone(),
    );
        dispatch_until(&mut browser, &transport, 1);
        if redirected {
            let mut first = redirect("http://page.test/start", "http://api.test/data");
            first.headers.insert_raw("referrer-policy", "origin");
            complete(&transport, Ok(first));
            dispatch_until(&mut browser, &transport, 2);
        }
        let options_index = usize::from(redirected);
        let expected_referrer = if redirected {
            "http://page.test/"
        } else {
            "http://page.test/private/index.html?q=1"
        };
        let options = transport.requests()[options_index].clone();
        assert_eq!(options.method.as_str(), "OPTIONS");
        assert_eq!(
            options
                .headers
                .get("access-control-request-headers")
                .as_deref(),
            Some("x-token")
        );
        assert_eq!(
            options.headers.get("referer").as_deref(),
            Some(expected_referrer)
        );
        let mut allowed = permission(
            "http://api.test/data",
            "http://page.test",
            b"OPTIONS bytes deliberately do not match",
        );
        allowed.headers.insert_raw("referrer-policy", "no-referrer");
        complete(&transport, Ok(allowed));
        dispatch_until(&mut browser, &transport, options_index + 2);
        let actual = transport.requests()[options_index + 1].clone();
        assert_eq!(actual.method.as_str(), "PUT");
        assert_eq!(
            actual.headers.get("referer").as_deref(),
            Some(expected_referrer)
        );
        assert!(!actual.headers.has("integrity"));
        if redirected {
            let mut next = permission(
                "http://api.test/data",
                "http://page.test",
                b"another redirect body that does not match",
            );
            next.status = 302;
            next.headers.insert_raw("location", "/final");
            complete(&transport, Ok(next));
            dispatch_until(&mut browser, &transport, 4);
            let options = transport.requests()[3].clone();
            assert_eq!(options.url.to_string(), "http://api.test/final");
            assert_eq!(options.method.as_str(), "OPTIONS");
            assert_eq!(
                options.headers.get("referer").as_deref(),
                Some(expected_referrer)
            );
            let mut allowed = permission(
                "http://api.test/final",
                "http://page.test",
                b"second OPTIONS body also does not match",
            );
            allowed.headers.insert_raw("referrer-policy", "no-referrer");
            complete(&transport, Ok(allowed));
            dispatch_until(&mut browser, &transport, 5);
            let final_request = transport.requests()[4].clone();
            assert_eq!(final_request.method.as_str(), "PUT");
            assert_eq!(
                final_request.headers.get("referer").as_deref(),
                Some(expected_referrer)
            );
            complete(
                &transport,
                Ok(permission(
                    "http://api.test/final",
                    "http://page.test",
                    b"ok",
                )),
            );
        } else {
            complete(
                &transport,
                Ok(permission(
                    "http://api.test/data",
                    "http://page.test",
                    b"ok",
                )),
            );
        }
        browser.settle_network(8);
        assert_eq!(browser.document().runtime.console, vec!["ok"]);
    }
}

#[test]
fn integrity_rejects_head_and_manual_redirect_before_body_exposure() {
    for manual in [false, true] {
        let transport = Rc::new(ManualNetwork::new());
        transport.set_auto_complete(true);
        let mut result = if manual {
            redirect("http://page.test/data", "/never")
        } else {
            response("http://page.test/data", 200, b"")
        };
        result.body.clear();
        transport.respond("http://page.test/data", result);
        let init = if manual {
            "redirect: 'manual'"
        } else {
            "method: 'HEAD'"
        };
        let mut browser = browser(
            "http://page.test/index.html",
            &format!("fetch('/data', {{ {init}, integrity: 'sha256-{SHA256_EMPTY}' }}).then(function () {{ console.log('unexpected'); }}).catch(function () {{ console.log('blocked'); }});"),
            transport.clone(),
        );
        browser.settle_network(8);
        assert_eq!(browser.document().runtime.console, vec!["blocked"]);
        assert_eq!(transport.requests().len(), 1);
    }
}

#[test]
fn fetch_metadata_overrides_are_isolated_and_sha384_options_match() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/data",
        response("http://page.test/data", 200, b"ok"),
    );
    let mut browser = browser(
        "http://page.test/private/index.html?q=1",
        &format!("var original = new Request('/data', {{ integrity: 'sha512-wrong', referrer: '', referrerPolicy: 'no-referrer' }}); fetch(original, {{ integrity: 'sha384-{SHA384_OK}?option', referrer: 'about:client', referrerPolicy: 'unsafe-url' }}).then(function () {{ console.log('done'); console.log(original.integrity); console.log(original.referrer); console.log(original.referrerPolicy); }}).catch(function () {{ console.log('blocked'); }});"),
        transport.clone(),
    );
    browser.settle_network(8);
    assert_eq!(
        browser.document().runtime.console,
        vec!["done", "sha512-wrong", "", "no-referrer"]
    );
    let seen = transport.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].headers.get("referer").as_deref(),
        Some("http://page.test/private/index.html?q=1")
    );
    assert!(!seen[0].headers.has("integrity"));
    assert!(!seen[0].headers.has("referrer-policy"));
}

#[test]
fn cors_failure_precedes_integrity_and_cannot_be_rescued_by_a_matching_digest() {
    for digest in [SHA256_OK, "wrong"] {
        let transport = Rc::new(ManualNetwork::new());
        transport.set_auto_complete(true);
        transport.respond(
            "http://api.test/data",
            response("http://api.test/data", 200, b"ok"),
        );
        let mut browser = browser(
            "http://page.test/index.html",
            &format!("fetch('http://api.test/data', {{ integrity: 'sha256-{digest}' }}).then(function () {{ console.log('unexpected'); }}).catch(function (error) {{ console.log(error); }});"),
            transport.clone(),
        );
        browser.settle_network(8);
        let console = &browser.document().runtime.console;
        assert_eq!(console.len(), 1);
        assert!(console[0].contains("CORS:"), "{}", console[0]);
        assert!(!console[0].contains("Integrity"));
        assert_eq!(transport.requests().len(), 1);
    }
}
