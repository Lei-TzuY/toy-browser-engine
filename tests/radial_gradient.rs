use browser_engine::css::parser::parse_css;
use browser_engine::html::parse_html;
use browser_engine::layout::layout_tree;
use browser_engine::paint::{build_display_list, paint, DisplayCommand};
use browser_engine::style::style_tree;

#[test]
fn test_radial_gradient_parsing_and_display_command() {
    let html = r#"<div class="circle-grad"></div>"#;
    let css = r#"
        .circle-grad {
            width: 100px;
            height: 100px;
            background: radial-gradient(circle, #ff0000 0%, #0000ff 100%);
        }
    "#;

    let doc = parse_html(html);
    let stylesheet = parse_css(css);
    let styled = style_tree(&doc, &stylesheet);
    let layout = layout_tree(&styled, 800.0);
    let display_list = build_display_list(&layout);

    let has_radial_grad = display_list
        .iter()
        .any(|cmd| matches!(cmd, DisplayCommand::RadialGradient(_, _, _)));
    assert!(has_radial_grad, "expected RadialGradient display command");

    // Also verify rasterization doesn't panic and renders pixels
    let canvas = paint(&layout, 100, 100);
    assert_eq!(canvas.width, 100);
    assert_eq!(canvas.height, 100);

    // Center pixel (50, 50) should be predominantly red (#ff0000)
    let center_idx = (50 * 100 + 50) * 3;
    let r = canvas.pixels[center_idx];
    let b = canvas.pixels[center_idx + 2];
    assert!(r > b, "center of radial gradient should have higher red than blue");

    // Corner pixel (0, 0) should be predominantly blue (#0000ff)
    let corner_idx = 0;
    let r_corner = canvas.pixels[corner_idx];
    let b_corner = canvas.pixels[corner_idx + 2];
    assert!(b_corner > r_corner, "corner of radial gradient should have higher blue than red");
}
