//! End-to-end tests over the real fixture site in `examples/site/`.
//!
//! These go through the whole browser: fetch a document from disk, resolve and
//! load its stylesheet, script and images, lay the page out, paint it, click
//! things and navigate. Nothing here touches the network.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use browser_engine::{
    browser::{Browser, ClickOutcome},
    document::PointerState,
    eventloop::ManualClock,
    input::{Key, KeyEvent, Modifiers},
    net::{DefaultLoader, FileLoader, ManualNetwork, Method, ResourceLoader, Url},
    script::dom_api,
    Document,
};

const VIEWPORT: (usize, usize) = (800, 1400);

fn site_path(relative: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("examples");
    path.push("site");
    for part in relative.split('/') {
        path.push(part);
    }
    path
}

fn site_url(relative: &str) -> Url {
    Url::from_file_path(site_path(relative))
}

fn open_site() -> Browser {
    Browser::open(Box::new(DefaultLoader::new()), &site_url("index.html"))
        .expect("fixture site loads")
}

fn text_of(document: &Document, selector: &str) -> String {
    let path = dom_api::query_selector(&document.dom, &[], selector)
        .unwrap_or_else(|| panic!("no element matched {selector:?}"));
    dom_api::text_content(dom_api::node_at(&document.dom, &path).unwrap())
}

// ── Loading ───────────────────────────────────────────────────────────────────

#[test]
fn loads_the_fixture_site_from_the_filesystem() {
    let browser = open_site();
    let document = browser.document();

    assert_eq!(document.title().as_deref(), Some("Browser Engine Toy"));
    // The only expected diagnostic is the deliberately missing image.
    let unexpected: Vec<_> = document
        .diagnostics
        .iter()
        .filter(|d| !d.url.contains("missing.png"))
        .collect();
    assert!(
        unexpected.is_empty(),
        "unexpected load failures: {unexpected:?}"
    );
}

#[test]
fn external_stylesheet_reaches_the_cascade() {
    let browser = open_site();
    let styled = browser
        .document()
        .style_tree(VIEWPORT.0 as f32, &PointerState::default());

    fn find_class<'a>(
        node: &'a browser_engine::style::StyledNode<'a>,
        class: &str,
    ) -> Option<&'a browser_engine::style::StyledNode<'a>> {
        let matches = node
            .node
            .as_element()
            .and_then(|e| e.get_attr("class"))
            .is_some_and(|c| c.split_whitespace().any(|token| token == class));
        if matches {
            return Some(node);
        }
        node.children.iter().find_map(|c| find_class(c, class))
    }

    // `.nav-link` only exists in the external stylesheet.
    let nav = find_class(&styled, "nav-link").expect("nav link is styled");
    assert!(
        nav.value("background-color").is_some(),
        "external rule applied"
    );

    // The inline <style> block follows the <link>, so it wins for `.intro`.
    let intro = find_class(&styled, "intro").expect("intro paragraph");
    assert_eq!(
        intro.value("color"),
        Some(&browser_engine::css::parser::Value::Color(
            browser_engine::css::parser::Color::rgb(74, 85, 104)
        )),
        "the later inline sheet should win"
    );
}

#[test]
fn external_script_runs_and_mutates_the_dom() {
    let browser = open_site();
    let document = browser.document();

    // app.js sets the button label and tags the grid headings.
    assert_eq!(text_of(document, "#counter"), "Clicks: 0");
    let tagged = dom_api::query_selector_all(&document.dom, &[], "h3[data-scripted]");
    assert_eq!(
        tagged.len(),
        6,
        "every pipeline cell heading should be tagged"
    );
}

#[test]
fn images_load_decode_and_size_the_layout() {
    let browser = open_site();
    let document = browser.document();

    let logo = document
        .images
        .get(&site_url("logo.png"))
        .expect("PNG decoded");
    assert_eq!((logo.width, logo.height), (160, 96));
    let photo = document
        .images
        .get(&site_url("photo.jpg"))
        .expect("JPEG decoded");
    assert_eq!((photo.width, photo.height), (240, 160));
    let icon = document
        .images
        .get(&site_url("assets/icon.png"))
        .expect("PNG in a subdirectory decoded");
    assert_eq!((icon.width, icon.height), (64, 64));

    // A missing image is remembered as broken rather than retried.
    assert!(document.images.error(&site_url("missing.png")).is_some());
}

#[test]
fn width_attribute_scales_the_height_by_the_aspect_ratio() {
    let browser = open_site();
    let document = browser.document();
    let styled = document.style_tree(VIEWPORT.0 as f32, &PointerState::default());
    let layout = document.layout(&styled, VIEWPORT.0 as f32);

    fn find_img<'a>(
        layout: &'a browser_engine::layout::LayoutBox<'a>,
        alt: &str,
    ) -> Option<&'a browser_engine::layout::LayoutBox<'a>> {
        let matches = layout
            .styled_node()
            .and_then(|s| s.node.as_element())
            .is_some_and(|e| e.tag_name == "img" && e.get_attr("alt") == Some(alt));
        if matches {
            return Some(layout);
        }
        layout.children.iter().find_map(|c| find_img(c, alt))
    }

    // photo.jpg is 240x160; width="220" must give height 220 * 160/240 ≈ 146.7.
    let photo = find_img(&layout, "Generated JPEG").expect("JPEG box");
    assert!((photo.dimensions.content.width - 220.0).abs() < 0.5);
    assert!(
        (photo.dimensions.content.height - 146.7).abs() < 1.0,
        "height should follow the aspect ratio, got {}",
        photo.dimensions.content.height
    );

    // The logo has no attributes: CSS gives it width 96, height follows.
    let logo = find_img(&layout, "Toy engine logo").expect("logo box");
    assert!((logo.dimensions.content.width - 96.0).abs() < 0.5);
    assert!(
        (logo.dimensions.content.height - 57.6).abs() < 1.0,
        "got {}",
        logo.dimensions.content.height
    );

    // The broken image falls back to a box sized for its alt text.
    let broken = find_img(&layout, "Missing file").expect("broken image box");
    assert!(broken.dimensions.content.width > 0.0);
    assert!(broken.image().is_none());
}

