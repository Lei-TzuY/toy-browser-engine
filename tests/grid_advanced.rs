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

fn find_grid<'a, 'b>(lb: &'b LayoutBox<'a>) -> Option<&'b LayoutBox<'a>> {
    if matches!(lb.box_type, BoxType::Grid(_)) {
        return Some(lb);
    }
    lb.children.iter().find_map(find_grid)
}

#[test]
fn test_grid_auto_fill_minmax() {
    let html = r#"
        <div class="grid">
            <div class="item">1</div>
            <div class="item">2</div>
            <div class="item">3</div>
        </div>
    "#;
    let css = r#"
        .grid {
            display: grid;
            grid-template-columns: repeat(auto-fill, minmax(100px, 1fr));
            width: 320px;
            gap: 10px;
        }
        .item {
            height: 50px;
        }
    "#;

    let layout = layout_html_css(html, css, 400.0);
    let grid = find_grid(&layout).expect("grid container not found");
    assert_eq!(grid.children.len(), 3);

    // In a 320px container with 10px gap:
    // (320 + 10) / (100 + 10) = 330 / 110 = 3 columns.
    // 3 columns with 2 gaps of 10px = 20px gaps.
    // Remaining 300px / 3 = 100px each!
    let item0 = &grid.children[0];
    let item1 = &grid.children[1];
    let item2 = &grid.children[2];

    assert_eq!(item0.dimensions.content.x, 0.0);
    assert_eq!(item0.dimensions.content.width, 100.0);

    assert_eq!(item1.dimensions.content.x, 110.0); // 100 + 10
    assert_eq!(item1.dimensions.content.width, 100.0);

    assert_eq!(item2.dimensions.content.x, 220.0); // 110 + 100 + 10
    assert_eq!(item2.dimensions.content.width, 100.0);
}
