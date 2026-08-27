use std::rc::Rc;
use std::time::Duration;

use browser_engine::animation::sample_keyframes;
use browser_engine::css::parser::{
    parse_css, AnimationDirection, AnimationFillMode, AnimationIterationCount, TimingFunction,
    Unit, Value,
};
use browser_engine::document::{Document, PointerState};
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{MemoryLoader, Url};
use browser_engine::Browser;

fn find_styled_by_id<'a>(
    node: &'a browser_engine::style::StyledNode<'a>,
    id: &str,
) -> Option<&'a browser_engine::style::StyledNode<'a>> {
    if let Some(element) = node.node.as_element() {
        if element.get_attr("id") == Some(id) {
            return Some(node);
        }
    }
    for child in &node.children {
        if let Some(found) = find_styled_by_id(child, id) {
            return Some(found);
        }
    }
    None
}

#[test]
fn test_keyframes_parsing() {
    let css = parse_css(
        r#"
        @keyframes fade-and-move {
            0% { opacity: 0; transform: translate(0px, 0px); }
            50% { opacity: 0.5; }
            100% { opacity: 1; transform: translate(100px, 50px); }
        }

        .box {
            animation: fade-and-move 2s ease-in-out 100ms 3 alternate forwards;
        }
        "#,
    );

    assert_eq!(css.keyframes.len(), 1);
    let kf = css.keyframes.get("fade-and-move").expect("found fade-and-move keyframe");
    assert_eq!(kf.name, "fade-and-move");
    assert_eq!(kf.steps.len(), 3);
    assert_eq!(kf.steps[0].offset, 0.0);
    assert_eq!(kf.steps[1].offset, 0.5);
    assert_eq!(kf.steps[2].offset, 1.0);

    let decls = &css.rules[0].declarations;
    if let Value::Animation(specs) = &decls[0].value {
        assert_eq!(specs.len(), 1);
        let spec = &specs[0];
        assert_eq!(spec.name, "fade-and-move");
        assert_eq!(spec.duration_ms, 2000.0);
        assert_eq!(spec.delay_ms, 100.0);
        assert_eq!(spec.timing_function, TimingFunction::EaseInOut);
        assert_eq!(spec.iteration_count, AnimationIterationCount::Finite(3.0));
        assert_eq!(spec.direction, AnimationDirection::Alternate);
        assert_eq!(spec.fill_mode, AnimationFillMode::Forwards);
    } else {
        panic!("expected Value::Animation");
    }
}

#[test]
fn test_sample_keyframes_interpolation() {
    let css = parse_css(
        r#"
        @keyframes grow {
            from { width: 0px; opacity: 0; }
            to { width: 200px; opacity: 1; }
        }
        "#,
    );

    let kf = css.keyframes.get("grow").unwrap();
    let sampled_start = sample_keyframes(kf, 0.0);
    assert_eq!(sampled_start.get("width"), Some(&Value::Length(0.0, Unit::Px)));
    assert_eq!(sampled_start.get("opacity"), Some(&Value::Number(0.0)));

    let sampled_mid = sample_keyframes(kf, 0.5);
    assert_eq!(sampled_mid.get("width"), Some(&Value::Length(100.0, Unit::Px)));
    assert_eq!(sampled_mid.get("opacity"), Some(&Value::Number(0.5)));

    let sampled_end = sample_keyframes(kf, 1.0);
    assert_eq!(sampled_end.get("width"), Some(&Value::Length(200.0, Unit::Px)));
    assert_eq!(sampled_end.get("opacity"), Some(&Value::Number(1.0)));
}

#[test]
fn test_document_animation_timeline_and_fill_mode() {
    let html = r#"
    <html>
        <head>
            <style>
                @keyframes slide {
                    0% { width: 0px; }
                    100% { width: 100px; }
                }
                #box {
                    width: 10px;
                    animation: slide 1s linear 0s 1 normal forwards;
                }
            </style>
        </head>
        <body>
            <div id="box"></div>
        </body>
    </html>
    "#;

    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/").unwrap();
    let mut doc = Document::from_html(html, &url, &loader);

    // At t = 0ms: animation starts at width 0px
    doc.runtime.now_ms = 0.0;
    let styled0 = doc.style_tree(800.0, &PointerState::default());
    let w0 = find_styled_by_id(&styled0, "box").unwrap().specified_values.get("width");
    assert_eq!(w0, Some(&Value::Length(0.0, Unit::Px)));
    assert!(doc.has_pending_tasks(), "Active animation should keep document awake");

    // At t = 500ms: halfway (50px)
    doc.runtime.now_ms = 500.0;
    let styled_mid = doc.style_tree(800.0, &PointerState::default());
    let w_mid = find_styled_by_id(&styled_mid, "box").unwrap().specified_values.get("width");
    assert_eq!(w_mid, Some(&Value::Length(50.0, Unit::Px)));

    // At t = 1000ms: finished at 100px
    doc.runtime.now_ms = 1000.0;
    let styled_end = doc.style_tree(800.0, &PointerState::default());
    let w_end = find_styled_by_id(&styled_end, "box").unwrap().specified_values.get("width");
    assert_eq!(w_end, Some(&Value::Length(100.0, Unit::Px)));

    // At t = 2000ms: forwards fill mode holds 100px
    doc.runtime.now_ms = 2000.0;
    let styled_after = doc.style_tree(800.0, &PointerState::default());
    let w_after = find_styled_by_id(&styled_after, "box").unwrap().specified_values.get("width");
    assert_eq!(w_after, Some(&Value::Length(100.0, Unit::Px)));
}

