//! Drive the promises page on virtual time and write one image per stage.
//!
//! ```text
//! cargo run --example promise_driver -- out_dir
//! ```
//!
//! The interesting property is that the first stage needs *no* time at all:
//! microtasks run at the checkpoint after the page's scripts, so the ordering
//! log is already complete before the clock moves.

use std::rc::Rc;
use std::time::Duration;

use browser_engine::{
    browser::Browser,
    document::PointerState,
    eventloop::ManualClock,
    net::{DefaultLoader, Url},
    script::dom_api,
};

const VIEWPORT: (usize, usize) = (820, 1180);
/// Moments to capture, in milliseconds since load.
const SAMPLES_MS: &[u64] = &[0, 50, 300, 600, 1000];

fn main() {
    let directory = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    std::fs::create_dir_all(&directory).expect("output directory");

    let page = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("site")
        .join("promise.html");

    let clock = Rc::new(ManualClock::new());
    let mut browser = Browser::open_with_clock(
        Box::new(DefaultLoader::new()),
        &Url::from_file_path(&page),
        clock.clone(),
    )
    .expect("promise page loads");

    println!("── after load, before any time passes ──");
    for line in log_lines(&browser) {
        println!("   {line}");
    }
    println!();

    let mut elapsed = 0u64;
    for sample in SAMPLES_MS {
        // Step in frame-sized slices, as the window does.
        while elapsed < *sample {
            let slice = 16u64.min(sample - elapsed);
            browser.advance_time(Duration::from_millis(slice));
            elapsed += slice;
        }

        let canvas = browser.render(VIEWPORT.0, VIEWPORT.1, 0.0, &PointerState::default());
        let path = format!("{directory}/promise_{sample:04}.ppm");
        std::fs::write(&path, canvas.to_ppm()).expect("write frame");

        println!(
            "t={:>5}ms  chain={:<22} rejection={:<34} all={:<40} rows={}",
            sample,
            text_of(&browser, "#chain"),
            text_of(&browser, "#rejection"),
            text_of(&browser, "#all"),
            dom_api::query_selector_all(&browser.document().dom, &[], "#rows li").len(),
        );
    }

    println!("\n── final execution order ──");
    for line in log_lines(&browser) {
        println!("   {line}");
    }
    println!("\nframes written to {directory}/promise_*.ppm");
}

fn log_lines(browser: &Browser) -> Vec<String> {
    dom_api::query_selector_all(&browser.document().dom, &[], "#order-log li")
        .iter()
        .filter_map(|path| dom_api::node_at(&browser.document().dom, path))
        .map(dom_api::text_content)
        .collect()
}

fn text_of(browser: &Browser, selector: &str) -> String {
    let Some(path) = dom_api::query_selector(&browser.document().dom, &[], selector) else {
        return String::new();
    };
    dom_api::text_content(dom_api::node_at(&browser.document().dom, &path).expect("node"))
}
