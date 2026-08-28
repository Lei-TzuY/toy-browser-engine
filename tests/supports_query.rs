use browser_engine::css::parser::parse_css;
use browser_engine::html::parse_html;
use browser_engine::style::style_tree;

#[test]
fn test_supports_at_rule_applied() {
    let css = r#"
        .box {
            color: red;
        }
        @supports (display: grid) {
            .box {
                color: green;
                display: grid;
            }
        }
        @supports (nonexistent-prop: invalid) {
            .box {
                color: blue;
            }
        }
        @supports not (unknown-property: foo) {
            .box {
                background-color: black;
            }
        }
    "#;

    let html = r#"<div class="box"></div>"#;
    let doc = parse_html(html);
    let stylesheet = parse_css(css);
    let styled_root = style_tree(&doc, &stylesheet);

    let box_styled = &styled_root.children[0];
    let style = &box_styled.specified_values;

    // @supports (display: grid) is supported -> color: green
    assert_eq!(
        style.get("color").unwrap().to_css_string(),
        "rgb(0, 128, 0)"
    );

    // @supports (nonexistent-prop: invalid) is not supported -> not blue

    // @supports not (unknown-property: foo) is supported -> background-color: black
    assert_eq!(
        style.get("background-color").unwrap().to_css_string(),
        "rgb(0, 0, 0)"
    );
}
