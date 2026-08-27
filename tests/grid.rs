use browser_engine::css::parse_css;
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
fn test_grid_repeat_tracks() {
    let layout = layout_html_css(
        r#"<div class="grid">
            <div class="item1">1</div>
            <div class="item2">2</div>
            <div class="item3">3</div>
        </div>"#,
        r#"
            .grid { display: grid; grid-template-columns: repeat(3, 100px); column-gap: 10px; width: 400px; }
            .item1, .item2, .item3 { height: 50px; }
        "#,
        400.0,
    );

    let grid = find_grid(&layout).expect("grid box not found");
    assert_eq!(grid.children.len(), 3);
    assert!((grid.children[0].dimensions.content.width - 100.0).abs() < 1.0);
    assert!((grid.children[1].dimensions.content.width - 100.0).abs() < 1.0);
    assert!((grid.children[2].dimensions.content.width - 100.0).abs() < 1.0);

    // Positions with column-gap = 10: 0, 110, 220
    assert!((grid.children[0].dimensions.content.x - 0.0).abs() < 1.0);
    assert!((grid.children[1].dimensions.content.x - 110.0).abs() < 1.0);
    assert!((grid.children[2].dimensions.content.x - 220.0).abs() < 1.0);
}

#[test]
fn test_grid_column_span() {
    let layout = layout_html_css(
        r#"<div class="grid">
            <div class="header">Header</div>
            <div class="item">Item 1</div>
            <div class="item">Item 2</div>
        </div>"#,
        r#"
            .grid { display: grid; grid-template-columns: 100px 100px; gap: 10px; width: 400px; }
            .header { grid-column: 1 / 3; height: 40px; }
            .item { height: 30px; }
        "#,
        400.0,
    );

    let grid = find_grid(&layout).expect("grid box not found");
    assert_eq!(grid.children.len(), 3);
    // Header spans 2 columns: 100 + 10 + 100 = 210px
    assert!((grid.children[0].dimensions.content.width - 210.0).abs() < 1.0);
    assert!((grid.children[0].dimensions.content.x - 0.0).abs() < 1.0);
    assert!((grid.children[0].dimensions.content.y - 0.0).abs() < 1.0);

    // Next items flow to row 2: y = 40 (header) + 10 (gap) = 50
    assert!((grid.children[1].dimensions.content.y - 50.0).abs() < 1.0);
    assert!((grid.children[1].dimensions.content.x - 0.0).abs() < 1.0);
    assert!((grid.children[2].dimensions.content.y - 50.0).abs() < 1.0);
    assert!((grid.children[2].dimensions.content.x - 110.0).abs() < 1.0);
}

#[test]
fn test_grid_row_span_and_auto_placement() {
    let layout = layout_html_css(
        r#"<div class="grid">
            <div class="sidebar">Sidebar</div>
            <div class="main1">Main 1</div>
            <div class="main2">Main 2</div>
        </div>"#,
        r#"
            .grid { display: grid; grid-template-columns: 100px 200px; gap: 10px; width: 400px; }
            .sidebar { grid-row: span 2; height: 100px; }
            .main1 { height: 45px; }
            .main2 { height: 45px; }
        "#,
        400.0,
    );

    let grid = find_grid(&layout).expect("grid box not found");
    assert_eq!(grid.children.len(), 3);

    // Sidebar placed at col 0, row 0..2
    assert!((grid.children[0].dimensions.content.x - 0.0).abs() < 1.0);
    assert!((grid.children[0].dimensions.content.y - 0.0).abs() < 1.0);

    // Main 1 placed at col 1, row 0
    assert!((grid.children[1].dimensions.content.x - 110.0).abs() < 1.0);
    assert!((grid.children[1].dimensions.content.y - 0.0).abs() < 1.0);

    // Main 2 placed at col 1, row 1 (since col 0 is occupied by sidebar)
    assert!((grid.children[2].dimensions.content.x - 110.0).abs() < 1.0);
    assert!(grid.children[2].dimensions.content.y > 40.0);
}

#[test]
fn test_grid_cell_alignment() {
    let layout = layout_html_css(
        r#"<div class="grid">
            <div class="center-item">C</div>
        </div>"#,
        r#"
            .grid { display: grid; grid-template-columns: 200px; width: 400px; }
            .center-item { width: 60px; height: 40px; justify-self: center; align-self: center; }
        "#,
        400.0,
    );

    let grid = find_grid(&layout).expect("grid box not found");
    assert_eq!(grid.children.len(), 1);
    let item = &grid.children[0];

    // Horizontal centering: (200 - 60) / 2 = 70
    assert!((item.dimensions.content.x - 70.0).abs() < 1.0);
}
