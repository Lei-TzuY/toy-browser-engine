// ============================================================
//  forms.rs  —  Focusability, tab order and form submission
// ============================================================
//
//  The rules here are pure functions over the DOM: which elements can take
//  focus and in what order, which controls are "successful" (submittable),
//  and how a form serialises into a URL. Keeping them separate from the
//  event plumbing makes each testable on its own.

use crate::dom::{ElementData, Node, NodeType};
use crate::net::Url;
use crate::script::dom_api::{self, NodePath};

// ── Focusability ──────────────────────────────────────────────────────────────

/// `tabindex`, if the attribute parses as an integer.
fn tab_index(element: &ElementData) -> Option<i32> {
    element.get_attr("tabindex")?.trim().parse::<i32>().ok()
}

/// True when the element can hold focus at all (including programmatically).
pub fn is_focusable(element: &ElementData) -> bool {
    if element.is_disabled() {
        return false;
    }
    match element.tag_name.as_str() {
        "input" => element.input_type() != "hidden",
        "textarea" | "select" | "button" => true,
        "a" => element.get_attr("href").is_some(),
        _ => tab_index(element).is_some(),
    }
}

/// True when Tab should stop on the element (`tabindex="-1"` is skipped).
pub fn is_tabbable(element: &ElementData) -> bool {
    is_focusable(element) && tab_index(element).is_none_or(|index| index >= 0)
}

/// Every tabbable element, in tab order.
///
/// Positive `tabindex` values come first in ascending order, then everything
/// else in document order — the HTML sequential focus navigation order.
pub fn tab_order(dom: &Node) -> Vec<NodePath> {
    let mut positive: Vec<(i32, usize, NodePath)> = Vec::new();
    let mut natural: Vec<NodePath> = Vec::new();

    for (order, path) in focusable_candidates(dom).into_iter().enumerate() {
        let Some(element) = dom_api::node_at(dom, &path).and_then(|n| n.as_element()) else {
            continue;
        };
        if !is_tabbable(element) {
            continue;
        }
        match tab_index(element) {
            Some(index) if index > 0 => positive.push((index, order, path)),
            _ => natural.push(path),
        }
    }

    positive.sort_by_key(|(index, order, _)| (*index, *order));
    positive
        .into_iter()
        .map(|(_, _, path)| path)
        .chain(natural)
        .collect()
}

/// Every element in the document, in document order (candidates for focus).
fn focusable_candidates(dom: &Node) -> Vec<NodePath> {
    let mut out = Vec::new();
    walk(dom, &mut Vec::new(), &mut out);
    return out;

    fn walk(node: &Node, path: &mut NodePath, out: &mut Vec<NodePath>) {
        if node.as_element().is_some() {
            out.push(path.clone());
        }
        for (index, child) in node.children.iter().enumerate() {
            path.push(index);
            walk(child, path, out);
            path.pop();
        }
    }
}

// ── Form association ──────────────────────────────────────────────────────────

/// The nearest ancestor `<form>` of `path`.
pub fn owning_form(dom: &Node, path: &[usize]) -> Option<NodePath> {
    dom_api::ancestor_paths(path).into_iter().find(|candidate| {
        dom_api::node_at(dom, candidate)
            .and_then(|n| n.as_element())
            .is_some_and(|e| e.tag_name == "form")
    })
}

/// Every form control inside `form_path`, in document order.
pub fn form_controls(dom: &Node, form_path: &[usize]) -> Vec<NodePath> {
    let mut out = Vec::new();
    let Some(form) = dom_api::node_at(dom, form_path) else {
        return out;
    };
    walk(form, &mut form_path.to_vec(), &mut out);
    return out;

    fn walk(node: &Node, path: &mut NodePath, out: &mut Vec<NodePath>) {
        if let NodeType::Element(element) = &node.node_type {
            if element.is_form_control() {
                out.push(path.clone());
            }
        }
        for (index, child) in node.children.iter().enumerate() {
            path.push(index);
            walk(child, path, out);
            path.pop();
        }
    }
}

