use browser_engine::css::parser::parse_css;
use browser_engine::dom::Node;
use browser_engine::layout::layout_tree;
use browser_engine::style::style_tree;

#[test]
fn test_table_layout_with_border_spacing() {
    let css = r#"
        table {
            display: table;
            width: 310px;
            border-spacing: 10px;
        }
        tr {
            display: table-row;
        }
        td {
            display: table-cell;
        }
        .col1 {
            width: 100px;
            height: 40px;
        }
        .col2 {
            width: 200px;
            height: 60px;
        }
    "#;
    let stylesheet = parse_css(css);
    let dom = Node::element(
        "table",
        vec![],
    );
    let mut table_node = dom;
    let mut row_node = Node::element("tr", vec![]);
    let mut cell1 = Node::element(
        "td",
        vec![("class".to_string(), "col1".to_string())],
    );
    cell1.children.push(Node::text("A"));
    let mut cell2 = Node::element(
        "td",
        vec![("class".to_string(), "col2".to_string())],
    );
    cell2.children.push(Node::text("B"));
    row_node.children.push(cell1);
    row_node.children.push(cell2);
    table_node.children.push(row_node);

    let styled = style_tree(&table_node, &stylesheet);
    assert_eq!(styled.border_spacing(), 10.0);
    assert_eq!(styled.border_collapse(), "separate");

    let layout = layout_tree(&styled, 400.0);
    assert_eq!(layout.children.len(), 1);
    let row = &layout.children[0];
    assert_eq!(row.children.len(), 2);

    let cell1_box = &row.children[0];
    let cell2_box = &row.children[1];

    // Cell 2 X should be offset by Cell 1 width + border-spacing (10px)
    assert_eq!(cell2_box.dimensions.content.x, cell1_box.dimensions.content.x + cell1_box.dimensions.content.width + 10.0);
    // Equalized row height should match tallest cell (60px)
    assert_eq!(row.dimensions.content.height, 60.0);
    assert_eq!(cell1_box.dimensions.content.height, 60.0);
}