#[test]
fn decoded_pixels_reach_the_canvas() {
    let browser = open_site();
    let with_images = browser.render(VIEWPORT.0, VIEWPORT.1, 0.0, &PointerState::default());

    // Render the same document with an empty image cache: the pixels must differ.
    let mut without = Document::load(&site_url("index.html"), &FileLoader).expect("loads");
    without.images = browser_engine::image::ImageCache::new();
    let blank = without.render(VIEWPORT.0, VIEWPORT.1, 0.0, &PointerState::default());

    assert_eq!(with_images.to_ppm().len(), blank.to_ppm().len());
    assert_ne!(
        with_images.to_ppm(),
        blank.to_ppm(),
        "painting real bitmaps must change the output"
    );
}

// ── Navigation ────────────────────────────────────────────────────────────────

#[test]
fn clicking_a_link_navigates_and_back_returns() {
    let mut browser = open_site();
    let about = dom_api::query_selector(&browser.document().dom, &[], ".nav-link").unwrap();

    let outcome = browser.click_node(&about);
    assert!(matches!(outcome, ClickOutcome::Navigated(_)), "{outcome:?}");
    assert_eq!(
        browser.document().title().as_deref(),
        Some("About — Browser Engine Toy")
    );
    // The second page pulls its stylesheet and images through ../ references.
    assert!(browser
        .document()
        .images
        .get(&site_url("logo.png"))
        .is_some());
    assert!(browser
        .document()
        .images
        .get(&site_url("assets/icon.png"))
        .is_some());

    assert!(browser.back());
    assert_eq!(
        browser.document().title().as_deref(),
        Some("Browser Engine Toy")
    );
    assert!(browser.can_go_forward());
}

#[test]
fn clicking_the_button_runs_the_script_without_navigating() {
    let mut browser = open_site();
    let button = dom_api::get_element_by_id(&browser.document().dom, "counter").unwrap();

    for expected in 1..=3 {
        assert_eq!(browser.click_node(&button), ClickOutcome::Script);
        assert_eq!(
            text_of(browser.document(), "#counter"),
            format!("Clicks: {expected}")
        );
    }
    // The script appended one log entry per click.
    assert_eq!(
        dom_api::query_selector_all(&browser.document().dom, &[], "#log li").len(),
        3
    );
    assert_eq!(browser.history().len(), 1);
}

#[test]
fn the_rendered_page_changes_after_a_scripted_click() {
    let mut browser = open_site();
    let before = browser
        .render(VIEWPORT.0, 400, 0.0, &PointerState::default())
        .to_ppm();

    let button = dom_api::get_element_by_id(&browser.document().dom, "counter").unwrap();
    browser.click_node(&button);

    let after = browser
        .render(VIEWPORT.0, 400, 0.0, &PointerState::default())
        .to_ppm();
    assert_ne!(before, after, "the click should be visible on screen");
}

#[test]
fn hit_testing_a_link_finds_it_through_its_text() {
    let mut browser = open_site();
    let link = dom_api::query_selector(&browser.document().dom, &[], ".nav-link").unwrap();

    // Find the link's centre through layout, then click that point.
    let (x, y) = {
        let document = browser.document();
        let styled = document.style_tree(VIEWPORT.0 as f32, &PointerState::default());
        let layout = document.layout(&styled, VIEWPORT.0 as f32);

        fn centre(
            layout: &browser_engine::layout::LayoutBox,
            dom: &browser_engine::dom::Node,
            target: &[usize],
        ) -> Option<(f32, f32)> {
            if let Some(node) = layout.styled_node() {
                if dom_api::path_of(dom, node.node).as_deref() == Some(target) {
                    let b = layout.dimensions.border_box();
                    return Some((b.x + b.width / 2.0, b.y + b.height / 2.0));
                }
            }
            layout.children.iter().find_map(|c| centre(c, dom, target))
        }
        centre(&layout, &document.dom, &link).expect("link has a box")
    };

    let outcome = browser.click_at(x, y, VIEWPORT.0 as f32);
    assert!(matches!(outcome, ClickOutcome::Navigated(_)), "{outcome:?}");
    assert_eq!(
        browser.document().title().as_deref(),
        Some("About — Browser Engine Toy")
    );
}

#[test]
fn fragment_links_do_not_reload_the_document() {
    let mut browser = open_site();
    let links = dom_api::query_selector_all(&browser.document().dom, &[], ".nav-link");
    let fragment_link = links[1].clone(); // href="#pipeline"

    assert!(matches!(
        browser.click_node(&fragment_link),
        ClickOutcome::Navigated(_)
    ));
    assert!(browser.url().to_string().ends_with("#pipeline"));
    assert_eq!(
        browser.document().title().as_deref(),
        Some("Browser Engine Toy")
    );
    assert_eq!(browser.history().len(), 2);
}

