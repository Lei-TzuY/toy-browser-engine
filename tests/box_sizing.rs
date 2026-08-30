use browser_engine::css::parser::parse_css;
use browser_engine::html::parse_html;
use browser_engine::layout::layout_tree;
use browser_engine::style::style_tree;

#[test]
fn test_box_sizing_border_box_vs_content_box() {
    let html = r#"
        <div class="content-box"></div>
        <div class="border-box"></div>
    "#;
    let css = r#"
        .content-box {
            display: block;
            box-sizing: content-box;
            width: 200px;
            padding: 20px;
            border: 5px solid black;
        }
        .border-box {
            display: block;
            box-sizing: border-box;
            width: 200px;
            padding: 20px;
            border: 5px solid black;
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

    let cb = find_by_class(&layout, "content-box").expect("content-box element not found");
    assert_eq!(cb.dimensions.content.width, 200.0);
    assert_eq!(cb.dimensions.border_box().width, 250.0);

    let bb = find_by_class(&layout, "border-box").expect("border-box element not found");
    assert_eq!(bb.dimensions.content.width, 150.0);
    assert_eq!(bb.dimensions.border_box().width, 200.0);
}
