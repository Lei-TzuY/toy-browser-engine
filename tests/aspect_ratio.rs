use browser_engine::css::parser::parse_css;
use browser_engine::html::parse_html;
use browser_engine::layout::layout_tree;
use browser_engine::style::style_tree;

#[test]
fn test_aspect_ratio_layout_dimensions() {
    let html = r#"
        <div class="box-16-9"></div>
        <div class="box-square"></div>
    "#;
    let css = r#"
        .box-16-9 {
            display: block;
            width: 160px;
            aspect-ratio: 16 / 9;
        }
        .box-square {
            display: block;
            width: 120px;
            aspect-ratio: 1;
        }
    "#;

    let doc = parse_html(html);
    let stylesheet = parse_css(css);
    let styled = style_tree(&doc, &stylesheet);
    let layout = layout_tree(&styled, 800.0);

    fn find_by_class<'a, 'b>(
        lb: &'b browser_engine::layout::LayoutBox<'a>,
        class: &str,
    ) -> Option<&'b browser_engine::layout::LayoutBox<'a>> {
        if let Some(s) = lb.styled_node() {
            if let browser_engine::dom::NodeType::Element(e) = &s.node.node_type {
                if e.get_attr("class") == Some(class) {
                    return Some(lb);
                }
            }
        }
        lb.children.iter().find_map(|c| find_by_class(c, class))
    }

    let b1 = find_by_class(&layout, "box-16-9").expect("box-16-9 not found");
    assert_eq!(b1.dimensions.content.width, 160.0);
    assert!(
        (b1.dimensions.content.height - 90.0).abs() < 1.0,
        "160px width with 16/9 aspect-ratio should have 90px height"
    );

    let b2 = find_by_class(&layout, "box-square").expect("box-square not found");
    assert_eq!(b2.dimensions.content.width, 120.0);
    assert_eq!(
        b2.dimensions.content.height, 120.0,
        "120px width with 1:1 aspect-ratio should have 120px height"
    );
}
