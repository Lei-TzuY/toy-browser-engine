use browser_engine::forms::{
    implicit_submitter, prepare_submission_with_submitter, submission_skips_validation,
    SubmissionMethod,
};
use browser_engine::html::parse_html;
use browser_engine::net::Url;
use browser_engine::script::dom_api;

#[test]
fn submitter_overrides_destination_method_and_contributes_its_value() {
    let dom = parse_html(
        r#"<form id="f" action="/save" method="post">
             <input name="title" value="Toy Browser">
             <button id="preview" name="intent" value="preview"
                     formaction="/preview" formmethod="get">Preview</button>
           </form>"#,
    );
    let form = dom_api::get_element_by_id(&dom, "f").unwrap();
    let preview = dom_api::get_element_by_id(&dom, "preview").unwrap();
    let base = Url::parse("http://example.test/editor").unwrap();

    let submission =
        prepare_submission_with_submitter(&dom, &form, Some(&preview), &base).unwrap();

    assert_eq!(submission.method, SubmissionMethod::Get);
    assert_eq!(
        submission.url.to_string(),
        "http://example.test/preview?title=Toy+Browser&intent=preview"
    );
}

#[test]
fn implicit_submitter_and_validation_override_are_explicit() {
    let dom = parse_html(
        r#"<form id="f">
             <input name="q" required>
             <button disabled>disabled</button>
             <button id="draft" formnovalidate>Save draft</button>
             <button id="publish">Publish</button>
           </form>"#,
    );
    let form = dom_api::get_element_by_id(&dom, "f").unwrap();
    let draft = dom_api::get_element_by_id(&dom, "draft").unwrap();

    assert_eq!(implicit_submitter(&dom, &form), Some(draft.clone()));
    assert!(submission_skips_validation(&dom, &form, Some(&draft)));
}
