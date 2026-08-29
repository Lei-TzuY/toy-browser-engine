use browser_engine::css::parser::{parse_css, Color};
use browser_engine::dom::Node;
use browser_engine::layout::layout_tree;
use browser_engine::paint::{build_display_list, DisplayCommand};
use browser_engine::style::style_tree;

#[test]
fn test_outline_and_outline_offset_parsing_and_painting() {
    let css = r#"
        div.target {
            width: 100px;
            height: 100px;
            outline: 2px solid rgb(255, 0, 0);
            outline-offset: 4px;
        }
    "#;
    let stylesheet = parse_css(css);
    let dom = Node::element(
        "div",
        vec![("class".to_string(), "target".to_string())],
    );

    let styled = style_tree(&dom, &stylesheet);
    assert_eq!(styled.outline_width(), 2.0);
    assert_eq!(styled.outline_style(), Some("solid"));
    assert_eq!(styled.outline_color(), Color::rgb(255, 0, 0));
    assert_eq!(styled.outline_offset(), 4.0);

    let layout = layout_tree(&styled, 200.0);
    let display_list = build_display_list(&layout);

    // Should contain solid color commands for the outline with width 2.0
    let outline_cmds: Vec<_> = display_list
        .iter()
        .filter(|cmd| match cmd {
            DisplayCommand::SolidColor(c, r) => *c == Color::rgb(255, 0, 0) && (r.width == 2.0 || r.height == 2.0),
            _ => false,
        })
        .collect();

    assert_eq!(outline_cmds.len(), 4, "Outline should produce 4 solid color side commands");
}
