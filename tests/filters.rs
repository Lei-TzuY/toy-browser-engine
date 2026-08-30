use browser_engine::canvas::CanvasContext2D;
use browser_engine::css::parser::{parse_css, FilterFunction, Value};
use browser_engine::document::Document;
use browser_engine::net::{MemoryLoader, Url};

fn run_js(html: &str, js: &str) -> Document {
    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/index.html").unwrap();
    let full_html = format!(
        "<!DOCTYPE html><html><head><style>#box {{ filter: blur(5px) grayscale(100%) brightness(1.2); }}</style></head><body>{}<script>{}</script></body></html>",
        html, js
    );
    Document::from_html(&full_html, &url, &loader)
}

#[test]
fn test_css_filter_parsing_and_computed_style() {
    let sheet = parse_css(
        r#"
        .hero { filter: blur(4px) grayscale(80%) brightness(1.5) contrast(2) invert(100%) opacity(0.5); }
        .reset { filter: none; }
    "#,
    );

    assert_eq!(sheet.rules.len(), 2);
    let decl = &sheet.rules[0].declarations[0];
    assert_eq!(decl.name, "filter");
    if let Value::Filter(funcs) = &decl.value {
        assert_eq!(funcs.len(), 6);
        assert_eq!(funcs[0], FilterFunction::Blur(4.0));
        assert_eq!(funcs[1], FilterFunction::Grayscale(0.8));
        assert_eq!(funcs[2], FilterFunction::Brightness(1.5));
        assert_eq!(funcs[3], FilterFunction::Contrast(2.0));
        assert_eq!(funcs[4], FilterFunction::Invert(1.0));
        assert_eq!(funcs[5], FilterFunction::Opacity(0.5));
    } else {
        panic!("Expected Value::Filter, got {:?}", decl.value);
    }

    let doc = run_js(
        r#"<div id="box" class="hero"></div>"#,
        r#"
            let el = document.getElementById("box");
            let style = window.getComputedStyle(el);
            console.log("filter:" + style.getPropertyValue("filter"));
        "#,
    );

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "filter:blur(5px) grayscale(100%) brightness(1.2)");
}

#[test]
fn test_canvas_2d_filter_effects() {
    let mut ctx = CanvasContext2D::new(10, 10);
    ctx.set_filter("grayscale(100%)");
    assert_eq!(ctx.filter, "grayscale(100%)");

    // Draw pure red rectangle: (255, 0, 0, 255)
    ctx.fill_style = browser_engine::css::parser::Color::rgba(255, 0, 0, 255);
    ctx.fill_rect(0.0, 0.0, 10.0, 10.0);

    // Initial red pixel before filter application
    assert_eq!(ctx.pixels[0], 255);
    assert_eq!(ctx.pixels[1], 0);
    assert_eq!(ctx.pixels[2], 0);

    // Apply grayscale filter: y = 0.2126 * 255 = 54
    ctx.apply_filters();
    assert_eq!(ctx.pixels[0], 54);
    assert_eq!(ctx.pixels[1], 54);
    assert_eq!(ctx.pixels[2], 54);

    // Test Invert filter
    let mut ctx2 = CanvasContext2D::new(10, 10);
    ctx2.set_filter("invert(100%)");
    ctx2.fill_style = browser_engine::css::parser::Color::rgba(100, 150, 200, 255);
    ctx2.fill_rect(0.0, 0.0, 10.0, 10.0);
    ctx2.apply_filters();

    assert_eq!(ctx2.pixels[0], 255 - 100);
    assert_eq!(ctx2.pixels[1], 255 - 150);
    assert_eq!(ctx2.pixels[2], 255 - 200);

    // Test Brightness filter
    let mut ctx3 = CanvasContext2D::new(10, 10);
    ctx3.set_filter("brightness(1.5)");
    ctx3.fill_style = browser_engine::css::parser::Color::rgba(100, 100, 100, 255);
    ctx3.fill_rect(0.0, 0.0, 10.0, 10.0);
    ctx3.apply_filters();

    assert_eq!(ctx3.pixels[0], 150);
    assert_eq!(ctx3.pixels[1], 150);
    assert_eq!(ctx3.pixels[2], 150);
}
