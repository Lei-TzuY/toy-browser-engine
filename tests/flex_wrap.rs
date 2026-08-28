use browser_engine::css::parser::parse_css;
use browser_engine::html::parse_html;
use browser_engine::layout::{layout_tree, BoxType, LayoutBox};
use browser_engine::style::style_tree;

fn layout_html_css<'a>(html: &'a str, css: &'a str, viewport_w: f32) -> LayoutBox<'a> {
    let document = Box::leak(Box::new(parse_html(html)));
    let stylesheet = Box::leak(Box::new(parse_css(css)));
    let style = Box::leak(Box::new(style_tree(document, stylesheet)));

    layout_tree(style, viewport_w)
}

fn find_flex<'a, 'b>(lb: &'b LayoutBox<'a>) -> Option<&'b LayoutBox<'a>> {
    if matches!(lb.box_type, BoxType::Flex(_)) {
        return Some(lb);
    }
    lb.children.iter().find_map(find_flex)
}

#[test]
fn test_flex_wrap_row_multiline_positions() {
    let html = r#"
        <div class="container">
            <div class="item"></div>
            <div class="item"></div>
            <div class="item"></div>
        </div>
    "#;
    let css = r#"
        .container {
            display: flex;
            flex-wrap: wrap;
            width: 200px;
        }
        .item {
            width: 120px;
            height: 50px;
        }
    "#;

    let layout = layout_html_css(html, css, 400.0);
    let container = find_flex(&layout).expect("flex container not found");
    assert_eq!(container.children.len(), 3);

    // Item 0: line 0, x = 0, y = 0
    assert_eq!(container.children[0].dimensions.content.x, 0.0);
    assert_eq!(container.children[0].dimensions.content.y, 0.0);

    // Item 1: exceeds 200px (120+120=240 > 200) -> line 1, x = 0, y = 50
    assert_eq!(container.children[1].dimensions.content.x, 0.0);
    assert_eq!(container.children[1].dimensions.content.y, 50.0);

    // Item 2: exceeds line 1 (120+120=240 > 200) -> line 2, x = 0, y = 100
    assert_eq!(container.children[2].dimensions.content.x, 0.0);
    assert_eq!(container.children[2].dimensions.content.y, 100.0);
}

#[test]
fn test_flex_flow_shorthand_and_wrap_reverse() {
    let html = r#"
        <div class="container">
            <div class="item"></div>
            <div class="item"></div>
        </div>
    "#;
    let css = r#"
        .container {
            display: flex;
            flex-flow: row wrap-reverse;
            width: 150px;
            height: 200px;
        }
        .item {
            width: 100px;
            height: 40px;
        }
    "#;

    let layout = layout_html_css(html, css, 400.0);
    let container = find_flex(&layout).expect("flex container not found");
    assert_eq!(container.children.len(), 2);

    // With align-content: stretch across 200px container, the 2 lines (40px each)
    // are stretched by 60px each (line size = 100px).
    // Under wrap-reverse:
    // Line 0 (bottom) is at y = 200 - 100 = 100.0
    // Line 1 (top) is at y = 0.0
    assert_eq!(container.children[0].dimensions.content.y, 100.0);
    assert_eq!(container.children[1].dimensions.content.y, 0.0);
}
