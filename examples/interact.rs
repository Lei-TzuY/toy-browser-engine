//! Drive the form page without a window: focus a field, type into it, toggle a
//! checkbox, and write the result to a PPM.
//!
//! This is the headless equivalent of the interactive window, and the way to
//! see what the browser looks like mid-interaction:
//!
//! ```text
//! cargo run --example interact -- out.ppm
//! ```

use browser_engine::{
    browser::Browser,
    document::PointerState,
    input::{Key, KeyEvent, Modifiers},
    net::{DefaultLoader, Url},
    script::dom_api,
};

const VIEWPORT: (usize, usize) = (800, 700);

fn main() {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "interact.ppm".into());

    let page = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("site")
        .join("form.html");
    let mut browser = Browser::open(Box::new(DefaultLoader::new()), &Url::from_file_path(&page))
        .expect("form page loads");

    // 1. Click into the search field, as a user would.
    let query = dom_api::get_element_by_id(&browser.document().dom, "q").expect("#q");
    browser.click_node(&query);
    println!("focused: {:?}", focused_id(&browser));

    // 2. Type, then correct a typo with Backspace.
    browser.type_text("toy browserr");
    browser.press_key(&KeyEvent::new(Key::Backspace));
    println!("value: {:?}", value_of(&browser, "q"));

    // 3. Tab to the next control, then Shift+Tab back.
    browser.press_key(&KeyEvent::new(Key::Tab));
    println!("after Tab: {:?}", focused_id(&browser));
    browser.press_key(&KeyEvent::with_modifiers(Key::Tab, Modifiers::shift()));
    println!("after Shift+Tab: {:?}", focused_id(&browser));

    // 4. Toggle a checkbox with the keyboard.
    let beta = dom_api::get_element_by_id(&browser.document().dom, "beta").expect("#beta");
    browser.document_mut().focus_path(&beta);
    browser.press_key(&KeyEvent::character(' '));
    println!("beta checked: {}", is_checked(&browser, "beta"));

    // 5. Put focus back in the field so the caret shows in the render.
    browser.document_mut().focus_path(&query);
    browser.press_key(&KeyEvent::new(Key::End));

    let canvas = browser.render(VIEWPORT.0, VIEWPORT.1, 0.0, &PointerState::default());
    std::fs::write(&output, canvas.to_ppm()).expect("write ppm");
    println!("painted → {output}");

    // 6. Submit with Enter and report where the browser navigated.
    browser.press_key(&KeyEvent::new(Key::Enter));
    println!("navigated to: {}", browser.url());
    println!("history: {} entries", browser.history().len());
}

fn focused_id(browser: &Browser) -> Option<String> {
    let path = browser.document().focused_path()?;
    let element = dom_api::node_at(&browser.document().dom, &path)?.as_element()?;
    Some(element.get_attr("id").unwrap_or("(no id)").to_string())
}

fn value_of(browser: &Browser, id: &str) -> String {
    let path = dom_api::get_element_by_id(&browser.document().dom, id).expect("element");
    dom_api::node_at(&browser.document().dom, &path)
        .and_then(|n| n.as_element())
        .map(|e| e.control_value())
        .unwrap_or_default()
}

fn is_checked(browser: &Browser, id: &str) -> bool {
    let path = dom_api::get_element_by_id(&browser.document().dom, id).expect("element");
    dom_api::node_at(&browser.document().dom, &path)
        .and_then(|n| n.as_element())
        .is_some_and(|e| e.is_checked())
}