#[test]
fn reload_restarts_the_page() {
    let mut browser = open_site();
    let button = dom_api::get_element_by_id(&browser.document().dom, "counter").unwrap();
    browser.click_node(&button);
    assert_eq!(text_of(browser.document(), "#counter"), "Clicks: 1");

    browser.reload().expect("reloads");
    assert_eq!(text_of(browser.document(), "#counter"), "Clicks: 0");
}

// ── Loader behaviour ──────────────────────────────────────────────────────────

#[test]
fn the_site_can_also_be_served_over_http() {
    // A one-request server backed by the same fixture files.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();

    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buffer = [0u8; 2048];
            let read = std::io::Read::read(&mut stream, &mut buffer).unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..read]).to_string();
            let target = request
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .trim_start_matches('/')
                .to_string();

            let body = std::fs::read(site_path(if target.is_empty() {
                "index.html"
            } else {
                &target
            }))
            .unwrap_or_default();
            let mime = browser_engine::net::mime_from_path(&target);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            use std::io::Write;
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&body);
        }
    });

    let url = Url::parse(&format!("http://127.0.0.1:{port}/index.html")).unwrap();
    let resource = DefaultLoader::new().load(&url).expect("fetched over http");
    assert!(resource.text().contains("Browser Engine Toy"));
    assert_eq!(resource.effective_mime(), "text/html");

    // The stylesheet resolves relative to the served document.
    let css = url.join("css/site.css").unwrap();
    assert_eq!(
        css.to_string(),
        format!("http://127.0.0.1:{port}/css/site.css")
    );
    let sheet = DefaultLoader::new().load(&css).expect("fetched stylesheet");
    assert!(sheet.text().contains(".nav-link"));

    server.join().unwrap();
}

// ── Interactive form page ─────────────────────────────────────────────────────

fn open_form() -> Browser {
    Browser::open(Box::new(DefaultLoader::new()), &site_url("form.html")).expect("form page loads")
}

fn element_by_id<'a>(browser: &'a Browser, id: &str) -> &'a browser_engine::dom::ElementData {
    let path = dom_api::get_element_by_id(&browser.document().dom, id)
        .unwrap_or_else(|| panic!("no #{id}"));
    dom_api::node_at(&browser.document().dom, &path)
        .and_then(|n| n.as_element())
        .expect("element")
}

fn focused_id(browser: &Browser) -> Option<String> {
    let path = browser.document().focused_path()?;
    let element = dom_api::node_at(&browser.document().dom, &path)?.as_element()?;
    element.get_attr("id").map(str::to_string)
}

#[test]
fn the_form_page_loads_its_own_stylesheet_and_script() {
    let browser = open_form();
    assert_eq!(
        browser.document().title().as_deref(),
        Some("Form controls — Browser Engine Toy")
    );
    // form.js ran and wrote its first log line.
    assert!(
        text_of(browser.document(), "#status").contains("ready"),
        "form.js should have run"
    );
    let unexpected: Vec<_> = browser
        .document()
        .diagnostics
        .iter()
        .filter(|d| !d.url.contains("missing.png"))
        .collect();
    assert!(unexpected.is_empty(), "unexpected failures: {unexpected:?}");
}

#[test]
fn clicking_focuses_typing_edits_and_tab_moves_on() {
    let mut browser = open_form();
    let query = dom_api::get_element_by_id(&browser.document().dom, "q").unwrap();

    browser.click_node(&query);
    assert_eq!(focused_id(&browser).as_deref(), Some("q"));

    browser.type_text("hello");
    assert_eq!(element_by_id(&browser, "q").control_value(), "hello");
    browser.press_key(&KeyEvent::new(Key::Backspace));
    assert_eq!(element_by_id(&browser, "q").control_value(), "hell");

    browser.press_key(&KeyEvent::new(Key::Tab));
    assert_eq!(focused_id(&browser).as_deref(), Some("notes"));
    browser.press_key(&KeyEvent::with_modifiers(Key::Tab, Modifiers::shift()));
    assert_eq!(focused_id(&browser).as_deref(), Some("q"));
}

#[test]
fn the_page_script_sees_focus_input_and_change_events() {
    let mut browser = open_form();
    let query = dom_api::get_element_by_id(&browser.document().dom, "q").unwrap();
    browser.click_node(&query);
    browser.type_text("ab");

    let status = text_of(browser.document(), "#status");
    assert!(status.contains("input: ab"), "status was {status:?}");

    let beta = dom_api::get_element_by_id(&browser.document().dom, "beta").unwrap();
    browser.click_node(&beta);
    let status = text_of(browser.document(), "#status");
    assert!(status.contains("change"), "status was {status:?}");
    assert!(element_by_id(&browser, "beta").is_checked());
}

#[test]
fn readonly_keeps_its_value_and_disabled_cannot_be_focused() {
    let mut browser = open_form();
    let locked = dom_api::get_element_by_id(&browser.document().dom, "locked").unwrap();
    browser.click_node(&locked);
    browser.type_text("xyz");
    assert_eq!(
        element_by_id(&browser, "locked").control_value(),
        "cannot be edited"
    );

    let off = dom_api::get_element_by_id(&browser.document().dom, "off").unwrap();
    browser.click_node(&off);
    assert_ne!(focused_id(&browser).as_deref(), Some("off"));
}

