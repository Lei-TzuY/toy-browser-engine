use browser_engine::css::parser::{parse_css, FilterFunction, Value};
use browser_engine::html::parse_html;
use browser_engine::layout::layout_tree;
use browser_engine::paint::{build_display_list, paint, DisplayCommand};
use browser_engine::style::style_tree;

#[test]
fn test_backdrop_filter_parsing_and_canvas_rasterization() {
    let css = r#"
        body {
            background-color: rgb(10, 20, 30);
        }
        .glass {
            width: 100px;
            height: 100px;
            backdrop-filter: invert(100%);
        }
    "#;
    let stylesheet = parse_css(css);
    let decl = &stylesheet.rules[1].declarations[2];
    assert_eq!(decl.name, "backdrop-filter");
    if let Value::Filter(ref filters) = decl.value {
        assert_eq!(filters.len(), 1);
        match &filters[0] {
            FilterFunction::Invert(amt) => assert!((amt - 1.0).abs() < 1e-3),
            _ => panic!("Expected Invert filter"),
        }
    } else {
        panic!("Expected Value::Filter");
    }

    let html = r#"<html><head></head><body><div class="glass"></div></body></html>"#;
    let dom = parse_html(html);
    let styled = style_tree(&dom, &stylesheet);
    let layout = layout_tree(&styled, 200.0);
    let display_list = build_display_list(&layout);

    let has_backdrop_filter = display_list.iter().any(|cmd| matches!(cmd, DisplayCommand::BackdropFilter(_, _)));
    assert!(has_backdrop_filter, "Display list should contain BackdropFilter command");

    let canvas = paint(&layout, 200, 200);

    // Initial background was (10, 20, 30). With invert(100%), area inside glass should be inverted:
    // (255 - 10, 255 - 20, 255 - 30) = (245, 235, 225)
    let pixel_idx = (10 * 200 + 10) * 3;
    let r = canvas.pixels[pixel_idx];
    let g = canvas.pixels[pixel_idx + 1];
    let b = canvas.pixels[pixel_idx + 2];
    assert_eq!(r, 245);
    assert_eq!(g, 235);
    assert_eq!(b, 225);
}
