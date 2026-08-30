use browser_engine::browser::ClickOutcome;
use browser_engine::net::Url;
use browser_engine::script::dom_api;
use browser_engine::select_state;
use browser_engine::{Browser, MemoryLoader};

fn browser_for(html: &str) -> Browser {
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///form.html", html);
    loader.insert("demo:///next", "<title>Next</title>");
    Browser::open(Box::new(loader), &Url::parse("demo:///form.html").unwrap()).unwrap()
}

#[test]
fn reset_button_restores_select_default_before_submission() {
    let html = r#"<form action="next">
        <select id="pick" name="pick">
            <option value="a" selected>A</option>
            <option value="b">B</option>
        </select>
        <button id="reset" type="reset">Reset</button>
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let pick = dom_api::get_element_by_id(&browser.document().dom, "pick").unwrap();
    let reset = dom_api::get_element_by_id(&browser.document().dom, "reset").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(select_state::set_value(
        &mut browser.document_mut().dom,
        &pick,
        "b"
    ));
    assert_eq!(
        select_state::value(&browser.document().dom, &pick).as_deref(),
        Some("b")
    );

    // The existing Document reset path only visits form controls. The select's
    // own canonical live-selection override must therefore be reset by its
    // generic control reset rather than by treating <option> as form controls.
    let _ = browser.click_node(&reset);
    assert_eq!(
        select_state::value(&browser.document().dom, &pick).as_deref(),
        Some("a")
    );
    assert_eq!(
        select_state::selected_index(&browser.document().dom, &pick),
        Some(0)
    );

    assert!(matches!(
        browser.click_node(&go),
        ClickOutcome::Navigated(_)
    ));
    assert_eq!(browser.url().to_string(), "demo:///next?pick=a");
}

#[test]
fn script_form_reset_restores_select_default_through_pending_action() {
    let html = r#"<form id="f" action="next">
        <select id="pick" name="pick">
            <option value="a">A</option>
            <option value="b">B</option>
        </select>
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let pick = dom_api::get_element_by_id(&browser.document().dom, "pick").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(select_state::set_value(
        &mut browser.document_mut().dom,
        &pick,
        "b"
    ));
    assert_eq!(
        select_state::value(&browser.document().dom, &pick).as_deref(),
        Some("b")
    );

    {
        let document = browser.document_mut();
        let (runtime, dom) = (&mut document.runtime, &mut document.dom);
        runtime.run_script(dom, r#"document.getElementById("f").reset();"#);
        document.apply_pending_actions();
    }

    // With no selected attribute, resetting returns the single select to its
    // pristine state, so the first enabled option becomes selected again.
    assert_eq!(
        select_state::value(&browser.document().dom, &pick).as_deref(),
        Some("a")
    );
    assert_eq!(
        select_state::selected_index(&browser.document().dom, &pick),
        Some(0)
    );

    assert!(matches!(
        browser.click_node(&go),
        ClickOutcome::Navigated(_)
    ));
    assert_eq!(browser.url().to_string(), "demo:///next?pick=a");
}

#[test]
fn reset_revalidates_required_select_from_its_default_state() {
    let html = r#"<form action="next">
        <select id="pick" name="pick" required>
            <option value="a" selected>A</option>
            <option value="b">B</option>
        </select>
        <button id="reset" type="reset">Reset</button>
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let pick = dom_api::get_element_by_id(&browser.document().dom, "pick").unwrap();
    let reset = dom_api::get_element_by_id(&browser.document().dom, "reset").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(select_state::set_value(
        &mut browser.document_mut().dom,
        &pick,
        "missing"
    ));
    assert_eq!(
        select_state::selected_index(&browser.document().dom, &pick),
        Some(-1)
    );
    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");

    let _ = browser.click_node(&reset);
    assert_eq!(
        select_state::value(&browser.document().dom, &pick).as_deref(),
        Some("a")
    );
    assert!(matches!(
        browser.click_node(&go),
        ClickOutcome::Navigated(_)
    ));
    assert_eq!(browser.url().to_string(), "demo:///next?pick=a");
}
