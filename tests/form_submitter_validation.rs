use browser_engine::forms::submission_skips_validation;
use browser_engine::html::parse_html;
use browser_engine::script::dom_api;

#[test]
fn form_novalidate_applies_without_a_submitter() {
    let dom = parse_html(r#"<form id="f" novalidate><input required></form>"#);
    let form = dom_api::get_element_by_id(&dom, "f").unwrap();
    assert!(submission_skips_validation(&dom, &form, None));
}