#[test]
fn submitting_the_form_navigates_with_every_successful_control() {
    let mut browser = open_form();
    let query = dom_api::get_element_by_id(&browser.document().dom, "q").unwrap();
    browser.click_node(&query);
    browser.type_text("toy browser");

    let outcome = browser.press_key(&KeyEvent::new(Key::Enter));
    assert!(matches!(outcome, ClickOutcome::Navigated(_)), "{outcome:?}");

    let url = browser.url().to_string();
    let query_string = browser.url().query().unwrap_or("").to_string();
    assert!(url.contains("results.html"), "went to {url}");
    // Text, textarea, checked box and selected radio are all present.
    assert!(query_string.contains("q=toy+browser"), "{query_string}");
    assert!(query_string.contains("news=on"), "{query_string}");
    assert!(query_string.contains("size=10"), "{query_string}");
    // The unchecked box and the disabled field are not.
    assert!(!query_string.contains("beta="), "{query_string}");
    assert!(!query_string.contains("off="), "{query_string}");

    assert_eq!(
        browser.document().title().as_deref(),
        Some("Results — Browser Engine Toy")
    );
    assert!(browser.can_go_back());
}

#[test]
fn a_cancelled_button_does_not_submit_the_form() {
    let mut browser = open_form();
    let quiet = dom_api::get_element_by_id(&browser.document().dom, "quiet").unwrap();
    browser.click_node(&quiet);

    assert!(browser.url().to_string().ends_with("form.html"));
    assert_eq!(browser.history().len(), 1);
    assert!(text_of(browser.document(), "#status").contains("cancelled"));
}

#[test]
fn reset_restores_the_controls_to_their_defaults() {
    let mut browser = open_form();
    let query = dom_api::get_element_by_id(&browser.document().dom, "q").unwrap();
    browser.click_node(&query);
    browser.type_text("typed");
    let news = dom_api::get_element_by_id(&browser.document().dom, "news").unwrap();
    browser.click_node(&news);
    assert!(!element_by_id(&browser, "news").is_checked());

    let clear = dom_api::get_element_by_id(&browser.document().dom, "clear").unwrap();
    browser.click_node(&clear);
    assert_eq!(element_by_id(&browser, "q").control_value(), "");
    assert!(element_by_id(&browser, "news").is_checked());
}

#[test]
fn keyboard_input_repaints_the_page() {
    let mut browser = open_form();
    let before = browser
        .render(VIEWPORT.0, 500, 0.0, &PointerState::default())
        .to_ppm();

    let query = dom_api::get_element_by_id(&browser.document().dom, "q").unwrap();
    browser.click_node(&query);
    let focused = browser
        .render(VIEWPORT.0, 500, 0.0, &PointerState::default())
        .to_ppm();
    assert_ne!(before, focused, "focus ring and caret should appear");

    browser.type_text("visible text");
    let typed = browser
        .render(VIEWPORT.0, 500, 0.0, &PointerState::default())
        .to_ppm();
    assert_ne!(focused, typed, "typed characters should be painted");
}

#[test]
fn the_textarea_starts_from_its_content_and_accepts_newlines() {
    let mut browser = open_form();
    let notes = dom_api::get_element_by_id(&browser.document().dom, "notes").unwrap();
    let initial = element_by_id(&browser, "notes").control_value();
    assert!(initial.contains('\n'), "seeded from content: {initial:?}");

    browser.click_node(&notes);
    // `End` is line-wise in a textarea, so this splits the caret's own line.
    browser.press_key(&KeyEvent::new(Key::End));
    browser.press_key(&KeyEvent::new(Key::Enter));
    browser.type_text("third line");

    let value = element_by_id(&browser, "notes").control_value();
    assert!(
        value.lines().any(|line| line == "third line"),
        "the typed text should stand on its own line: {value:?}"
    );
    assert_eq!(
        value.matches('\n').count(),
        initial.matches('\n').count() + 1
    );
}

// ── Timers and animation on the fixture page ──────────────────────────────────

/// Open the async page on virtual time.
fn open_async() -> Browser {
    Browser::open_with_clock(
        Box::new(DefaultLoader::new()),
        &site_url("async.html"),
        Rc::new(ManualClock::new()),
    )
    .expect("async page loads")
}

/// Advance in frame-sized steps, the way the window drives the loop.
fn run_for(browser: &mut Browser, total: Duration) {
    browser.advance_time_in_steps(total, Duration::from_millis(16));
}

#[test]
fn the_async_page_loads_its_stylesheet_and_script() {
    let browser = open_async();
    assert_eq!(
        browser.document().title().as_deref(),
        Some("Timers and animation — Browser Engine Toy")
    );
    assert!(
        browser.has_pending_tasks(),
        "the page scheduled work at load"
    );

    let unexpected: Vec<_> = browser
        .document()
        .diagnostics
        .iter()
        .filter(|d| !d.url.contains("missing.png"))
        .collect();
    assert!(unexpected.is_empty(), "unexpected failures: {unexpected:?}");
}

#[test]
fn a_timeout_updates_the_page_after_its_delay() {
    let mut browser = open_async();
    assert_eq!(text_of(browser.document(), "#timeout-status"), "waiting…");

    run_for(&mut browser, Duration::from_millis(900));
    assert_eq!(
        text_of(browser.document(), "#timeout-status"),
        "waiting…",
        "still waiting before the deadline"
    );

    run_for(&mut browser, Duration::from_millis(200));
    assert!(
        text_of(browser.document(), "#timeout-status").starts_with("done after"),
        "got {:?}",
        text_of(browser.document(), "#timeout-status")
    );
}

