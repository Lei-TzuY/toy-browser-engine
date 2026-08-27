use std::rc::Rc;
use std::time::Duration;

use browser_engine::css::parser::{parse_css, Color, TimingFunction, Unit, Value};
use browser_engine::document::{Document, PointerState};
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{MemoryLoader, Url};
use browser_engine::transition::interpolate;
use browser_engine::Browser;

#[test]
fn test_transition_shorthand_parsing() {
    let css = parse_css(
        r#"
        .box {
            transition: opacity 0.5s ease, transform 1s 0.2s cubic-bezier(0.25, 0.1, 0.25, 1.0);
            transition-duration: 300ms;
        }
        "#,
    );

    let decls = &css.rules[0].declarations;
    assert_eq!(decls[0].name, "transition");
    if let Value::Transition(specs) = &decls[0].value {
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].property, "opacity");
        assert_eq!(specs[0].duration_ms, 500.0);
        assert_eq!(specs[0].timing_function, TimingFunction::Ease);
        assert_eq!(specs[0].delay_ms, 0.0);

        assert_eq!(specs[1].property, "transform");
        assert_eq!(specs[1].duration_ms, 1000.0);
        assert_eq!(specs[1].delay_ms, 200.0);
        assert_eq!(
            specs[1].timing_function,
            TimingFunction::CubicBezier(0.25, 0.1, 0.25, 1.0)
        );
    } else {
        panic!("expected Value::Transition");
    }

    assert_eq!(decls[1].name, "transition-duration");
    assert_eq!(decls[1].value, Value::Number(300.0));
}

#[test]
fn test_timing_functions_evaluation() {
    let linear = TimingFunction::Linear;
    assert!((linear.evaluate(0.0) - 0.0).abs() < 1e-4);
    assert!((linear.evaluate(0.5) - 0.5).abs() < 1e-4);
    assert!((linear.evaluate(1.0) - 1.0).abs() < 1e-4);

    let ease = TimingFunction::Ease;
    assert!((ease.evaluate(0.0) - 0.0).abs() < 1e-4);
    assert!((ease.evaluate(1.0) - 1.0).abs() < 1e-4);
    // Ease accelerates quickly early on then decelerates
    assert!(ease.evaluate(0.5) > 0.5);

    let cb = TimingFunction::CubicBezier(0.0, 0.0, 1.0, 1.0);
    assert!((cb.evaluate(0.5) - 0.5).abs() < 1e-3);
}

#[test]
fn test_value_interpolation() {
    // Colors
    let red = Value::Color(Color::rgb(255, 0, 0));
    let blue = Value::Color(Color::rgb(0, 0, 255));
    let mid_color = interpolate(&red, &blue, 0.5);
    if let Value::Color(c) = mid_color {
        assert_eq!(c.r, 128);
        assert_eq!(c.g, 0);
        assert_eq!(c.b, 128);
        assert_eq!(c.a, 255);
    } else {
        panic!("expected Value::Color");
    }

    // Lengths
    let len1 = Value::Length(100.0, Unit::Px);
    let len2 = Value::Length(200.0, Unit::Px);
    assert_eq!(interpolate(&len1, &len2, 0.25), Value::Length(125.0, Unit::Px));
    assert_eq!(interpolate(&len1, &len2, 0.75), Value::Length(175.0, Unit::Px));

    // Numbers (opacity)
    let num1 = Value::Number(0.0);
    let num2 = Value::Number(1.0);
    assert_eq!(interpolate(&num1, &num2, 0.4), Value::Number(0.4));
}

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

fn find_dom_by_id_mut<'a>(
    node: &'a mut browser_engine::dom::Node,
    id: &str,
) -> Option<&'a mut browser_engine::dom::ElementData> {
    if let browser_engine::dom::NodeType::Element(ref mut element) = node.node_type {
        if element.get_attr("id") == Some(id) {
            return Some(element);
        }
    }
    for child in &mut node.children {
        if let Some(found) = find_dom_by_id_mut(child, id) {
            return Some(found);
        }
    }
    None
}

#[test]
fn test_document_transition_over_time() {
    let html = r#"
    <html>
        <head>
            <style>
                #box {
                    opacity: 1;
                    transition: opacity 1s linear;
                }
                #box.fade {
                    opacity: 0;
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

    // Initial style: opacity 1
    let styled0 = doc.style_tree(800.0, &PointerState::default());
    let box0 = find_styled_by_id(&styled0, "box")
        .expect("found #box")
        .specified_values
        .get("opacity");
    assert_eq!(box0, Some(&Value::Number(1.0)));

    // Script or class change: add "fade" class
    find_dom_by_id_mut(&mut doc.dom, "box")
        .expect("found #box in dom")
        .set_attr("class", "fade");

    // At t = 0ms: transition starts, opacity is 1.0
    doc.runtime.now_ms = 0.0;
    let styled_start = doc.style_tree(800.0, &PointerState::default());
    let opacity_start = find_styled_by_id(&styled_start, "box")
        .unwrap()
        .specified_values
        .get("opacity");
    assert_eq!(opacity_start, Some(&Value::Number(1.0)));
    assert!(doc.has_pending_tasks(), "Active transition must count as pending task");

    // At t = 500ms: opacity is 0.5
    doc.runtime.now_ms = 500.0;
    let styled_mid = doc.style_tree(800.0, &PointerState::default());
    let opacity_mid = find_styled_by_id(&styled_mid, "box")
        .unwrap()
        .specified_values
        .get("opacity");
    if let Some(Value::Number(n)) = opacity_mid {
        assert!((n - 0.5).abs() < 1e-3, "Expected opacity 0.5, got {}", n);
    } else {
        panic!("expected Value::Number, got {:?}", opacity_mid);
    }

    // At t = 1000ms: opacity reaches 0.0 and transition completes
    doc.runtime.now_ms = 1000.0;
    let styled_end = doc.style_tree(800.0, &PointerState::default());
    let opacity_end = find_styled_by_id(&styled_end, "box")
        .unwrap()
        .specified_values
        .get("opacity");
    assert_eq!(opacity_end, Some(&Value::Number(0.0)));
    assert!(!doc.has_pending_tasks(), "Completed transition leaves document idle");
}

