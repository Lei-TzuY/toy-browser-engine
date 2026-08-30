use browser_engine::css::parser::parse_css;
use browser_engine::html::parse_html;
use browser_engine::layout::layout_tree;
use browser_engine::paint::{build_display_list, paint, DisplayCommand};
use browser_engine::style::style_tree;

#[test]
fn test_conic_gradient_parsing_and_rasterization() {
    let html = r#"<div class="conic-grad"></div>"#;
    let css = r#"
        .conic-grad {
            width: 100px;
            height: 100px;
            background: conic-gradient(from 0deg, #ff0000 0%, #00ff00 50%, #0000ff 100%);
        }
    "#;

    let doc = parse_html(html);
    let stylesheet = parse_css(css);
    let styled = style_tree(&doc, &stylesheet);
    let layout = layout_tree(&styled, 800.0);
    let display_list = build_display_list(&layout);

    let has_conic = display_list
        .iter()
        .any(|cmd| matches!(cmd, DisplayCommand::ConicGradient(_, _, _)));
    assert!(has_conic, "expected ConicGradient display command");

    let canvas = paint(&layout, 100, 100);
    assert_eq!(canvas.width, 100);
    assert_eq!(canvas.height, 100);

    // Pixel directly above center (50, 25): angle ~ 0deg / top -> red dominates
    let top_idx = (25 * 100 + 50) * 3;
    let r_top = canvas.pixels[top_idx];
    let g_top = canvas.pixels[top_idx + 1];
    let b_top = canvas.pixels[top_idx + 2];
    assert!(
        r_top > g_top && r_top > b_top,
        "top of conic gradient should be red"
    );

    // Pixel directly below center (50, 75): angle ~ 180deg (50%) -> green dominates
    let bottom_idx = (75 * 100 + 50) * 3;
    let r_bottom = canvas.pixels[bottom_idx];
    let g_bottom = canvas.pixels[bottom_idx + 1];
    let b_bottom = canvas.pixels[bottom_idx + 2];
    assert!(
        g_bottom > r_bottom && g_bottom > b_bottom,
        "bottom of conic gradient should be green"
    );
}
