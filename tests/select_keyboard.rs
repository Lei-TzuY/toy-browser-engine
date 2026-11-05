use browser_engine::browser::ClickOutcome;
use browser_engine::input::{Key, KeyEvent, Modifiers};
use browser_engine::net::Url;
use browser_engine::script::dom_api;
use browser_engine::select_state;
use browser_engine::{Browser, MemoryLoader};

fn browser(html: &str) -> Browser {
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///select.html", html);
    loader.insert("demo:///next", "<title>Submitted</title>");
    let mut browser = Browser::open(
        Box::new(loader),
        &Url::parse("demo:///select.html").unwrap(),
    )
    .unwrap();
    browser.document_mut().runtime.quiet = true;
    browser
}

fn path(browser: &Browser, id: &str) -> Vec<usize> {
    dom_api::get_element_by_id(&browser.document().dom, id).unwrap()
}

fn value(browser: &Browser, id: &str) -> String {
    select_state::value(&browser.document().dom, &path(browser, id)).unwrap()
}

#[test]
fn actual_control_hit_keyboard_events_and_form_submission_share_selection() {
    let mut browser = browser(
        r#"
        <style>html,body{margin:0;padding:0}select{display:block;width:240px;height:52px}</style>
        <form action="next"><select id="pick" name="pick">
          <option value="a">Alpha</option><option value="b">Beta</option>
        </select><button id="go">Submit</button></form>
        <script>
          const pick = document.getElementById("pick");
          pick.addEventListener("input", function(){console.log("input:" + pick.value);});
          pick.addEventListener("change", function(){console.log("change:" + pick.value);});
        </script>"#,
    );
    browser.click_at(60.0, 26.0, 800.0);
    assert_eq!(
        browser.document().focused_path(),
        Some(path(&browser, "pick"))
    );
    browser.press_key(&KeyEvent::new(Key::ArrowDown));
    browser.press_key(&KeyEvent::new(Key::Enter));
    assert_eq!(value(&browser, "pick"), "b");
    assert_eq!(browser.document().runtime.console, ["input:b", "change:b"]);
    let submit = path(&browser, "go");
    assert!(matches!(
        browser.click_node(&submit),
        ClickOutcome::Navigated(_)
    ));
    assert_eq!(browser.url().to_string(), "demo:///next?pick=b");
}

#[test]
fn keyboard_skips_disabled_options_and_groups_and_stops_at_the_ends() {
    let mut browser = browser(
        r#"<select id="pick">
        <option value="a">Alpha</option><option disabled value="x">Disabled</option>
        <optgroup disabled><option value="y">Disabled group</option></optgroup>
        <option value="b">Beta</option><option value="c">Gamma</option>
        </select><script>
        const pick = document.getElementById("pick");
        pick.addEventListener("change",function(){console.log(pick.value);});</script>"#,
    );
    let select = path(&browser, "pick");
    browser.document_mut().focus_path(&select);
    for (key, expected) in [
        (Key::ArrowDown, "b"),
        (Key::ArrowDown, "c"),
        (Key::ArrowDown, "c"),
        (Key::Home, "a"),
        (Key::ArrowUp, "a"),
        (Key::End, "c"),
        (Key::ArrowUp, "b"),
    ] {
        browser.press_key(&KeyEvent::new(key));
        assert_eq!(value(&browser, "pick"), expected);
    }
    assert_eq!(
        browser.document().runtime.console,
        ["b", "c", "a", "c", "b"]
    );
}

#[test]
fn cancelled_modified_disabled_and_listbox_keys_do_not_change_selection() {
    let mut browser = browser(
        r#"<select id="pick"><option>A</option><option>B</option></select>
        <select id="off" disabled><option>A</option><option>B</option></select>
        <select id="multi" multiple><option selected>A</option><option>B</option></select>
        <select id="list" size="3"><option>A</option><option>B</option></select>
        <script>document.getElementById("pick").addEventListener("keydown",function(e){
          if(e.key === "ArrowDown"){e.preventDefault();}
        });</script>"#,
    );
    let select = path(&browser, "pick");
    browser.document_mut().focus_path(&select);
    browser.press_key(&KeyEvent::new(Key::ArrowDown));
    browser.press_key(&KeyEvent::with_modifiers(Key::End, Modifiers::ctrl()));
    assert_eq!(value(&browser, "pick"), "A");
    let disabled = path(&browser, "off");
    assert!(!browser.document_mut().focus_path(&disabled));
    assert_eq!(value(&browser, "off"), "A");
    for id in ["multi", "list"] {
        let select = path(&browser, id);
        browser.document_mut().focus_path(&select);
        browser.press_key(&KeyEvent::new(Key::ArrowDown));
        assert_eq!(value(&browser, id), "A");
    }
}

#[test]
fn script_value_assignment_uses_the_same_state_without_firing_user_events() {
    let browser = browser(
        r#"<form action="next"><select id="pick" name="pick">
        <option value="a">Alpha</option><option value="b">Beta</option></select></form>
        <script>
          const pick = document.getElementById("pick");
          pick.addEventListener("change",function(){console.log("unexpected change");});
          pick.value = "b";
          console.log(pick.value);
        </script>"#,
    );
    assert_eq!(value(&browser, "pick"), "b");
    assert_eq!(browser.document().runtime.console, ["b"]);
}

#[test]
fn input_listener_removal_cannot_send_change_to_the_next_control() {
    let mut browser = browser(
        r#"<select id="pick"><option>A</option><option>B</option></select><select id="other"><option>A</option><option>B</option></select><script>
          const pick = document.getElementById("pick");
          pick.addEventListener("input",function(){pick.remove();console.log("removed");});
          document.getElementById("other").addEventListener("change",function(){console.log("wrong target");});
        </script>"#,
    );
    let select = path(&browser, "pick");
    browser.document_mut().focus_path(&select);
    browser.press_key(&KeyEvent::new(Key::ArrowDown));
    assert_eq!(browser.document().runtime.console, ["removed"]);
    assert_eq!(value(&browser, "other"), "A");
}

#[test]
fn keydown_listener_removal_cannot_change_the_following_select() {
    let mut browser = browser(
        r#"<select id="a"><option>A</option><option>B</option></select><select id="b"><option>A</option><option>B</option></select><script>
          const a = document.getElementById("a");
          const b = document.getElementById("b");
          a.addEventListener("keydown",function(){a.remove();console.log("removed");});
          b.addEventListener("input",function(){console.log("WRONG input " + b.value);});
          b.addEventListener("change",function(){console.log("WRONG change " + b.value);});
        </script>"#,
    );
    let select = path(&browser, "a");
    browser.document_mut().focus_path(&select);
    browser.press_key(&KeyEvent::new(Key::ArrowDown));
    assert_eq!(value(&browser, "b"), "A");
    assert_eq!(browser.document().runtime.console, ["removed"]);
}
