use browser_engine::browser::Browser;
use browser_engine::cursor::CursorIcon;
use browser_engine::cursor_assets::{parse_cursor_value, CursorResolver, ResolvedCursor};
use browser_engine::document::PointerState;
use browser_engine::net::{DefaultLoader, MemoryLoader, Url};
use browser_engine::script::dom_api;

const PNG2: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAFElEQVR4nGP4z8DwHwyBNBAw/AcAR8oI+ItOQ4UAAAAASUVORK5CYII=";

fn browser_with(body: String) -> Browser {
    let mut memory = MemoryLoader::new();
    memory.insert("demo:///index.html", body);
    Browser::open(
        Box::new(DefaultLoader::new().with_memory(memory)),
        &Url::parse("demo:///index.html").unwrap(),
    )
    .unwrap()
}

#[test]
fn resolver_tries_candidates_in_order_and_applies_css_hotspot() {
    let browser = browser_with(format!(
        r#"<style>
              #target {{ cursor: url("missing.cur"), url("{PNG2}") 1 1, crosshair; }}
            </style>
            <div id="target">target</div>"#
    ));
    let path = dom_api::get_element_by_id(&browser.document().dom, "target").unwrap();
    let pointer = PointerState {
        hovered: Some(path.clone()),
        ..PointerState::default()
    };

    let mut resolver = CursorResolver::new();
    let preload = resolver.preload_stylesheet(&browser);
    assert_eq!(preload.discovered, 2);
    assert_eq!(preload.loaded, 1);
    assert_eq!(preload.failed, 1);

    let resolved = resolver
        .resolve_for_path(&browser, &path, 800.0, &pointer)
        .unwrap();
    match resolved {
        ResolvedCursor::Image {
            cursor,
            source,
            fallback,
        } => {
            assert_eq!(source.scheme(), "data");
            assert_eq!(cursor.hotspot(), (1, 1));
            assert_eq!(fallback, CursorIcon::Crosshair);
            assert_eq!((cursor.image.width, cursor.image.height), (2, 2));
        }
        ResolvedCursor::System(icon) => panic!("expected image cursor, got {icon:?}"),
    }
}

#[test]
fn all_failed_candidates_use_the_authored_keyword_fallback() {
    let browser = browser_with(
        r#"<style>
             #target { cursor: url(missing-a.cur), url(missing-b.png) 4 5, crosshair; }
           </style>
           <div id="target">target</div>"#
            .to_string(),
    );
    let path = dom_api::get_element_by_id(&browser.document().dom, "target").unwrap();
    let pointer = PointerState {
        hovered: Some(path.clone()),
        ..PointerState::default()
    };
    let mut resolver = CursorResolver::new();
    let resolved = resolver
        .resolve_for_path(&browser, &path, 800.0, &pointer)
        .unwrap();
    assert!(matches!(
        resolved,
        ResolvedCursor::System(CursorIcon::Crosshair)
    ));
}

#[test]
fn inline_style_cursor_lists_are_preserved_and_resolved() {
    let browser = browser_with(format!(
        r#"<div id="target" style="cursor: url(missing.cur), url('{PNG2}') 1 0, pointer">target</div>"#
    ));
    let path = dom_api::get_element_by_id(&browser.document().dom, "target").unwrap();
    let pointer = PointerState {
        hovered: Some(path.clone()),
        ..PointerState::default()
    };
    let mut resolver = CursorResolver::new();
    let resolved = resolver
        .resolve_for_path(&browser, &path, 800.0, &pointer)
        .unwrap();
    let image = resolved.image().expect("inline candidate resolves");
    assert_eq!(image.hotspot(), (1, 0));
    assert_eq!(resolved.fallback_icon(), CursorIcon::Pointer);
}

#[test]
fn parser_keeps_data_url_comma_inside_url_function() {
    let parsed = parse_cursor_value(&format!(
        "url({PNG2}) 1 1, url(other.cur), pointer"
    ))
    .unwrap();
    assert_eq!(parsed.images.len(), 2);
    assert_eq!(parsed.images[0].reference, PNG2);
    assert_eq!(parsed.images[0].hotspot, Some((1.0, 1.0)));
    assert_eq!(parsed.fallback, CursorIcon::Pointer);
}
