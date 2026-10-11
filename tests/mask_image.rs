use browser_engine::css::parser::parse_css;
use browser_engine::dom::Node;
use browser_engine::style::style_tree;

#[test]
fn test_mask_image_linear_gradient_parsing() {
    let css = r#"
        div.masked {
            width: 150px;
            height: 150px;
            mask-image: linear-gradient(180deg, rgb(0, 0, 0) 0%, rgba(0, 0, 0, 0) 100%);
        }
    "#;
    let stylesheet = parse_css(css);
    let dom = Node::element(
        "div",
        vec![("class".to_string(), "masked".to_string())],
    );

    let styled = style_tree(&dom, &stylesheet);
    let mask = styled.mask_image();
    assert!(mask.is_some(), "StyledNode should have mask_image linear gradient");

    let grad = mask.unwrap();
    assert_eq!(grad.angle_deg, 180.0);
    assert_eq!(grad.stops.len(), 2);
}