/// The control a bare Enter press should submit through, if the form has a
/// single-line text field (HTML's "implicit submission").
pub fn allows_implicit_submission(dom: &Node, form_path: &[usize]) -> bool {
    form_controls(dom, form_path)
        .iter()
        .filter_map(|path| dom_api::node_at(dom, path)?.as_element())
        .any(|element| element.tag_name == "input" && element.is_text_entry())
}

// ── Submission ────────────────────────────────────────────────────────────────

/// One `name=value` pair from a successful control.
pub type FormEntry = (String, String);

/// The submittable name/value pairs of a form, in document order.
///
/// A control is "successful" when it has a name, is not disabled, and — for
/// checkboxes and radios — is checked. Buttons only submit when they are the
/// control that triggered the submission, so they are never collected here.
pub fn form_data(dom: &Node, form_path: &[usize]) -> Vec<FormEntry> {
    let mut entries = Vec::new();
    for path in form_controls(dom, form_path) {
        let Some(element) = dom_api::node_at(dom, &path).and_then(|n| n.as_element()) else {
            continue;
        };
        if element.is_disabled() || element.tag_name == "button" {
            continue;
        }
        let Some(name) = element.get_attr("name").filter(|n| !n.is_empty()) else {
            continue;
        };
        if element.tag_name == "input" {
            match element.input_type().as_str() {
                // Unchecked boxes and radios are not successful.
                "checkbox" | "radio" if !element.is_checked() => continue,
                // A checked box with no value submits the string "on".
                "checkbox" | "radio" => {
                    let value = match element.get_attr("value") {
                        Some(value) => value.to_string(),
                        None => "on".to_string(),
                    };
                    entries.push((name.to_string(), value));
                    continue;
                }
                "submit" | "reset" | "button" | "image" | "file" => continue,
                _ => {}
            }
        }
        entries.push((name.to_string(), element.control_value()));
    }
    entries
}

