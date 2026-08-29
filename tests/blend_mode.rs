use browser_engine::css::parser::{parse_css, BlendMode, Color};
use browser_engine::dom::Node;
use browser_engine::layout::layout_tree;
use browser_engine::paint::{build_display_list, Canvas, DisplayCommand};
use browser_engine::style::style_tree;

#[test]
fn test_blend_mode_parsing_and_canvas_pixel_blending() {
    let css = r#"
        div.box {
            width: 100px;
            height: 100px;
            background-color: rgb(255, 0, 0);
            mix-blend-mode: multiply;
        }
    "#;
    let stylesheet = parse_css(css);
    let dom = Node::element(
        "div",
        vec![("class".to_string(), "box".to_string())],
    );

    let styled = style_tree(&dom, &stylesheet);
    assert_eq!(styled.mix_blend_mode(), BlendMode::Multiply);

    let layout = layout_tree(&styled, 100.0);
    let display_list = build_display_list(&layout);

    let has_push_blend = display_list.iter().any(|cmd| matches!(cmd, DisplayCommand::PushBlendMode(BlendMode::Multiply)));
    let has_pop_blend = display_list.iter().any(|cmd| matches!(cmd, DisplayCommand::PopBlendMode));
    assert!(has_push_blend, "Display list should include PushBlendMode");
    assert!(has_pop_blend, "Display list should include PopBlendMode");

    // Test pixel blending directly on Canvas:
    // Create cyan canvas (0, 255, 255)
    let mut canvas = Canvas::new(10, 10, Color::rgb(0, 255, 255));
    // Push multiply blend mode
    canvas.paint(&DisplayCommand::PushBlendMode(BlendMode::Multiply));
    // Paint yellow rectangle (255, 255, 0)
    canvas.paint(&DisplayCommand::SolidColor(
        Color::rgb(255, 255, 0),
        browser_engine::layout::Rect {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        },
    ));
    canvas.paint(&DisplayCommand::PopBlendMode);

    // Cyan (0, 255, 255) * Yellow (255, 255, 0) = Green (0, 255, 0)
    let idx = 0;
    assert_eq!(canvas.pixels[idx], 0);     // Red: 0 * 255 / 255 = 0
    assert_eq!(canvas.pixels[idx + 1], 255); // Green: 255 * 255 / 255 = 255
    assert_eq!(canvas.pixels[idx + 2], 0);   // Blue: 255 * 0 / 255 = 0
}
