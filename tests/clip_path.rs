use browser_engine::css::parser::parse_css;
use browser_engine::html::parse_html;
use browser_engine::layout::layout_tree;
use browser_engine::paint::{build_display_list, paint, DisplayCommand};
use browser_engine::style::style_tree;

#[test]
fn test_clip_path_parsing_and_canvas_clipping() {
    let html = r#"<div class="clipped"></div>"#;
    let css = r#"
        * {
            margin: 0;
            padding: 0;
        }
        .clipped {
            display: block;
            width: 100px;
            height: 100px;
            background-color: rgb(255, 0, 0);
            clip-path: polygon(0% 0%, 100% 0%, 0% 100%);
        }
    "#;

    let doc = parse_html(html);
    let stylesheet = parse_css(css);
    let styled = style_tree(&doc, &stylesheet);

    let layout = layout_tree(&styled, 100.0);
    let display_list = build_display_list(&layout);

    let has_clip_path = display_list.iter().any(|cmd| matches!(cmd, DisplayCommand::PushClipPath(_, _)));
    assert!(has_clip_path, "Display list should include PushClipPath");

    let canvas = paint(&layout, 100, 100);

    let idx_in = (10 * 100 + 10) * 3;
    let idx_out = (90 * 100 + 90) * 3;

    // Top-left pixel (10, 10) is inside triangle: should be red (255, 0, 0)
    assert_eq!(canvas.pixels[idx_in], 255);
    assert_eq!(canvas.pixels[idx_in + 1], 0);
    assert_eq!(canvas.pixels[idx_in + 2], 0);

    // Bottom-right pixel (90, 90) is outside triangle: should remain white (255, 255, 255)
    assert_eq!(canvas.pixels[idx_out], 255);
    assert_eq!(canvas.pixels[idx_out + 1], 255);
    assert_eq!(canvas.pixels[idx_out + 2], 255);
}
