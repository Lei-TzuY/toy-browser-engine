use browser_engine::document::Document;
use browser_engine::net::{MemoryLoader, Url};

fn run_js(js: &str) -> Document {
    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/app/index.html").unwrap();
    let full_html = format!(
        "<!DOCTYPE html><html><body><script>{}</script></body></html>",
        js
    );
    Document::from_html(&full_html, &url, &loader)
}

#[test]
fn test_abort_signal_timeout_and_any() {
    let doc = run_js(
        r#"
        // Test AbortSignal.abort()
        const s1 = AbortSignal.abort();
        console.log("s1:" + s1.aborted);

        // Test AbortSignal.timeout(0)
        const s2 = AbortSignal.timeout(0);
        console.log("s2:" + s2.aborted);

        // Test AbortSignal.any()
        const s3 = AbortSignal.any([s1]);
        console.log("s3:" + s3.aborted);

        // Test un-aborted controller signal in any()
        const c4 = new AbortController();
        const s4 = AbortSignal.any([c4.signal]);
        console.log("s4_before:" + s4.aborted);
        c4.abort();
        const s5 = AbortSignal.any([c4.signal]);
        console.log("s5_after:" + s5.aborted);
    "#,
    );

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "s1:true");
    assert_eq!(logs[1], "s2:true");
    assert_eq!(logs[2], "s3:true");
    assert_eq!(logs[3], "s4_before:false");
    assert_eq!(logs[4], "s5_after:true");
}

#[test]
fn timeout_retires_pending_fetch_and_preflight_at_deadline() {
    use browser_engine::net::{FetchResponse, ManualNetwork};
    use std::rc::Rc;

    for preflight in [false, true] {
        for ready in [false, true] {
            let endpoint = if preflight {
                "http://api.test/data"
            } else {
                "http://example.com/app/data"
            };
            let init = if preflight {
                "method: 'PUT', body: 'payload',"
            } else {
                ""
            };
            let script = format!(
                "fetch('{endpoint}', {{ {init} signal: AbortSignal.timeout(5) }})\
                 .then(function () {{ console.log('unexpected success'); }})\
                 .catch(function () {{ console.log('aborted'); }});"
            );
            let mut doc = run_js(&script);
            let network = Rc::new(ManualNetwork::new());
            let mut response = FetchResponse::synthetic(
                Url::parse(endpoint).unwrap(),
                200,
                Some("text/plain"),
                b"ok".to_vec(),
            );
            response
                .headers
                .insert_raw("access-control-allow-origin", "*");
            response
                .headers
                .insert_raw("access-control-allow-methods", "PUT");
            network.respond(endpoint, response);
            doc.set_network(network.clone());

            assert_eq!(doc.run_event_loop(0.0).requests_sent, 1);
            assert_eq!(
                network.requests()[0].method.as_str(),
                if preflight { "OPTIONS" } else { "GET" }
            );
            doc.run_event_loop(4.0);
            assert_eq!(doc.in_flight_requests(), 1, "timeout must not fire early");
            assert!(doc.runtime.console.is_empty());
            if ready {
                assert_eq!(network.complete_all(), 1);
            }

            doc.run_event_loop(5.0);
            assert_eq!(
                doc.runtime.console,
                vec!["aborted"],
                "preflight={preflight}, ready={ready}"
            );
            assert_eq!(doc.in_flight_requests(), 0);
            assert_eq!(network.pending_count(), 0);
            assert_eq!(
                network.requests().len(),
                1,
                "expired preflight must not send PUT"
            );
            assert_eq!(network.complete_all(), 0);
            doc.run_event_loop(6.0);
            assert!(!doc.has_pending_network());
            assert_eq!(doc.runtime.console, vec!["aborted"], "one rejection only");
        }
    }
}

#[test]
fn nested_any_cancels_on_later_abort_and_preserves_unrelated_fetch() {
    use browser_engine::net::ManualNetwork;
    use std::rc::Rc;

    let mut doc = run_js(
        r#"
        const controller = new AbortController();
        const combined = AbortSignal.any([
            AbortSignal.any([controller.signal]),
            AbortSignal.timeout(100)
        ]);
        fetch('/app/cancelled', { signal: combined })
            .then(function () { console.log('unexpected success'); })
            .catch(function () { console.log('aborted'); });
        fetch('/app/unrelated')
            .then(function () { console.log('unrelated'); });
        setTimeout(function () { controller.abort(); }, 2);
    "#,
    );
    let network = Rc::new(ManualNetwork::new());
    network.respond_text("http://example.com/app/cancelled", "cancelled");
    network.respond_text("http://example.com/app/unrelated", "ok");
    doc.set_network(network.clone());

    assert_eq!(doc.run_event_loop(0.0).requests_sent, 2);
    doc.run_event_loop(2.0);
    assert_eq!(doc.runtime.console, vec!["aborted"]);
    assert_eq!(doc.in_flight_requests(), 1, "unrelated fetch remains live");
    doc.run_event_loop(3.0);
    assert_eq!(network.pending_count(), 1);
    assert_eq!(network.complete_all(), 1);
    doc.run_event_loop(4.0);
    assert_eq!(doc.runtime.console, vec!["aborted", "unrelated"]);
    assert_eq!(doc.in_flight_requests(), 0);
    assert!(!doc.has_pending_network());
}

#[test]
fn expired_signal_rejects_fetch_created_by_a_later_timer() {
    use browser_engine::net::ManualNetwork;
    use std::rc::Rc;

    let mut doc = run_js(
        r#"
        const signal = AbortSignal.timeout(1);
        setTimeout(function () {
            fetch('/app/never', { signal: signal })
                .catch(function () { console.log('aborted'); });
        }, 2);
    "#,
    );
    let network = Rc::new(ManualNetwork::new());
    doc.set_network(network.clone());

    doc.run_event_loop(2.0);
    assert_eq!(doc.runtime.console, vec!["aborted"]);
    assert_eq!(doc.in_flight_requests(), 0);
    doc.run_event_loop(3.0);
    assert_eq!(doc.runtime.console, vec!["aborted"]);
    assert!(
        network.requests().is_empty(),
        "already expired signal must not dispatch"
    );
    assert_eq!(doc.in_flight_requests(), 0);
    assert!(!doc.has_pending_network());
}
