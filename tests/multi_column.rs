use browser_engine::css::parser::parse_css;
use browser_engine::html::parse_html;
use browser_engine::layout::layout_tree;
use browser_engine::style::style_tree;

#[test]
fn test_multi_column_layout_calculation() {
    let html = r#"
        <div class="container">
            <div class="card">Item 1</div>
            <div class="card">Item 2</div>
        </div>
    "#;
    let css = r#"
        .container {
            display: block;
            width: 200px;
            column-count: 2;
            column-gap: 20px;
        }
        .card {
            display: block;
            height: 50px;
        }
    "#;

    let doc = parse_html(html);
    let stylesheet = parse_css(css);
    let styled = style_tree(&doc, &stylesheet);

    let layout = layout_tree(&styled, 800.0);

    // Find the container box
    let container_box = layout
        .children
        .iter()
        .flat_map(|b| std::iter::once(b).chain(b.children.iter()))
        .find(|b| {
            b.styled_node()
                .map(|s| s.value("column-count").is_some())
                .unwrap_or(false)
        })
        .expect("container box found");

    let cards: Vec<_> = container_box
        .children
        .iter()
        .filter(|c| c.styled_node().is_some())
        .collect();

    assert_eq!(cards.len(), 2);
    // col_width = (200 - 20) / 2 = 90px
    let card1 = cards[0];
    let card2 = cards[1];

    assert_eq!(card1.dimensions.content.width, 90.0);
    assert_eq!(card2.dimensions.content.width, 90.0);

    // card2 should be placed at col 1: x_offset = 1 * (90 + 20) = 110px
    assert_eq!(card2.dimensions.content.x - card1.dimensions.content.x, 110.0);
}