#[test]
fn test_transition_smooth_reversal() {
    let html = r#"
    <html>
        <head>
            <style>
                #box {
                    width: 0px;
                    transition: width 1s linear;
                }
                #box.expand {
                    width: 100px;
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

    // Initial style at t = 0
    doc.runtime.now_ms = 0.0;
    let _ = doc.style_tree(800.0, &PointerState::default());

    // Trigger expand at t = 0
    find_dom_by_id_mut(&mut doc.dom, "box")
        .unwrap()
        .set_attr("class", "expand");
    let _ = doc.style_tree(800.0, &PointerState::default());

    // Advance to 500ms (width should be 50px)
    doc.runtime.now_ms = 500.0;
    let styled = doc.style_tree(800.0, &PointerState::default());
    let width_mid = find_styled_by_id(&styled, "box")
        .unwrap()
        .specified_values
        .get("width");
    assert_eq!(width_mid, Some(&Value::Length(50.0, Unit::Px)));

    // Revert before finishing at t = 500ms: remove "expand" class
    find_dom_by_id_mut(&mut doc.dom, "box")
        .unwrap()
        .remove_attr("class");

    // The new transition must start from 50px towards 0px smoothly!
    doc.runtime.now_ms = 500.0;
    let styled_revert_start = doc.style_tree(800.0, &PointerState::default());
    let width_rev0 = find_styled_by_id(&styled_revert_start, "box")
        .unwrap()
        .specified_values
        .get("width");
    assert_eq!(width_rev0, Some(&Value::Length(50.0, Unit::Px)));

    // 500ms later (at t = 1000ms), it is halfway between 50px and 0px = 25px!
    doc.runtime.now_ms = 1000.0;
    let styled_rev_mid = doc.style_tree(800.0, &PointerState::default());
    let width_rev_mid = find_styled_by_id(&styled_rev_mid, "box")
        .unwrap()
        .specified_values
        .get("width");
    assert_eq!(width_rev_mid, Some(&Value::Length(25.0, Unit::Px)));
}

#[test]
fn test_browser_advance_time_transitions() {
    let html = r#"
    <html>
        <head>
            <style>
                body { margin: 0; }
                #target {
                    width: 100px;
                    height: 50px;
                    opacity: 1;
                    transition: opacity 0.5s linear;
                }
            </style>
        </head>
        <body>
            <div id="target"></div>
            <script>
                setTimeout(() => {
                    document.getElementById("target").style.opacity = "0";
                }, 100);
            </script>
        </body>
    </html>
    "#;

    let mut loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/").unwrap();
    loader.insert("http://example.com/", html.as_bytes().to_vec());

    let clock = Rc::new(ManualClock::new());
    let mut browser = Browser::open_with_clock(Box::new(loader), &url, clock.clone()).unwrap();

    // Before timer: opacity is 1
    let styled0 = browser.document().style_tree(800.0, &PointerState::default());
    assert_eq!(
        find_styled_by_id(&styled0, "target")
            .unwrap()
            .specified_values
            .get("opacity"),
        Some(&Value::Number(1.0))
    );

    // Advance 100ms: timer fires, setting style.opacity = "0", initiating transition
    browser.advance_time(Duration::from_millis(100));
    // Sample frame at 100ms
    let _ = browser.document().style_tree(800.0, &PointerState::default());

    // Advance 250ms into the transition (t = 350ms): opacity is 0.5
    browser.advance_time(Duration::from_millis(250));
    let styled = browser.document().style_tree(800.0, &PointerState::default());
    let op = find_styled_by_id(&styled, "target")
        .unwrap()
        .specified_values
        .get("opacity");
    if let Some(Value::Number(n)) = op {
        assert!((n - 0.5).abs() < 1e-3, "Expected opacity ~0.5 at midpoint, got {}", n);
    } else {
        panic!("expected Value::Number, got {:?}", op);
    }

    // Advance 250ms more (t = 600ms): transition finishes, opacity is 0.0
    browser.advance_time(Duration::from_millis(250));
    let styled_done = browser.document().style_tree(800.0, &PointerState::default());
    assert_eq!(
        find_styled_by_id(&styled_done, "target")
            .unwrap()
            .specified_values
            .get("opacity"),
        Some(&Value::Number(0.0))
    );
}
