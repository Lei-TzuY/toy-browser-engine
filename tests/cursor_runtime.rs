use browser_engine::cursor::{cursor_for_path, CursorIcon};
use browser_engine::script::dom_api;
use browser_engine::{Document, MemoryLoader, PointerState, Url};

fn document(html: &str) -> Document {
    let url = Url::parse("demo:///cursor-runtime.html").unwrap();
    Document::from_html(html, &url, &MemoryLoader::new())
}

#[test]
fn public_cursor_runtime_reads_ua_and_author_styles() {
    let doc = document(
        "<style>#custom { cursor: not-allowed; }</style>\
         <a id='link' href='/next'>next</a>\
         <div id='custom'>blocked</div>",
    );

    let link = dom_api::query_selector(&doc.dom, &[], "#link").unwrap();
    let custom = dom_api::query_selector(&doc.dom, &[], "#custom").unwrap();

    assert_eq!(
        cursor_for_path(&doc, &link, 800.0, &PointerState::default()),
        Some(CursorIcon::Pointer)
    );
    assert_eq!(
        cursor_for_path(&doc, &custom, 800.0, &PointerState::default()),
        Some(CursorIcon::NotAllowed)
    );
}

#[test]
fn public_cursor_runtime_restyles_hovered_element() {
    let doc = document(
        "<style>#target { cursor: default; } #target:hover { cursor: grab; }</style>\
         <div id='target'>target</div>",
    );
    let target = dom_api::query_selector(&doc.dom, &[], "#target").unwrap();
    let pointer = PointerState {
        hovered: Some(target.clone()),
        ..PointerState::default()
    };

    assert_eq!(
        cursor_for_path(&doc, &target, 800.0, &pointer),
        Some(CursorIcon::Grab)
    );
}
