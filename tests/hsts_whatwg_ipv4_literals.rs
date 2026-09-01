use std::rc::Rc;

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchRequest, FetchResponse, ManualNetwork, NetworkBackend, Url};
use browser_engine::HstsNetwork;

fn url(input: &str) -> Url {
    Url::parse(input).unwrap()
}

#[test]
fn legacy_numeric_ipv4_hosts_never_learn_or_apply_hsts() {
    for (index, host) in [
        "127.1",
        "2130706433",
        "0x7f000001",
        "0177.0.0.1",
    ]
    .into_iter()
    .enumerate()
    {
        let clock = Rc::new(ManualClock::new());
        let transport = Rc::new(ManualNetwork::new());
        transport.set_auto_complete(true);

        let bootstrap_url = format!("https://{host}/bootstrap");
        let plain_url = format!("http://{host}/plain");

        let mut bootstrap = FetchResponse::synthetic(
            url(&bootstrap_url),
            200,
            Some("text/plain"),
            Vec::new(),
        );
        bootstrap.headers.append_raw(
            "strict-transport-security",
            "max-age=600; includeSubDomains",
        );
        transport.respond(&bootstrap_url, bootstrap);
        transport.respond_text(&plain_url, "plain");

        let network = HstsNetwork::with_new_cache(transport.clone(), clock);
        network.start(index as u64 * 2 + 1, FetchRequest::get(url(&bootstrap_url)));
        assert_eq!(network.poll().len(), 1, "bootstrap completion for {host}");
        assert!(
            network.cache().borrow().is_empty(),
            "legacy IPv4 spelling {host} must not create HSTS state"
        );

        network.start(index as u64 * 2 + 2, FetchRequest::get(url(&plain_url)));
        assert_eq!(
            transport.requests().last().unwrap().url.to_string(),
            plain_url,
            "legacy IPv4 spelling {host} must stay HTTP"
        );
    }
}

#[test]
fn ordinary_numeric_dns_labels_are_not_misclassified_as_ipv4() {
    let clock = Rc::new(ManualClock::new());
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);

    let mut bootstrap = FetchResponse::synthetic(
        url("https://123.example.test/bootstrap"),
        200,
        Some("text/plain"),
        Vec::new(),
    );
    bootstrap
        .headers
        .append_raw("strict-transport-security", "max-age=600");
    transport.respond("https://123.example.test/bootstrap", bootstrap);
    transport.respond_text("https://123.example.test/data", "upgraded");

    let network = HstsNetwork::with_new_cache(transport.clone(), clock);
    network.start(
        1,
        FetchRequest::get(url("https://123.example.test/bootstrap")),
    );
    assert_eq!(network.poll().len(), 1);
    assert!(
        network
            .cache()
            .borrow()
            .is_known_host("123.example.test", 0)
    );

    network.start(2, FetchRequest::get(url("http://123.example.test/data")));
    assert_eq!(
        transport.requests().last().unwrap().url.to_string(),
        "https://123.example.test/data"
    );
}
