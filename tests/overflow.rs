use browser_engine::css::parser::parse_css;
use browser_engine::html::parse_html;
use browser_engine::layout::layout_tree;
use browser_engine::paint::{build_display_list, DisplayCommand};
use browser_engine::style::style_tree;

#[test]
fn test_overflow_hidden_generates_clipping_commands() {
    let html = r#"
        <div class="outer">
            <div class="inner">Hello Overflow</div>
        </div>
    "#;
    let css = r#"
        .outer {
            display: block;
            width: 100px;
            height: 50px;
            overflow: hidden;
        }
        .inner {
            display: block;
            width: 200px;
            height: 100px;
        }
    "#;

    let doc = parse_html(html);
    let stylesheet = parse_css(css);
    let styled = style_tree(&doc, &stylesheet);
    let layout = layout_tree(&styled, 800.0);
    let display_list = build_display_list(&layout);

    let has_push_clip = display_list
        .iter()
        .any(|cmd| matches!(cmd, DisplayCommand::PushClip(_)));
    let has_pop_clip = display_list
        .iter()
        .any(|cmd| matches!(cmd, DisplayCommand::PopClip));

    assert!(has_push_clip, "expected PushClip for overflow: hidden");
    assert!(has_pop_clip, "expected PopClip for overflow: hidden");
}

#[test]
fn test_overflow_visible_does_not_clip() {
    let html = r#"
        <div class="outer">
            <div class="inner">Hello Visible</div>
        </div>
    "#;
    let css = r#"
        .outer {
            display: block;
            width: 100px;
            height: 50px;
            overflow: visible;
        }
        .inner {
            display: block;
            width: 200px;
            height: 100px;
        }
    "#;

    let doc = parse_html(html);
    let stylesheet = parse_css(css);
    let styled = style_tree(&doc, &stylesheet);
    let layout = layout_tree(&styled, 800.0);
    let display_list = build_display_list(&layout);

    let has_push_clip = display_list
        .iter()
        .any(|cmd| matches!(cmd, DisplayCommand::PushClip(_)));

    assert!(!has_push_clip, "overflow: visible should not generate PushClip");
}
