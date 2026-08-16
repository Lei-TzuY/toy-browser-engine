//! Drive the fetch page on a network under our control, and write one image
//! per stage.
//!
//! ```text
//! cargo run --example fetch_driver -- out_dir
//! ```
//!
//! Everything here is deterministic: a `ManualClock` for time and a
//! `ManualNetwork` for I/O. Nothing completes until this driver says so, which
//! is what makes the middle frame possible at all — a real network would give
//! you no reliable moment at which the request is provably still in flight.

use std::rc::Rc;
use std::time::Duration;

use browser_engine::{
    browser::Browser,
    document::PointerState,
    eventloop::ManualClock,
    net::{DefaultLoader, ManualNetwork, Method, NetworkBackend, Url},
    script::dom_api,
};

const VIEWPORT: (usize, usize) = (820, 1500);

fn main() {
    let directory = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    std::fs::create_dir_all(&directory).expect("output directory");

    let site = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("site");
    let page = Url::from_file_path(site.join("fetch.html"));

    // The page's own resources come off disk; its `fetch()` calls go here.
    let network = Rc::new(ManualNetwork::new());
    let data_url = Url::from_file_path(site.join("api/data.json")).to_string();
    network.respond_json(
        &data_url,
        &std::fs::read_to_string(site.join("api/data.json")).expect("fixture JSON"),
    );

    let mut browser = Browser::open_with_network(
        Box::new(DefaultLoader::new()),
        network.clone(),
        &page,
        Rc::new(ManualClock::new()),
    )
    .expect("fetch page loads");

    // ── 1. Before anything is asked for ───────────────────────────────────
    write(&browser, &directory, "fetch_0");
    println!(
        "sync                 status={:?}",
        text(&browser, "#status")
    );

    // ── 2. Click Load ─────────────────────────────────────────────────────
    let button = dom_api::get_element_by_id(&browser.document().dom, "load").expect("the button");
    browser.click_node(&button);
    println!(
        "clicked              status={:?}  in flight={}",
        text(&browser, "#status"),
        browser.document().in_flight_requests()
    );

    // One turn sends the request; it cannot be collected in the same turn.
    browser.advance_time(Duration::from_millis(16));
    let pending = network.pending();
    println!(
        "fetch pending        {} request(s): {}",
        pending.len(),
        pending
            .iter()
            .map(|(_, url)| url.rsplit('/').next().unwrap_or(url).to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    for request in network.requests() {
        println!(
            "  sent               {} {}",
            request.method,
            request.url.file_name()
        );
        assert_eq!(request.method, Method::Get);
    }
    write(&browser, &directory, "fetch_pending");
    println!(
        "still pending        rows={}  (the promise has not settled)",
        rows(&browser)
    );

    // ── 3. Let the network answer ─────────────────────────────────────────
    let completed = network.complete_all();
    println!("network completed    {completed} request(s) answered");

    // The next turn delivers the completion as a task, then drains the
    // microtasks it released: json(), then the handler that builds the DOM.
    let report = browser.advance_time(Duration::from_millis(16));
    println!(
        "response status 200  completions={}  (delivered as a task)",
        report.network_completions
    );
    println!(
        "json parsed          status={:?}",
        text(&browser, "#status")
    );
    println!("DOM updated          rows={}", rows(&browser));
    write(&browser, &directory, "fetch_done");

    // ── 4. The other buttons, for the finished frame ──────────────────────
    for (id, url, body) in [
        ("load-text", "api/note.txt", None),
        ("load-missing", "api/nowhere.json", Some(404)),
        ("post", "api/echo.json", None),
    ] {
        let target = Url::from_file_path(site.join(url)).to_string();
        match body {
            // Left unregistered on purpose: the manual network answers an
            // unknown URL with a 404, which is exactly what this demonstrates.
            Some(404) => {}
            _ => network.respond_with(
                &target,
                200,
                mime_for(url),
                std::fs::read(site.join(url)).expect("fixture file"),
            ),
        }
        let button = dom_api::get_element_by_id(&browser.document().dom, id).expect("a button");
        browser.click_node(&button);
        browser.advance_time(Duration::from_millis(16));
        network.complete_all();
        browser.advance_time(Duration::from_millis(16));
    }

    // Three at once, completed out of order, to show `Promise.all` keeping the
    // input order regardless.
    let button = dom_api::get_element_by_id(&browser.document().dom, "load-all").expect("button");
    browser.click_node(&button);
    browser.advance_time(Duration::from_millis(16));
    let ids: Vec<u64> = network.pending().into_iter().map(|(id, _)| id).collect();
    println!("\nPromise.all          {} requests in flight", ids.len());
    for id in ids.iter().rev() {
        network.complete(*id);
    }
    println!("  completed in reverse order");
    browser.advance_time(Duration::from_millis(16));
    println!("  result             {}", text(&browser, "#all"));

    // The cross-origin attempt needs no network at all: it is refused before
    // the request is ever recorded.
    let button = dom_api::get_element_by_id(&browser.document().dom, "fail").expect("the button");
    browser.click_node(&button);
    browser.advance_time(Duration::from_millis(16));

    println!("\n── the finished page ──");
    for selector in [
        "#status", "#note", "#missing", "#posted", "#all", "#failure",
    ] {
        println!("   {selector:<10} {}", text(&browser, selector));
    }
    write(&browser, &directory, "fetch_all");

    // Nothing left over: every answer was collected by the page.
    println!(
        "\nno stray completions left: {}",
        network.poll().is_empty() && network.pending_count() == 0
    );
    println!("frames written to {directory}/fetch_*.ppm");
}

fn mime_for(path: &str) -> &'static str {
    if path.ends_with(".json") {
        "application/json"
    } else {
        "text/plain; charset=utf-8"
    }
}

fn write(browser: &Browser, directory: &str, name: &str) {
    let canvas = browser.render(VIEWPORT.0, VIEWPORT.1, 0.0, &PointerState::default());
    std::fs::write(format!("{directory}/{name}.ppm"), canvas.to_ppm()).expect("write frame");
}

fn rows(browser: &Browser) -> usize {
    dom_api::query_selector_all(&browser.document().dom, &[], "#cards li").len()
}

fn text(browser: &Browser, selector: &str) -> String {
    let Some(path) = dom_api::query_selector(&browser.document().dom, &[], selector) else {
        return String::new();
    };
    dom_api::text_content(dom_api::node_at(&browser.document().dom, &path).expect("node"))
}