#[test]
fn an_interval_counts_up_and_then_stops_itself() {
    let mut browser = open_async();
    run_for(&mut browser, Duration::from_millis(2_100));
    let after_two = text_of(browser.document(), "#tick-count");
    assert!(
        after_two == "2" || after_two == "3",
        "expected a couple of ticks, got {after_two:?}"
    );

    run_for(&mut browser, Duration::from_millis(6_000));
    assert_eq!(text_of(browser.document(), "#tick-count"), "5");
    assert_eq!(
        text_of(browser.document(), "#interval-status"),
        "stopped at 5"
    );

    // It really stopped: more time changes nothing.
    run_for(&mut browser, Duration::from_millis(3_000));
    assert_eq!(text_of(browser.document(), "#tick-count"), "5");
}

#[test]
fn a_timer_adds_dom_that_reaches_the_paint() {
    let mut browser = open_async();
    assert_eq!(
        dom_api::query_selector_all(&browser.document().dom, &[], "#generated li").len(),
        0
    );
    let before = browser
        .render(VIEWPORT.0, 900, 0.0, &PointerState::default())
        .to_ppm();

    run_for(&mut browser, Duration::from_millis(600));
    assert_eq!(
        dom_api::query_selector_all(&browser.document().dom, &[], "#generated li").len(),
        3
    );

    let after = browser
        .render(VIEWPORT.0, 900, 0.0, &PointerState::default())
        .to_ppm();
    assert_ne!(before, after, "rows added by a timer must be painted");
}

#[test]
fn animation_frames_move_the_box_across_the_track() {
    let mut browser = open_async();

    let position = |browser: &Browser| -> f32 {
        let path = dom_api::get_element_by_id(&browser.document().dom, "box").expect("#box");
        let element = dom_api::node_at(&browser.document().dom, &path)
            .and_then(|n| n.as_element())
            .expect("element");
        // The frame callback writes the offset into the inline style.
        dom_api::get_style_property(element, "margin-left")
            .and_then(|value| value.trim_end_matches("px").parse::<f32>().ok())
            .unwrap_or(0.0)
    };

    let start = position(&browser);
    run_for(&mut browser, Duration::from_millis(160));
    let moved = position(&browser);
    assert!(
        moved > start,
        "the box should have advanced: {start} → {moved}"
    );

    let first = browser
        .render(VIEWPORT.0, 900, 0.0, &PointerState::default())
        .to_ppm();
    run_for(&mut browser, Duration::from_millis(160));
    let second = browser
        .render(VIEWPORT.0, 900, 0.0, &PointerState::default())
        .to_ppm();
    assert_ne!(first, second, "successive frames must differ on screen");
}

#[test]
fn buttons_can_start_and_cancel_scheduled_work() {
    let mut browser = open_async();
    assert_eq!(text_of(browser.document(), "#job-status"), "idle");

    let start = dom_api::get_element_by_id(&browser.document().dom, "start").unwrap();
    browser.click_node(&start);
    assert_eq!(text_of(browser.document(), "#job-status"), "running");

    run_for(&mut browser, Duration::from_millis(600));
    let entries = dom_api::query_selector_all(&browser.document().dom, &[], "#log li").len();
    assert!(
        entries >= 2,
        "the interval should have logged ticks: {entries}"
    );

    let cancel = dom_api::get_element_by_id(&browser.document().dom, "cancel").unwrap();
    browser.click_node(&cancel);
    assert_eq!(text_of(browser.document(), "#job-status"), "cancelled");

    let after_cancel = dom_api::query_selector_all(&browser.document().dom, &[], "#log li").len();
    run_for(&mut browser, Duration::from_millis(3_000));
    assert_eq!(
        dom_api::query_selector_all(&browser.document().dom, &[], "#log li").len(),
        after_cancel,
        "cancelling stopped both the timeout and the interval"
    );
    assert_ne!(
        text_of(browser.document(), "#job-status"),
        "finished",
        "the cancelled timeout must not fire"
    );
}

#[test]
fn navigating_away_from_the_async_page_stops_its_work() {
    let mut browser = open_async();
    run_for(&mut browser, Duration::from_millis(500));
    assert!(browser.has_pending_tasks());

    let home = dom_api::query_selector(&browser.document().dom, &[], ".nav-link").unwrap();
    browser.click_node(&home);
    assert_eq!(
        browser.document().title().as_deref(),
        Some("Browser Engine Toy")
    );

    let report =
        browser.advance_time_in_steps(Duration::from_millis(3_000), Duration::from_millis(100));
    assert_eq!(report.timers_run, 0, "the old page's timers are gone");
    assert_eq!(report.frames_run, 0, "and so are its frame callbacks");
}

#[test]
fn reloading_the_async_page_restarts_its_timers() {
    let mut browser = open_async();
    run_for(&mut browser, Duration::from_millis(2_100));
    assert_ne!(text_of(browser.document(), "#tick-count"), "0");

    browser.reload().expect("reloads");
    assert_eq!(text_of(browser.document(), "#tick-count"), "0");
    assert_eq!(text_of(browser.document(), "#timeout-status"), "waiting…");

    run_for(&mut browser, Duration::from_millis(1_100));
    assert_eq!(text_of(browser.document(), "#tick-count"), "1");
}

