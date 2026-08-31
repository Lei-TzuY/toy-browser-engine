use std::rc::Rc;

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};
use browser_engine::Browser;

const SHA512_OK: &str =
    "n7u7Wg8yn5eC4jVvpB2Jz5s2lDJ8GpNNavKp3y1/k2zoNxf7UTGWpM5VSEcXCM1xNMKumbPDV7yrsur8e5t1cA==";

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

#[test]
fn fetch_accepts_tab_separated_sri_metadata() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/data",
        FetchResponse::synthetic(
            url("http://page.test/data"),
            200,
            Some("text/plain"),
            b"ok".to_vec(),
        ),
    );

    let mut loader = MemoryLoader::new();
    loader.insert(
        "http://page.test/index.html",
        format!(
            r#"<script>
                fetch('/data', {{ integrity: 'sha256-wrong\tsha512-{SHA512_OK}' }})
                  .then(function () {{ console.log('verified'); }})
                  .catch(function () {{ console.log('blocked'); }});
            </script>"#
        ),
    );

    let mut browser = Browser::open_with_single_hop_network(
        Box::new(loader),
        transport.clone(),
        &url("http://page.test/index.html"),
        Rc::new(ManualClock::new()),
    )
    .expect("browser opens");
    browser.settle_network(10);

    assert_eq!(browser.document().runtime.console, vec!["verified"]);
    assert_eq!(transport.requests().len(), 1);
}
