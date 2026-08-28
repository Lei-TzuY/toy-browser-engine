use browser_engine::browser_chrome::BrowserChromeTracker;
use browser_engine::dom::{ElementData, Node, NodeType};
use browser_engine::{Browser, MemoryLoader, Url};

fn png_rgba(rgba: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&rgba).unwrap();
    }
    out
}

fn browser() -> Browser {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "demo:///index.html",
        b"<head><title>Home</title><link rel='icon' href='home.png' sizes='32x32'></head><a href='about.html'>about</a>".to_vec(),
    );
    loader.insert(
        "demo:///about.html",
        b"<head><title>About</title><link rel='icon' href='about.png' sizes='32x32'></head>".to_vec(),
    );
    loader.insert("demo:///home.png", png_rgba([10, 20, 30, 255]));
    loader.insert("demo:///about.png", png_rgba([40, 50, 60, 255]));
    loader.insert("demo:///dynamic.png", png_rgba([70, 80, 90, 200]));
    Browser::open(
        Box::new(loader),
        &Url::parse("demo:///index.html").unwrap(),
    )
    .unwrap()
}

fn find_element_mut<'a>(node: &'a mut Node, tag: &str) -> Option<&'a mut ElementData> {
    if let NodeType::Element(element) = &mut node.node_type {
        if element.tag_name == tag {
            return Some(element);
        }
    }
    for child in &mut node.children {
        if let Some(found) = find_element_mut(child, tag) {
            return Some(found);
        }
    }
    None
}

fn set_title_text(node: &mut Node, value: &str) -> bool {
    if let NodeType::Element(element) = &node.node_type {
        if element.tag_name == "title" {
            if let Some(child) = node.children.first_mut() {
                child.node_type = NodeType::Text(value.to_string());
            } else {
                node.children.push(Node::text(value));
            }
            return true;
        }
    }
    node.children.iter_mut().any(|child| set_title_text(child, value))
}

#[test]
fn first_poll_reports_complete_chrome_state_then_stabilizes() {
    let browser = browser();
    let mut tracker = BrowserChromeTracker::with_icon_size(32, 32);

    let first = tracker.poll(&browser);
    assert!(first.changes.title);
    assert!(first.changes.url);
    assert!(first.changes.history);
    assert!(first.changes.icon);
    assert_eq!(first.state.title.as_deref(), Some("Home"));
    assert_eq!(first.state.url, "demo:///index.html");
    assert_eq!(first.state.history_index, 0);
    assert_eq!(first.state.history_len, 1);
    assert_eq!(first.state.status_line(), browser.status_line());
    let icon = first.icon.expect("home icon");
    assert_eq!(icon.image.pixel(0, 0), [10, 20, 30, 255]);

    let second = tracker.poll(&browser);
    assert!(!second.changes.any());
    assert_eq!(tracker.icon_resolver().cache().len(), 1);
}

#[test]
fn live_dom_title_and_icon_mutations_are_detected_without_navigation() {
    let mut browser = browser();
    let mut tracker = BrowserChromeTracker::new();
    let initial = tracker.poll(&browser);
    let initial_icon_hash = initial.state.icon.unwrap().pixel_hash;

    assert!(set_title_text(&mut browser.document_mut().dom, "Changed"));
    let link = find_element_mut(&mut browser.document_mut().dom, "link").unwrap();
    link.set_attr("href", "dynamic.png");

    let update = tracker.poll(&browser);
    assert!(update.changes.title);
    assert!(update.changes.icon);
    assert!(!update.changes.url);
    assert!(!update.changes.history);
    assert_eq!(update.state.title.as_deref(), Some("Changed"));
    assert_ne!(update.state.icon.as_ref().unwrap().pixel_hash, initial_icon_hash);
    assert_eq!(update.icon.unwrap().image.pixel(0, 0), [70, 80, 90, 200]);
    assert_eq!(tracker.icon_resolver().cache().len(), 2);
}

#[test]
fn navigation_reports_url_history_title_and_icon_changes() {
    let mut browser = browser();
    let mut tracker = BrowserChromeTracker::new();
    let home = tracker.poll(&browser);
    let home_icon = home.state.icon.unwrap();

    browser.follow_link("about.html").unwrap();
    let about = tracker.poll(&browser);
    assert!(about.changes.title);
    assert!(about.changes.url);
    assert!(about.changes.history);
    assert!(about.changes.icon);
    assert_eq!(about.state.title.as_deref(), Some("About"));
    assert_eq!(about.state.url, "demo:///about.html");
    assert_eq!(about.state.history_index, 1);
    assert_eq!(about.state.history_len, 2);
    assert_ne!(about.state.icon.as_ref().unwrap(), &home_icon);
    assert_eq!(about.icon.unwrap().image.pixel(0, 0), [40, 50, 60, 255]);
}

#[test]
fn reset_forces_a_full_frontend_refresh_and_clears_icon_cache() {
    let browser = browser();
    let mut tracker = BrowserChromeTracker::new();
    let _ = tracker.poll(&browser);
    assert_eq!(tracker.icon_resolver().cache().len(), 1);

    tracker.reset();
    assert!(tracker.previous().is_none());
    assert!(tracker.icon_resolver().cache().is_empty());

    let update = tracker.poll(&browser);
    assert!(update.changes.title);
    assert!(update.changes.url);
    assert!(update.changes.history);
    assert!(update.changes.icon);
}