// ── Promises on the fixture page ──────────────────────────────────────────────

fn open_promises() -> Browser {
    Browser::open_with_clock(
        Box::new(DefaultLoader::new()),
        &site_url("promise.html"),
        Rc::new(ManualClock::new()),
    )
    .expect("promise page loads")
}

fn order_log(browser: &Browser) -> Vec<String> {
    dom_api::query_selector_all(&browser.document().dom, &[], "#order-log li")
        .iter()
        .filter_map(|path| dom_api::node_at(&browser.document().dom, path))
        .map(dom_api::text_content)
        .collect()
}

#[test]
fn microtasks_finish_before_any_time_passes() {
    let browser = open_promises();
    let entries = order_log(&browser);

    // Synchronous work, then the microtasks, all before the clock moves.
    assert_eq!(
        entries,
        vec![
            "1. sync start",
            "2. executor (synchronous)",
            "3. sync end",
            "4. promise then: ready",
            "5. queueMicrotask",
        ],
        "load-time checkpoint should have drained the microtask queue"
    );
}

#[test]
fn a_timer_task_runs_after_every_microtask() {
    let mut browser = open_promises();
    browser.advance_time(Duration::from_millis(16));

    let entries = order_log(&browser);
    assert_eq!(entries[5], "6. timer (task)");
    assert_eq!(
        entries[6], "7. microtask from the timer",
        "a microtask queued by the timer runs before the next task"
    );
}

#[test]
fn a_chain_resolves_without_the_clock_moving() {
    let browser = open_promises();
    assert_eq!(text_of(browser.document(), "#chain"), "1 + 1 + 1 = 3");
}

#[test]
fn a_thrown_value_is_recovered_and_finally_still_runs() {
    let browser = open_promises();
    assert_eq!(
        text_of(browser.document(), "#rejection"),
        "recovered from: something broke"
    );
    assert_eq!(text_of(browser.document(), "#cleanup"), "finally ran");
}

#[test]
fn promise_all_waits_for_its_timer_backed_entry() {
    let mut browser = open_promises();
    assert_eq!(
        text_of(browser.document(), "#all"),
        "waiting for every promise…"
    );

    browser.advance_time_in_steps(Duration::from_millis(500), Duration::from_millis(16));
    assert_eq!(
        text_of(browser.document(), "#all"),
        "waiting for every promise…",
        "still waiting at 500ms"
    );

    browser.advance_time_in_steps(Duration::from_millis(200), Duration::from_millis(16));
    assert_eq!(
        text_of(browser.document(), "#all"),
        "first | a plain value | from a timer",
        "input order is preserved"
    );
}

#[test]
fn a_promise_chain_builds_dom_after_waiting_on_a_timer() {
    let mut browser = open_promises();
    assert_eq!(
        dom_api::query_selector_all(&browser.document().dom, &[], "#rows li").len(),
        0
    );
    let before = browser
        .render(VIEWPORT.0, 1100, 0.0, &PointerState::default())
        .to_ppm();

    browser.advance_time_in_steps(Duration::from_millis(400), Duration::from_millis(16));
    assert_eq!(
        dom_api::query_selector_all(&browser.document().dom, &[], "#rows li").len(),
        3
    );

    let after = browser
        .render(VIEWPORT.0, 1100, 0.0, &PointerState::default())
        .to_ppm();
    assert_ne!(before, after, "promise-built rows must be painted");
}

#[test]
fn navigating_away_drops_pending_promise_work() {
    let mut browser = open_promises();
    assert!(
        browser.has_pending_tasks(),
        "timers back the pending promises"
    );

    let home = dom_api::query_selector(&browser.document().dom, &[], ".nav-link").unwrap();
    browser.click_node(&home);
    assert_eq!(
        browser.document().title().as_deref(),
        Some("Browser Engine Toy")
    );

    let report =
        browser.advance_time_in_steps(Duration::from_millis(2_000), Duration::from_millis(100));
    assert_eq!(
        report.timers_run, 0,
        "the old page cannot resolve anything now"
    );
}

#[test]
fn reloading_restarts_the_promise_work() {
    let mut browser = open_promises();
    browser.advance_time_in_steps(Duration::from_millis(700), Duration::from_millis(16));
    assert_eq!(
        text_of(browser.document(), "#all"),
        "first | a plain value | from a timer"
    );

    browser.reload().expect("reloads");
    assert_eq!(
        text_of(browser.document(), "#all"),
        "waiting for every promise…",
        "a fresh runtime starts the promises over"
    );
    // …and the load-time checkpoint has already run on the new document.
    assert_eq!(text_of(browser.document(), "#chain"), "1 + 1 + 1 = 3");
}

// ── Fetch on the fixture page ─────────────────────────────────────────────────

/// Open `fetch.html` on a network nothing completes without being told to.
fn open_fetch_page() -> (Browser, Rc<ManualNetwork>) {
    let network = Rc::new(ManualNetwork::new());
    let browser = Browser::open_with_network(
        Box::new(DefaultLoader::new()),
        network.clone(),
        &site_url("fetch.html"),
        Rc::new(ManualClock::new()),
    )
    .expect("fetch page loads");
    (browser, network)
}

