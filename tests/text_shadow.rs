use browser_engine::css::parser::{parse_css, Color, Value};
use browser_engine::html::parse_html;
use browser_engine::layout::layout_tree;
use browser_engine::paint::{build_display_list, DisplayCommand};
use browser_engine::style::style_tree;

#[test]
fn test_text_shadow_parsing_and_display_commands() {
    let css = r#"
        h1 {
            text-shadow: 2px 4px 5px rgb(255, 0, 0);
        }
    "#;
    let stylesheet = parse_css(css);
    let decl = &stylesheet.rules[0].declarations[0];
    assert_eq!(decl.name, "text-shadow");
    if let Value::TextShadow(ref ts) = decl.value {
        assert_eq!(ts.offset_x, 2.0);
        assert_eq!(ts.offset_y, 4.0);
        assert_eq!(ts.blur_radius, 5.0);
        assert_eq!(ts.color, Color::rgb(255, 0, 0));
    } else {
        panic!("Expected Value::TextShadow");
    }

    let html = r#"<html><head></head><body><h1>Hello World</h1></body></html>"#;
    let dom = parse_html(html);
    let styled = style_tree(&dom, &stylesheet);
    let layout = layout_tree(&styled, 800.0);
    let display_list = build_display_list(&layout);

    // Filter all text commands
    let text_cmds: Vec<&DisplayCommand> = display_list
        .iter()
        .filter(|cmd| matches!(cmd, DisplayCommand::Text(_)))
        .collect();

    // Since text-shadow is active, we should have the shadow text fragment emitted right before the foreground text fragment
    assert!(text_cmds.len() >= 2, "Expected at least shadow text and main text commands");

    if let (DisplayCommand::Text(shadow_frag), DisplayCommand::Text(main_frag)) = (text_cmds[0], text_cmds[1]) {
        assert_eq!(shadow_frag.color, Color::rgb(255, 0, 0));
        assert_eq!(shadow_frag.rect.x, main_frag.rect.x + 2.0);
        assert_eq!(shadow_frag.rect.y, main_frag.rect.y + 4.0);
    } else {
        panic!("Display commands were not text fragments");
    }
}