/// Percent-encode one form field, `application/x-www-form-urlencoded` style.
pub fn encode_form_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(byte as char)
            }
            // Spaces become `+` in form encoding, not `%20`.
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Serialise entries into a query string.
pub fn encode_form_entries(entries: &[FormEntry]) -> String {
    entries
        .iter()
        .map(|(name, value)| {
            format!(
                "{}={}",
                encode_form_component(name),
                encode_form_component(value)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// How a form wants to be submitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmissionMethod {
    Get,
    /// POST is recognised but not performed: the loader only issues GETs.
    Post,
}

/// A prepared submission: where to go and what to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    pub method: SubmissionMethod,
    pub url: Url,
    pub entries: Vec<FormEntry>,
}

/// Build the navigation a form submission implies.
///
/// `action` is resolved against `base`; for GET the entries replace the
/// query string, exactly as the HTML spec's "mutate action URL" step does.
pub fn prepare_submission(dom: &Node, form_path: &[usize], base: &Url) -> Option<Submission> {
    let form = dom_api::node_at(dom, form_path)?.as_element()?;
    let method = match form
        .get_attr("method")
        .map(|m| m.trim().to_ascii_lowercase())
    {
        Some(m) if m == "post" => SubmissionMethod::Post,
        _ => SubmissionMethod::Get,
    };

    let action = form.get_attr("action").unwrap_or("").trim().to_string();
    let target = if action.is_empty() {
        // An empty action submits back to the current document.
        base.clone()
    } else {
        base.join(&action).ok()?
    };

    let entries = form_data(dom, form_path);
    let url = match method {
        SubmissionMethod::Get => {
            let query = encode_form_entries(&entries);
            let mut without_query = target.to_string();
            if let Some(index) = without_query.find(['?', '#']) {
                without_query.truncate(index);
            }
            let combined = if query.is_empty() {
                without_query
            } else {
                format!("{without_query}?{query}")
            };
            Url::parse(&combined).ok()?
        }
        SubmissionMethod::Post => target,
    };

    Some(Submission {
        method,
        url,
        entries,
    })
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::parse_html;

    fn element<'a>(dom: &'a Node, path: &[usize]) -> &'a ElementData {
        dom_api::node_at(dom, path).unwrap().as_element().unwrap()
    }

    fn tags_in_tab_order(html: &str) -> Vec<String> {
        let dom = parse_html(html);
        tab_order(&dom)
            .iter()
            .map(|path| {
                let e = element(&dom, path);
                match e.get_attr("id") {
                    Some(id) => id.to_string(),
                    None => e.tag_name.clone(),
                }
            })
            .collect()
    }

    #[test]
    fn form_controls_and_links_are_focusable() {
        let dom = parse_html(
            r#"<input><textarea></textarea><button>b</button><select></select>
               <a href="x">link</a><a>no href</a><div></div><div tabindex="0"></div>"#,
        );
        let focusable: Vec<String> = focusable_candidates(&dom)
            .iter()
            .filter(|p| is_focusable(element(&dom, p)))
            .map(|p| element(&dom, p).tag_name.clone())
            .collect();
        assert_eq!(
            focusable,
            vec!["input", "textarea", "button", "select", "a", "div"]
        );
    }

    #[test]
    fn disabled_and_hidden_controls_are_not_focusable() {
        let dom = parse_html(r#"<input disabled><input type="hidden"><input>"#);
        let paths = focusable_candidates(&dom);
        assert!(!is_focusable(element(&dom, &paths[0])));
        assert!(!is_focusable(element(&dom, &paths[1])));
        assert!(is_focusable(element(&dom, &paths[2])));
    }

    #[test]
    fn tab_order_follows_the_document() {
        assert_eq!(
            tags_in_tab_order(
                r##"<input id="a"><button id="b">x</button><a id="c" href="#">l</a>"##
            ),
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn positive_tabindex_comes_first_in_ascending_order() {
        assert_eq!(
            tags_in_tab_order(
                r#"<input id="natural"><input id="second" tabindex="2"><input id="first" tabindex="1">"#
            ),
            vec!["first", "second", "natural"]
        );
    }

    #[test]
    fn negative_tabindex_is_focusable_but_not_tabbable() {
        let dom = parse_html(r#"<div id="d" tabindex="-1"></div>"#);
        let path = dom_api::get_element_by_id(&dom, "d").unwrap();
        assert!(is_focusable(element(&dom, &path)));
        assert!(!is_tabbable(element(&dom, &path)));
        assert!(tab_order(&dom).is_empty());
    }

    // ── Submission ────────────────────────────────────────────────────────

    fn form_of(html: &str) -> (Node, NodePath) {
        let dom = parse_html(html);
        let path = dom_api::query_selector(&dom, &[], "form").expect("form");
        (dom, path)
    }

    #[test]
    fn collects_named_controls_in_document_order() {
        let (dom, form) = form_of(
            r#"<form><input name="q" value="browser"><input name="page" value="2"></form>"#,
        );
        assert_eq!(
            form_data(&dom, &form),
            vec![
                ("q".to_string(), "browser".to_string()),
                ("page".to_string(), "2".to_string())
            ]
        );
    }

    #[test]
    fn unnamed_and_disabled_controls_are_skipped() {
        let (dom, form) = form_of(
            r#"<form><input value="anon"><input name="off" value="x" disabled><input name="on" value="y"></form>"#,
        );
        assert_eq!(
            form_data(&dom, &form),
            vec![("on".to_string(), "y".to_string())]
        );
    }

    #[test]
    fn unchecked_boxes_are_not_submitted() {
        let (dom, form) = form_of(
            r#"<form>
                 <input type="checkbox" name="a" value="1" checked>
                 <input type="checkbox" name="b" value="2">
                 <input type="checkbox" name="c" checked>
               </form>"#,
        );
        assert_eq!(
            form_data(&dom, &form),
            vec![
                ("a".to_string(), "1".to_string()),
                // No value attribute: a checked box submits "on".
                ("c".to_string(), "on".to_string())
            ]
        );
    }

    #[test]
    fn live_values_are_submitted_rather_than_attributes() {
        let (mut dom, form) = form_of(r#"<form><input name="q" value="default"></form>"#);
        let path = dom_api::query_selector(&dom, &[], "input").unwrap();
        if let NodeType::Element(e) = &mut dom_api::node_at_mut(&mut dom, &path).unwrap().node_type
        {
            e.set_control_value("typed");
        }
        assert_eq!(
            form_data(&dom, &form),
            vec![("q".to_string(), "typed".to_string())]
        );
    }

    #[test]
    fn form_encoding_escapes_reserved_characters() {
        assert_eq!(encode_form_component("a b"), "a+b");
        assert_eq!(encode_form_component("a&b=c"), "a%26b%3Dc");
        assert_eq!(encode_form_component("ü"), "%C3%BC");
        assert_eq!(encode_form_component("safe-._*"), "safe-._*");
    }

    #[test]
    fn get_submission_builds_a_query_string() {
        let (dom, form) = form_of(
            r#"<form action="/search" method="get"><input name="q" value="toy browser"></form>"#,
        );
        let base = Url::parse("http://example.com/dir/page.html").unwrap();
        let submission = prepare_submission(&dom, &form, &base).expect("submission");
        assert_eq!(submission.method, SubmissionMethod::Get);
        assert_eq!(
            submission.url.to_string(),
            "http://example.com/search?q=toy+browser"
        );
    }

    #[test]
    fn get_submission_replaces_an_existing_query() {
        let (dom, form) = form_of(r#"<form action="s?old=1"><input name="q" value="new"></form>"#);
        let base = Url::parse("http://example.com/").unwrap();
        let submission = prepare_submission(&dom, &form, &base).unwrap();
        assert_eq!(submission.url.to_string(), "http://example.com/s?q=new");
    }

    #[test]
    fn an_empty_action_submits_to_the_current_document() {
        let (dom, form) = form_of(r#"<form><input name="q" value="x"></form>"#);
        let base = Url::parse("http://example.com/a/b.html").unwrap();
        let submission = prepare_submission(&dom, &form, &base).unwrap();
        assert_eq!(
            submission.url.to_string(),
            "http://example.com/a/b.html?q=x"
        );
    }

    #[test]
    fn post_is_recognised_but_kept_distinct() {
        let (dom, form) =
            form_of(r#"<form action="/p" method="POST"><input name="q" value="v"></form>"#);
        let base = Url::parse("http://example.com/").unwrap();
        let submission = prepare_submission(&dom, &form, &base).unwrap();
        assert_eq!(submission.method, SubmissionMethod::Post);
        // The query is not appended for POST.
        assert_eq!(submission.url.to_string(), "http://example.com/p");
    }

    #[test]
    fn implicit_submission_needs_a_single_line_text_field() {
        let (dom, form) = form_of(r#"<form><input name="q"></form>"#);
        assert!(allows_implicit_submission(&dom, &form));

        let (dom, form) = form_of(r#"<form><textarea name="t"></textarea></form>"#);
        assert!(!allows_implicit_submission(&dom, &form));
    }

    #[test]
    fn controls_find_their_owning_form() {
        let dom = parse_html(r#"<form id="f"><div><input id="i"></div></form><input id="loose">"#);
        let inside = dom_api::get_element_by_id(&dom, "i").unwrap();
        let outside = dom_api::get_element_by_id(&dom, "loose").unwrap();
        assert!(owning_form(&dom, &inside).is_some());
        assert!(owning_form(&dom, &outside).is_none());
    }
}
