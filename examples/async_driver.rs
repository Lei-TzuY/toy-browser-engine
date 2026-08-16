//! Drive the timers-and-animation page on virtual time and write one image per
//! sampled moment — the headless equivalent of watching the window.
//!
//! ```text
//! cargo run --example async_driver -- out_dir
//! ```
//!
//! Nothing here sleeps: the clock is advanced by hand, so the frames are
//! reproducible.

use std::rc::Rc;
use std::time::Duration;

use browser_engine::{
    browser::Browser,
    document::PointerState,
    eventloop::ManualClock,
    net::{DefaultLoader, Url},
    script::dom_api,
};

const VIEWPORT: (usize, usize) = (820, 1320);
/// How often the loop is stepped, roughly one 60Hz frame.
const FRAME_STEP: Duration = Duration::from_millis(16);
/// Moments to capture, in milliseconds since load.
const SAMPLES_MS: &[u64] = &[0, 100, 500, 1000, 2000, 5000];

fn main() {
    let directory = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    std::fs::create_dir_all(&directory).expect("output directory");

    let page = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("site")
        .join("async.html");

    // Virtual time: the page believes it is running, but nothing waits.
    let clock = Rc::new(ManualClock::new());
    let mut browser = Browser::open_with_clock(
        Box::new(DefaultLoader::new()),
        &Url::from_file_path(&page),
        clock.clone(),
    )
    .expect("async page loads");

    // Press "Start" so the button-driven job is running too.
    let start = dom_api::get_element_by_id(&browser.document().dom, "start").expect("#start");
    browser.click_node(&start);

    let mut elapsed_ms = 0u64;
    for sample in SAMPLES_MS {
        // Step in frame-sized slices up to the sample point, exactly as a
        // window would.
        while elapsed_ms < *sample {
            let step = FRAME_STEP.as_millis() as u64;
            let slice = step.min(sample - elapsed_ms);
            browser.advance_time(Duration::from_millis(slice));
            elapsed_ms += slice;
        }

        let canvas = browser.render(VIEWPORT.0, VIEWPORT.1, 0.0, &PointerState::default());
        let path = format!("{directory}/frame_{sample:04}.ppm");
        std::fs::write(&path, canvas.to_ppm()).expect("write frame");

        println!(
            "t={:>5}ms  timeout={:<28} counter={:<3} {:<22} job={:<10} rows={}",
            sample,
            text_of(&browser, "#timeout-status"),
            text_of(&browser, "#tick-count"),
            text_of(&browser, "#frame-status"),
            text_of(&browser, "#job-status"),
            dom_api::query_selector_all(&browser.document().dom, &[], "#generated li").len(),
        );
    }

    println!("\npending tasks: {}", browser.has_pending_tasks());
    println!("frames written to {directory}/frame_*.ppm");
}

fn text_of(browser: &Browser, selector: &str) -> String {
    let Some(path) = dom_api::query_selector(&browser.document().dom, &[], selector) else {
        return String::new();
    };
    dom_api::text_content(dom_api::node_at(&browser.document().dom, &path).expect("node"))
}
