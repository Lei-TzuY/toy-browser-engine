use browser_engine::css::parser::{evaluate_supports_condition, parse_css};
use browser_engine::dom::Node;
use browser_engine::style::style_tree;

#[test]
fn test_scroll_behavior_and_snap_properties() {
    assert!(evaluate_supports_condition("(scroll-behavior: smooth)"));
    assert!(evaluate_supports_condition("(scroll-snap-type: y mandatory)"));

    let css = r#"
        div.container {
            scroll-behavior: smooth;
            scroll-snap-type: y mandatory;
        }
        div.item {
            scroll-snap-align: center;
        }
    "#;
    let stylesheet = parse_css(css);

    let dom = Node::element(
        "div",
        vec![("class".to_string(), "container".to_string())],
    );
    let styled = style_tree(&dom, &stylesheet);
    assert_eq!(styled.scroll_behavior(), "smooth");
    assert_eq!(styled.scroll_snap_type(), "y mandatory");

    let item_dom = Node::element(
        "div",
        vec![("class".to_string(), "item".to_string())],
    );
    let item_styled = style_tree(&item_dom, &stylesheet);
    assert_eq!(item_styled.scroll_snap_align(), "center");
}