#[test]
fn test_animation_alternate_direction() {
    let html = r#"
    <html>
        <head>
            <style>
                @keyframes pulse {
                    0% { opacity: 0; }
                    100% { opacity: 1; }
                }
                #box {
                    animation: pulse 1s linear 0s 2 alternate forwards;
                }
            </style>
        </head>
        <body>
            <div id="box"></div>
        </body>
    </html>
    "#;

    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/").unwrap();
    let mut doc = Document::from_html(html, &url, &loader);

    // Initial frame at t = 0
    doc.runtime.now_ms = 0.0;
    let _ = doc.style_tree(800.0, &PointerState::default());

    // Cycle 1 (0 -> 1): at 500ms, opacity is 0.5
    doc.runtime.now_ms = 500.0;
    let styled1 = doc.style_tree(800.0, &PointerState::default());
    let op1 = find_styled_by_id(&styled1, "box").unwrap().specified_values.get("opacity");
    if let Some(Value::Number(n)) = op1 {
        assert!((n - 0.5).abs() < 1e-3);
    } else {
        panic!("expected Number");
    }

    // Cycle 2 (1 -> 0 alternate): at 1500ms (50% of cycle 2), opacity is 0.5
    doc.runtime.now_ms = 1500.0;
    let styled2 = doc.style_tree(800.0, &PointerState::default());
    let op2 = find_styled_by_id(&styled2, "box").unwrap().specified_values.get("opacity");
    if let Some(Value::Number(n)) = op2 {
        assert!((n - 0.5).abs() < 1e-3);
    } else {
        panic!("expected Number");
    }

    // At 2000ms: finished at 0.0 (end of cycle 2)
    doc.runtime.now_ms = 2000.0;
    let styled_end = doc.style_tree(800.0, &PointerState::default());
    let op_end = find_styled_by_id(&styled_end, "box").unwrap().specified_values.get("opacity");
    if let Some(Value::Number(n)) = op_end {
        assert!((n - 0.0).abs() < 1e-3);
    } else {
        panic!("expected Number");
    }
}

#[test]
fn test_browser_advance_time_animations() {
    let html = r#"
    <html>
        <head>
            <style>
                @keyframes spin {
                    0% { opacity: 0; }
                    100% { opacity: 1; }
                }
                #spinner {
                    opacity: 0;
                    animation: spin 1s linear infinite;
                }
            </style>
        </head>
        <body>
            <div id="spinner"></div>
        </body>
    </html>
    "#;

    let mut loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/").unwrap();
    loader.insert("http://example.com/", html.as_bytes().to_vec());

    let clock = Rc::new(ManualClock::new());
    let mut browser = Browser::open_with_clock(Box::new(loader), &url, clock.clone()).unwrap();

    // Start of animation
    let styled0 = browser.document().style_tree(800.0, &PointerState::default());
    let op0 = find_styled_by_id(&styled0, "spinner").unwrap().specified_values.get("opacity");
    assert_eq!(op0, Some(&Value::Number(0.0)));

    // Advance 500ms
    browser.advance_time(Duration::from_millis(500));
    let styled500 = browser.document().style_tree(800.0, &PointerState::default());
    let op500 = find_styled_by_id(&styled500, "spinner").unwrap().specified_values.get("opacity");
    if let Some(Value::Number(n)) = op500 {
        assert!((n - 0.5).abs() < 1e-3);
    } else {
        panic!("expected Number");
    }

    // Infinite animation should always request next frame
    assert!(browser.document().has_pending_tasks());
    assert!(browser.next_wakeup_in_ms().is_some());
}