/// Register the fixture's own files as the answers for its `fetch()` calls.
fn serve_fixture_api(network: &ManualNetwork) {
    for (relative, mime) in [
        ("api/data.json", "application/json"),
        ("api/note.txt", "text/plain; charset=utf-8"),
        ("api/echo.json", "application/json"),
    ] {
        let bytes = std::fs::read(site_path(relative)).expect("fixture file");
        network.respond_with(&site_url(relative).to_string(), 200, mime, bytes);
    }
}

fn click(browser: &mut Browser, id: &str) {
    let path = dom_api::get_element_by_id(&browser.document().dom, id)
        .unwrap_or_else(|| panic!("no element with id {id:?}"));
    browser.click_node(&path);
}

/// One turn to send, then complete by hand, then one turn to collect.
fn exchange(browser: &mut Browser, network: &ManualNetwork) -> usize {
    browser.advance_time(Duration::from_millis(16));
    let completed = network.complete_all();
    browser.advance_time(Duration::from_millis(16));
    completed
}

#[test]
fn the_page_starts_idle_with_nothing_requested() {
    let (browser, network) = open_fetch_page();
    assert_eq!(text_of(browser.document(), "#status"), "idle");
    assert_eq!(browser.document().in_flight_requests(), 0);
    assert_eq!(network.requests().len(), 0, "nothing is fetched at load");
}

#[test]
fn clicking_load_starts_a_request_that_has_not_completed() {
    let (mut browser, network) = open_fetch_page();
    serve_fixture_api(&network);
    click(&mut browser, "load");

    // The handler ran and the promise is pending; the request has not even
    // reached the network, let alone come back.
    assert_eq!(text_of(browser.document(), "#status"), "loading…");
    assert_eq!(browser.document().in_flight_requests(), 1);
    assert_eq!(network.pending_count(), 0);

    browser.advance_time(Duration::from_millis(16));
    assert_eq!(network.pending_count(), 1, "now it is on the network");
    assert_eq!(
        text_of(browser.document(), "#status"),
        "loading…",
        "and still pending"
    );
}

#[test]
fn a_completed_request_parses_json_and_builds_the_dom() {
    let (mut browser, network) = open_fetch_page();
    serve_fixture_api(&network);

    let before = browser
        .render(VIEWPORT.0, VIEWPORT.1, 0.0, &PointerState::default())
        .to_ppm();

    click(&mut browser, "load");
    assert_eq!(exchange(&mut browser, &network), 1);

    assert_eq!(
        text_of(browser.document(), "#status"),
        "Loaded over the network · Toy Browser Engine"
    );
    assert_eq!(
        dom_api::query_selector_all(&browser.document().dom, &[], "#cards li").len(),
        3
    );

    let after = browser
        .render(VIEWPORT.0, VIEWPORT.1, 0.0, &PointerState::default())
        .to_ppm();
    assert_ne!(before, after, "the fetched cards must be painted");
}

#[test]
fn a_promise_handler_changes_the_class_and_the_paint_follows() {
    let (mut browser, network) = open_fetch_page();
    serve_fixture_api(&network);

    click(&mut browser, "load");
    let loading = browser
        .render(VIEWPORT.0, VIEWPORT.1, 0.0, &PointerState::default())
        .to_ppm();
    exchange(&mut browser, &network);
    let done = browser
        .render(VIEWPORT.0, VIEWPORT.1, 0.0, &PointerState::default())
        .to_ppm();

    // `.readout.loading` and `.readout.done` are different colours, so the
    // class the handler set has to reach the pixels.
    assert_ne!(loading, done);
}

#[test]
fn response_text_reads_a_plain_text_resource() {
    let (mut browser, network) = open_fetch_page();
    serve_fixture_api(&network);
    click(&mut browser, "load-text");
    exchange(&mut browser, &network);

    assert_eq!(
        text_of(browser.document(), "#note"),
        "A plain-text resource, decoded as UTF-8 by response.text()."
    );
}

#[test]
fn a_missing_resource_resolves_with_a_404() {
    let (mut browser, network) = open_fetch_page();
    // Deliberately not registered: the manual network answers with a 404.
    click(&mut browser, "load-missing");
    exchange(&mut browser, &network);

    assert_eq!(
        text_of(browser.document(), "#missing"),
        "resolved with 404 Not Found (ok = false)"
    );
}

#[test]
fn a_post_sends_its_method_headers_and_body() {
    let (mut browser, network) = open_fetch_page();
    serve_fixture_api(&network);
    click(&mut browser, "post");
    exchange(&mut browser, &network);

    let sent = network.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].method, Method::Post);
    assert_eq!(
        sent[0].headers.get("content-type").as_deref(),
        Some("application/json")
    );
    assert_eq!(
        String::from_utf8_lossy(sent[0].body.as_deref().unwrap()),
        r#"{"name":"toy browser","version":1}"#
    );
    assert!(
        text_of(browser.document(), "#posted").starts_with("server said:"),
        "{}",
        text_of(browser.document(), "#posted")
    );
}

#[test]
fn promise_all_over_three_fetches_keeps_the_input_order() {
    let (mut browser, network) = open_fetch_page();
    serve_fixture_api(&network);
    click(&mut browser, "load-all");
    browser.advance_time(Duration::from_millis(16));

    // Complete them backwards; the result must still be in request order.
    let ids: Vec<u64> = network.pending().into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids.len(), 3);
    for id in ids.iter().rev() {
        network.complete(*id);
    }
    browser.advance_time(Duration::from_millis(16));

    assert_eq!(
        text_of(browser.document(), "#all"),
        "data.json=200  ·  note.txt=200  ·  echo.json=200"
    );
}

