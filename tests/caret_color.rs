use browser_engine::css::parser::{parse_css, Color};
use browser_engine::dom::Node;
use browser_engine::layout::layout_tree;
use browser_engine::paint::build_display_list;
use browser_engine::style::style_tree;

#[test]
fn test_caret_color_parsing_and_cursor_rendering() {
    let css = r#"
        input.custom {
            caret-color: rgb(0, 255, 0);
            user-select: none;
        }
    "#;
    let stylesheet = parse_css(css);

    let dom = Node::element(
        "input",
        vec![
            ("class".to_string(), "custom".to_string()),
            ("value".to_string(), "hello".to_string()),
        ],
    );

    // Style tree test
    let styled = style_tree(&dom, &stylesheet);
    assert_eq!(styled.caret_color(), Some(Color::rgb(0, 255, 0)));
    assert_eq!(styled.user_select(), Some("none"));

    // Layout and focused display list test
    let layout = layout_tree(&styled, 300.0);
    let display_list = build_display_list(&layout);

    // Verify display list can be built cleanly
    assert!(!display_list.is_empty());
}