#[test]
fn a_cross_origin_request_is_caught_by_the_page() {
    let (mut browser, network) = open_fetch_page();
    click(&mut browser, "fail");

    // Refused before it is recorded: the network never sees it, and the
    // rejection still arrives asynchronously.
    assert_eq!(network.requests().len(), 0);
    assert!(
        text_of(browser.document(), "#failure").contains("same-origin policy"),
        "{}",
        text_of(browser.document(), "#failure")
    );
}

#[test]
fn navigating_away_stops_the_old_page_from_ever_settling() {
    let (mut browser, network) = open_fetch_page();
    serve_fixture_api(&network);
    click(&mut browser, "load");
    browser.advance_time(Duration::from_millis(16));
    assert_eq!(network.pending_count(), 1);

    // Leave while the answer is still owed.
    let home = dom_api::query_selector(&browser.document().dom, &[], ".nav-link").unwrap();
    browser.click_node(&home);
    assert_eq!(
        browser.document().title().as_deref(),
        Some("Browser Engine Toy")
    );
    assert_eq!(browser.document().in_flight_requests(), 0);

    // The answer arrives for a page that no longer exists.
    network.complete_all();
    let report = browser.advance_time(Duration::from_millis(16));
    assert_eq!(
        report.network_completions, 0,
        "a completion for the previous page settles nothing"
    );
    assert!(dom_api::query_selector(&browser.document().dom, &[], "#cards").is_none());
}

#[test]
fn reloading_starts_the_page_over_with_no_requests() {
    let (mut browser, network) = open_fetch_page();
    serve_fixture_api(&network);
    click(&mut browser, "load");
    exchange(&mut browser, &network);
    assert_eq!(
        dom_api::query_selector_all(&browser.document().dom, &[], "#cards li").len(),
        3
    );

    browser.reload().expect("reloads");
    assert_eq!(text_of(browser.document(), "#status"), "idle");
    assert_eq!(
        dom_api::query_selector_all(&browser.document().dom, &[], "#cards li").len(),
        0
    );
    assert_eq!(browser.document().in_flight_requests(), 0);
}

#[test]
fn the_page_also_works_against_the_plain_file_backend() {
    // No manual network: `DefaultNetwork` reads `api/data.json` off the disk,
    // which is what happens when the page is opened in the window.
    let mut browser = Browser::open_with_clock(
        Box::new(DefaultLoader::new()),
        &site_url("fetch.html"),
        Rc::new(ManualClock::new()),
    )
    .expect("fetch page loads");

    click(&mut browser, "load");
    let report = browser.settle_network(20);
    assert_eq!(report.requests_sent, 1);
    assert_eq!(report.network_completions, 1);

    assert_eq!(
        text_of(browser.document(), "#status"),
        "Loaded over the network · Toy Browser Engine"
    );
    assert_eq!(
        dom_api::query_selector_all(&browser.document().dom, &[], "#cards li").len(),
        3
    );
}

#[test]
fn a_local_page_may_not_fetch_outside_its_directory() {
    let mut browser = Browser::open_with_clock(
        Box::new(DefaultLoader::new()),
        &site_url("fetch.html"),
        Rc::new(ManualClock::new()),
    )
    .expect("fetch page loads");
    browser.document_mut().runtime.quiet = true;

    // The fixture lives in examples/site; its parent must be out of reach.
    let document = browser.document_mut();
    document.runtime.run_script(
        &mut document.dom,
        r#"fetch("../../Cargo.toml").catch(function (e) { console.log("blocked: " + e); });"#,
    );
    document.run_microtask_checkpoint();

    let logs = browser.document().runtime.console.join("\n");
    assert!(logs.contains("blocked: TypeError"), "{logs}");
    assert_eq!(browser.document().in_flight_requests(), 0);
}

#[test]
fn a_static_source_answers_a_post_with_405_rather_than_failing() {
    // The file backend can only read. A real server would echo the body; this
    // one says so with a status, which is still a resolved promise.
    let mut browser = Browser::open_with_clock(
        Box::new(DefaultLoader::new()),
        &site_url("fetch.html"),
        Rc::new(ManualClock::new()),
    )
    .expect("fetch page loads");

    click(&mut browser, "post");
    browser.settle_network(20);

    assert_eq!(
        text_of(browser.document(), "#posted"),
        "backend answered 405 Method Not Allowed"
    );
}

#[test]
fn every_button_works_against_the_plain_file_backend() {
    let mut browser = Browser::open_with_clock(
        Box::new(DefaultLoader::new()),
        &site_url("fetch.html"),
        Rc::new(ManualClock::new()),
    )
    .expect("fetch page loads");

    for id in ["load", "load-text", "load-missing", "load-all", "fail"] {
        click(&mut browser, id);
        browser.settle_network(20);
    }

    assert_eq!(
        text_of(browser.document(), "#status"),
        "Loaded over the network · Toy Browser Engine"
    );
    assert!(text_of(browser.document(), "#note").starts_with("A plain-text resource"));
    assert_eq!(
        text_of(browser.document(), "#missing"),
        "resolved with 404 Not Found (ok = false)"
    );
    assert_eq!(
        text_of(browser.document(), "#all"),
        "data.json=200  ·  note.txt=200  ·  echo.json=200"
    );
    assert!(text_of(browser.document(), "#failure").contains("same-origin policy"));
    assert!(!browser.document().has_pending_network());
}
