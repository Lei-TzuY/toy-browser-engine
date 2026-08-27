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

/// The form owner of a form-associated control.
///
/// A `form="id"` attribute explicitly reassociates a control, even when the
/// control is physically nested in another form. If the id does not resolve to
/// a `<form>`, the control is deliberately unowned; it does not fall back to an
/// ancestor form. Without an explicit owner, the nearest ancestor form wins.
pub fn owning_form(dom: &Node, path: &[usize]) -> Option<NodePath> {
    let element = dom_api::node_at(dom, path)?.as_element()?;
    if element.is_form_control() {
        if let Some(form_id) = element.get_attr("form") {
            if form_id.is_empty() {
                return None;
            }
            let candidate = dom_api::get_element_by_id(dom, form_id)?;
            let is_form = dom_api::node_at(dom, &candidate)
                .and_then(|node| node.as_element())
                .is_some_and(|owner| owner.tag_name == "form");
            return is_form.then_some(candidate);
        }
    }

    dom_api::ancestor_paths(path).into_iter().find(|candidate| {
        dom_api::node_at(dom, candidate)
            .and_then(|n| n.as_element())
            .is_some_and(|e| e.tag_name == "form")
    })
}

/// Every control owned by `form_path`, in whole-document order.
///
/// Scanning the document rather than only the form subtree is required for
/// controls using `form="id"`. It also means a nested control explicitly
/// reassociated to another form is excluded from its physical ancestor's
/// `elements` collection and successful form data.
pub fn form_controls(dom: &Node, form_path: &[usize]) -> Vec<NodePath> {
    let is_form = dom_api::node_at(dom, form_path)
        .and_then(|node| node.as_element())
        .is_some_and(|element| element.tag_name == "form");
    if !is_form {
        return Vec::new();
    }

    let mut out = Vec::new();
    walk(dom, &mut Vec::new(), dom, form_path, &mut out);
    return out;

    fn walk(
        node: &Node,
        path: &mut NodePath,
        dom: &Node,
        form_path: &[usize],
        out: &mut Vec<NodePath>,
    ) {
        if let NodeType::Element(element) = &node.node_type {
            if element.is_form_control()
                && owning_form(dom, path).as_deref() == Some(form_path)
            {
                out.push(path.clone());
            }
        }
        for (index, child) in node.children.iter().enumerate() {
            path.push(index);
            walk(child, path, dom, form_path, out);
            path.pop();
        }
    }
}

/// True when an element is structurally a submit button.
///
/// This deliberately ignores `disabled`: `requestSubmit(submitter)` is allowed
/// to name a disabled submit button, even though disabled controls are not
/// successful form-data controls and cannot be activated by a user click.
pub fn is_submit_button(element: &ElementData) -> bool {
    match element.tag_name.as_str() {
        "button" => element
            .get_attr("type")
            .map(|kind| kind.eq_ignore_ascii_case("submit"))
            .unwrap_or(true),
        "input" => matches!(element.input_type().as_str(), "submit" | "image"),
        _ => false,
    }
}

/// True when an element can be the submitter for normal user activation.
///
/// Image-submit activation is not wired through the pointer pipeline yet, so
/// this helper intentionally keeps the engine's existing activation subset;
/// `requestSubmit()` uses [`is_submit_button`] instead.
pub fn is_submit_control(element: &ElementData) -> bool {
    if element.is_disabled() {
        return false;
    }
    match element.tag_name.as_str() {
        "button" => element
            .get_attr("type")
            .map(|kind| kind.eq_ignore_ascii_case("submit"))
            .unwrap_or(true),
        "input" => element.input_type() == "submit",
        _ => false,
    }
}

/// Errors produced while resolving the optional `requestSubmit(submitter)`
/// argument. They map directly onto the Web API's `TypeError` and
/// `NotFoundError` DOMException branches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestSubmitterError {
    NotSubmitButton,
    NotOwnedByForm,
}

/// Validate and resolve the optional submitter used by `requestSubmit()`.
///
/// Omission means the form itself acts as the submitter and therefore returns
/// `Ok(None)`. A supplied element must first be a submit button and then have
/// this exact form as its owner; the order matters because the HTML algorithm
/// exposes different exception types for those two failures.
pub fn request_submitter<'a>(
    dom: &'a Node,
    form_path: &[usize],
    submitter: Option<&'a [usize]>,
) -> Result<Option<&'a [usize]>, RequestSubmitterError> {
    let Some(path) = submitter else {
        return Ok(None);
    };
    let Some(element) = dom_api::node_at(dom, path).and_then(|node| node.as_element()) else {
        return Err(RequestSubmitterError::NotSubmitButton);
    };
    if !is_submit_button(element) {
        return Err(RequestSubmitterError::NotSubmitButton);
    }
    if owning_form(dom, path).as_deref() != Some(form_path) {
        return Err(RequestSubmitterError::NotOwnedByForm);
    }
    Ok(Some(path))
}

/// The first enabled submit button used by implicit Enter submission.
pub fn implicit_submitter(dom: &Node, form_path: &[usize]) -> Option<NodePath> {
    form_controls(dom, form_path).into_iter().find(|path| {
        dom_api::node_at(dom, path)
            .and_then(|node| node.as_element())
            .is_some_and(is_submit_control)
    })
}

/// The control a bare Enter press should submit through, if the form has a
/// single-line text field (HTML's "implicit submission").
pub fn allows_implicit_submission(dom: &Node, form_path: &[usize]) -> bool {
    form_controls(dom, form_path)
        .iter()
        .filter_map(|path| dom_api::node_at(dom, path)?.as_element())
        .any(|element| element.tag_name == "input" && element.is_text_entry())
}

/// Whether interactive constraint validation is skipped for this submission.
///
/// `<form novalidate>` applies to every interactive submission. A submit button
/// may opt out independently with `formnovalidate`.
pub fn submission_skips_validation(
    dom: &Node,
    form_path: &[usize],
    submitter: Option<&[usize]>,
) -> bool {
    let Some(form) = dom_api::node_at(dom, form_path).and_then(|node| node.as_element()) else {
        return false;
    };
    if form.get_attr("novalidate").is_some() {
        return true;
    }
    associated_submitter(dom, form_path, submitter)
        .and_then(|path| dom_api::node_at(dom, path))
        .and_then(|node| node.as_element())
        .is_some_and(|element| element.get_attr("formnovalidate").is_some())
}

fn associated_submitter<'a>(
    dom: &'a Node,
    form_path: &[usize],
    submitter: Option<&'a [usize]>,
) -> Option<&'a [usize]> {
    let path = submitter?;
    if owning_form(dom, path).as_deref() != Some(form_path) {
        return None;
    }
    let element = dom_api::node_at(dom, path)?.as_element()?;
    is_submit_button(element).then_some(path)
}

// ── Select controls ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectOption {
    default_selected: bool,
    disabled: bool,
    value: String,
}

/// The nearest ancestor `<select>` of an option path.
pub fn owning_select(dom: &Node, option_path: &[usize]) -> Option<NodePath> {
    let option = dom_api::node_at(dom, option_path)?.as_element()?;
    if option.tag_name != "option" {
        return None;
    }
    dom_api::ancestor_paths(option_path).into_iter().find(|candidate| {
        dom_api::node_at(dom, candidate)
            .and_then(|node| node.as_element())
            .is_some_and(|element| element.tag_name == "select")
    })
}

/// Effective selected option indexes for a select.
///
/// A live override on the `<select>` wins. Otherwise selection is derived from
/// option `selected` content attributes; a pristine single-select with no such
/// attribute falls back to the first enabled option.
pub fn select_selected_indices(dom: &Node, select_path: &[usize]) -> Option<Vec<usize>> {
    let select_node = dom_api::node_at(dom, select_path)?;
    let select = select_node.as_element()?;
    if select.tag_name != "select" {
        return None;
    }
    if let Some(indices) = select.selected_indices() {
        return Some(indices.to_vec());
    }

    let mut options = Vec::new();
    collect_select_options(select_node, false, &mut options);
    if select.get_attr("multiple").is_some() {
        return Some(
            options
                .iter()
                .enumerate()
                .filter_map(|(index, option)| option.default_selected.then_some(index))
                .collect(),
        );
    }
    if let Some(index) = options.iter().rposition(|option| option.default_selected) {
        return Some(vec![index]);
    }
    Some(
        options
            .iter()
            .position(|option| !option.disabled)
            .into_iter()
            .collect(),
    )
}

/// Whether an option is effectively selected in its owning select.
pub fn option_selected(dom: &Node, option_path: &[usize]) -> Option<bool> {
    let select_path = owning_select(dom, option_path)?;
    let option_paths = select_option_paths(dom, &select_path);
    let index = option_paths.iter().position(|path| path == option_path)?;
    Some(select_selected_indices(dom, &select_path)?.contains(&index))
}

/// Values contributed by a `<select>` in submission order.
pub fn select_values(dom: &Node, select_path: &[usize]) -> Vec<String> {
    let Some(select_node) = dom_api::node_at(dom, select_path) else {
        return Vec::new();
    };
    let Some(select) = select_node.as_element() else {
        return Vec::new();
    };
    if select.tag_name != "select" {
        return Vec::new();
    }

    let mut options = Vec::new();
    collect_select_options(select_node, false, &mut options);
    let selected = select_selected_indices(dom, select_path).unwrap_or_default();
    if select.get_attr("multiple").is_some() {
        return options
            .into_iter()
            .enumerate()
            .filter(|(index, option)| selected.contains(index) && !option.disabled)
            .map(|(_, option)| option.value)
            .collect();
    }

    let Some(index) = selected.last().copied() else {
        return Vec::new();
    };
    options
        .get(index)
        .filter(|option| !option.disabled)
        .map(|option| option.value.clone())
        .into_iter()
        .collect()
}

/// Replace a select's canonical live selected option indexes.
///
/// Invalid indexes are dropped. A non-multiple select keeps only the last
/// supplied valid index. Option-local selected bits are mirrored for the future
/// `option.selected` binding, while the select-level override remains the
/// canonical state consumed by submission, validation and reset.
pub fn set_select_selected_indices(
    dom: &mut Node,
    select_path: &[usize],
    indices: Vec<usize>,
) -> bool {
    let Some(select) = dom_api::node_at(dom, select_path).and_then(|node| node.as_element()) else {
        return false;
    };
    if select.tag_name != "select" {
        return false;
    }
    let multiple = select.get_attr("multiple").is_some();
    let option_paths = select_option_paths(dom, select_path);
    let mut normalized = Vec::new();
    for index in indices {
        if index < option_paths.len() && !normalized.contains(&index) {
            normalized.push(index);
        }
    }
    if !multiple && normalized.len() > 1 {
        let last = *normalized.last().unwrap();
        normalized.clear();
        normalized.push(last);
    }

    for (index, path) in option_paths.iter().enumerate() {
        if let Some(NodeType::Element(option)) =
            dom_api::node_at_mut(dom, path).map(|node| &mut node.node_type)
        {
            option.set_selected(normalized.contains(&index));
        }
    }
    let Some(NodeType::Element(select)) =
        dom_api::node_at_mut(dom, select_path).map(|node| &mut node.node_type)
    else {
        return false;
    };
    select.set_selected_indices(normalized);
    true
}

/// Change one option's current selectedness.
///
/// Selecting an option in a non-`multiple` select clears every other option in
/// that same select. Deselecting is allowed to leave a single-select with no
/// selected option. Disabled options can still be selected programmatically.
pub fn set_option_selected(dom: &mut Node, option_path: &[usize], selected: bool) -> bool {
    let Some(select_path) = owning_select(dom, option_path) else {
        return false;
    };
    let option_paths = select_option_paths(dom, &select_path);
    let Some(index) = option_paths.iter().position(|path| path == option_path) else {
        return false;
    };
    let multiple = dom_api::node_at(dom, &select_path)
        .and_then(|node| node.as_element())
        .is_some_and(|select| select.get_attr("multiple").is_some());
    let mut indices = select_selected_indices(dom, &select_path).unwrap_or_default();
    if selected {
        if multiple {
            if !indices.contains(&index) {
                indices.push(index);
                indices.sort_unstable();
            }
        } else {
            indices.clear();
            indices.push(index);
        }
    } else {
        indices.retain(|candidate| *candidate != index);
    }
    set_select_selected_indices(dom, &select_path, indices)
}

/// Restore a select to its content-attribute/default selection state.
pub fn reset_select_selectedness(dom: &mut Node, select_path: &[usize]) -> bool {
    let is_select = dom_api::node_at(dom, select_path)
        .and_then(|node| node.as_element())
        .is_some_and(|element| element.tag_name == "select");
    if !is_select {
        return false;
    }
    for path in select_option_paths(dom, select_path) {
        if let Some(NodeType::Element(option)) =
            dom_api::node_at_mut(dom, &path).map(|node| &mut node.node_type)
        {
            option.reset_selected();
        }
    }
    if let Some(NodeType::Element(select)) =
        dom_api::node_at_mut(dom, select_path).map(|node| &mut node.node_type)
    {
        select.reset_selected_indices();
    }
    true
}

fn select_option_paths(dom: &Node, select_path: &[usize]) -> Vec<NodePath> {
    let mut out = Vec::new();
    let Some(select) = dom_api::node_at(dom, select_path) else {
        return out;
    };
    walk(select, &mut select_path.to_vec(), &mut out, true);
    return out;

    fn walk(node: &Node, path: &mut NodePath, out: &mut Vec<NodePath>, is_root: bool) {
        if let Some(element) = node.as_element() {
            if !is_root && element.tag_name == "select" {
                return;
            }
            if element.tag_name == "option" {
                out.push(path.clone());
                return;
            }
        }
        for (index, child) in node.children.iter().enumerate() {
            path.push(index);
            walk(child, path, out, false);
            path.pop();
        }
    }
}

fn collect_select_options(node: &Node, disabled_group: bool, out: &mut Vec<SelectOption>) {
    let mut descendants_disabled = disabled_group;
    if let Some(element) = node.as_element() {
        if element.tag_name == "optgroup" && element.get_attr("disabled").is_some() {
            descendants_disabled = true;
        }
        if element.tag_name == "option" {
            out.push(SelectOption {
                default_selected: element.get_attr("selected").is_some(),
                disabled: descendants_disabled || element.get_attr("disabled").is_some(),
                value: option_submission_value(node, element),
            });
            return;
        }
    }
    for child in &node.children {
        collect_select_options(child, descendants_disabled, out);
    }
}

fn option_submission_value(node: &Node, element: &ElementData) -> String {
    element
        .get_attr("value")
        .map(str::to_string)
        .unwrap_or_else(|| {
            dom_api::text_content(node)
                .split_ascii_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
}

// ── Submission ────────────────────────────────────────────────────────────────

/// One `name=value` pair from a successful control.
pub type FormEntry = (String, String);

/// The submittable name/value pairs of a form, in document order.
pub fn form_data(dom: &Node, form_path: &[usize]) -> Vec<FormEntry> {
    form_data_with_submitter(dom, form_path, None)
}

/// The successful controls for a submission, including only the submit button
/// that actually triggered it.
pub fn form_data_with_submitter(
    dom: &Node,
    form_path: &[usize],
    submitter: Option<&[usize]>,
) -> Vec<FormEntry> {
    let submitter = associated_submitter(dom, form_path, submitter);
    let mut entries = Vec::new();
    for path in form_controls(dom, form_path) {
        let Some(element) = dom_api::node_at(dom, &path).and_then(|n| n.as_element()) else {
            continue;
        };
        if element.is_disabled() {
            continue;
        }

        let is_submitter = submitter.is_some_and(|candidate| candidate == path.as_slice());
        if is_submitter {
            // Image submit buttons contribute coordinates rather than their
            // value. A scripted requestSubmit has no pointer position, so the
            // standard default coordinate is represented as 0,0.
            if element.tag_name == "input" && element.input_type() == "image" {
                match element.get_attr("name").filter(|name| !name.is_empty()) {
                    Some(name) => {
                        entries.push((format!("{name}.x"), "0".to_string()));
                        entries.push((format!("{name}.y"), "0".to_string()));
                    }
                    None => {
                        entries.push(("x".to_string(), "0".to_string()));
                        entries.push(("y".to_string(), "0".to_string()));
                    }
                }
                continue;
            }
            if let Some(name) = element.get_attr("name").filter(|name| !name.is_empty()) {
                entries.push((
                    name.to_string(),
                    element.get_attr("value").unwrap_or("").to_string(),
                ));
            }
            continue;
        }

        if element.tag_name == "button" {
            continue;
        }
        let Some(name) = element.get_attr("name").filter(|n| !n.is_empty()) else {
            continue;
        };
        if element.tag_name == "select" {
            entries.extend(
                select_values(dom, &path)
                    .into_iter()
                    .map(|value| (name.to_string(), value)),
            );
            continue;
        }
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
    Post,
}

/// A prepared submission: where to go and what to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    pub method: SubmissionMethod,
    pub url: Url,
    pub entries: Vec<FormEntry>,
}

fn parse_method(value: Option<&str>) -> SubmissionMethod {
    match value.map(|method| method.trim().to_ascii_lowercase()) {
        Some(method) if method == "post" => SubmissionMethod::Post,
        _ => SubmissionMethod::Get,
    }
}

/// Build a form submission without a distinguished submit button.
pub fn prepare_submission(dom: &Node, form_path: &[usize], base: &Url) -> Option<Submission> {
    prepare_submission_with_submitter(dom, form_path, None, base)
}

/// Build the navigation a form submission implies, honoring submitter
/// overrides (`formaction`, `formmethod`) and including its `name=value` pair.
pub fn prepare_submission_with_submitter(
    dom: &Node,
    form_path: &[usize],
    submitter: Option<&[usize]>,
    base: &Url,
) -> Option<Submission> {
    let form = dom_api::node_at(dom, form_path)?.as_element()?;
    let submitter = associated_submitter(dom, form_path, submitter);
    let submitter_element = submitter
        .and_then(|path| dom_api::node_at(dom, path))
        .and_then(|node| node.as_element());

    let method = if let Some(element) = submitter_element {
        match element.get_attr("formmethod") {
            Some(value) => parse_method(Some(value)),
            None => parse_method(form.get_attr("method")),
        }
    } else {
        parse_method(form.get_attr("method"))
    };

    let action = submitter_element
        .and_then(|element| element.get_attr("formaction"))
        .or_else(|| form.get_attr("action"))
        .unwrap_or("")
        .trim()
        .to_string();
    let target = if action.is_empty() {
        // An empty action submits back to the current document.
        base.clone()
    } else {
        base.join(&action).ok()?
    };

    let entries = form_data_with_submitter(dom, form_path, submitter);
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
                ("c".to_string(), "on".to_string())
            ]
        );
    }

    #[test]
    fn single_select_uses_last_selected_and_first_option_fallback() {
        let (dom, form) = form_of(
            r#"<form>
                 <select name="chosen">
                   <option value="a" selected>A</option>
                   <option value="b" selected>B</option>
                   <option value="c">C</option>
                 </select>
                 <select name="fallback">
                   <option disabled value="x">X</option>
                   <option value="y">Y</option>
                 </select>
               </form>"#,
        );
        assert_eq!(
            form_data(&dom, &form),
            vec![("chosen".into(), "b".into()), ("fallback".into(), "y".into())]
        );
    }

    #[test]
    fn multiple_select_submits_selected_enabled_options_in_order() {
        let (dom, form) = form_of(
            r#"<form><select name="tag" multiple>
                 <option selected value="rust">Rust</option>
                 <option selected disabled value="hidden">Hidden</option>
                 <optgroup disabled><option selected value="grouped">Grouped</option></optgroup>
                 <option selected>  Toy   Browser  </option>
               </select></form>"#,
        );
        assert_eq!(
            form_data(&dom, &form),
            vec![("tag".into(), "rust".into()), ("tag".into(), "Toy Browser".into())]
        );
    }

    #[test]
    fn selected_disabled_option_does_not_fall_back_in_single_select() {
        let (dom, form) = form_of(
            r#"<form><select name="pick"><option selected disabled value="x">X</option><option value="y">Y</option></select></form>"#,
        );
        assert!(form_data(&dom, &form).is_empty());
    }

    #[test]
    fn live_single_select_selection_is_exclusive_and_serialized() {
        let (mut dom, form) = form_of(
            r#"<form><select id="pick" name="pick"><option id="a" value="a" selected>A</option><option id="b" value="b">B</option></select></form>"#,
        );
        let select = dom_api::get_element_by_id(&dom, "pick").unwrap();
        let a = dom_api::get_element_by_id(&dom, "a").unwrap();
        let b = dom_api::get_element_by_id(&dom, "b").unwrap();

        assert!(set_option_selected(&mut dom, &b, true));
        assert_eq!(select_values(&dom, &select), vec!["b"]);
        assert!(!element(&dom, &a).is_selected());
        assert!(element(&dom, &b).is_selected());
        assert_eq!(form_data(&dom, &form), vec![("pick".into(), "b".into())]);
    }

    #[test]
    fn live_multiple_select_keeps_several_selected_options() {
        let (mut dom, form) = form_of(
            r#"<form><select id="tags" name="tag" multiple><option id="a" value="a">A</option><option id="b" value="b">B</option></select></form>"#,
        );
        let a = dom_api::get_element_by_id(&dom, "a").unwrap();
        let b = dom_api::get_element_by_id(&dom, "b").unwrap();
        assert!(set_option_selected(&mut dom, &a, true));
        assert!(set_option_selected(&mut dom, &b, true));
        assert_eq!(
            form_data(&dom, &form),
            vec![("tag".into(), "a".into()), ("tag".into(), "b".into())]
        );
    }

    #[test]
    fn explicit_live_deselection_suppresses_single_select_fallback() {
        let (mut dom, form) = form_of(
            r#"<form><select id="pick" name="pick"><option id="a" value="a">A</option><option value="b">B</option></select></form>"#,
        );
        let select = dom_api::get_element_by_id(&dom, "pick").unwrap();
        let a = dom_api::get_element_by_id(&dom, "a").unwrap();
        assert_eq!(select_values(&dom, &select), vec!["a"]);
        assert!(set_option_selected(&mut dom, &a, false));
        assert!(select_values(&dom, &select).is_empty());
        assert!(form_data(&dom, &form).is_empty());
    }

    #[test]
    fn resetting_select_selectedness_restores_attribute_defaults() {
        let (mut dom, _) = form_of(
            r#"<form><select id="pick"><option id="a" value="a" selected>A</option><option id="b" value="b">B</option></select></form>"#,
        );
        let select = dom_api::get_element_by_id(&dom, "pick").unwrap();
        let b = dom_api::get_element_by_id(&dom, "b").unwrap();
        assert!(set_option_selected(&mut dom, &b, true));
        assert_eq!(select_values(&dom, &select), vec!["b"]);
        assert!(reset_select_selectedness(&mut dom, &select));
        assert_eq!(select_values(&dom, &select), vec!["a"]);
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
    fn clicked_submitter_is_included_in_document_order() {
        let (dom, form) = form_of(
            r#"<form><input name="before" value="1"><button id="save" name="intent" value="save">Save</button><input name="after" value="2"><button id="other" name="intent" value="other">Other</button></form>"#,
        );
        let save = dom_api::get_element_by_id(&dom, "save").unwrap();
        assert_eq!(
            form_data_with_submitter(&dom, &form, Some(&save)),
            vec![
                ("before".into(), "1".into()),
                ("intent".into(), "save".into()),
                ("after".into(), "2".into()),
            ]
        );
    }

    #[test]
    fn only_a_real_submitter_is_included() {
        let (dom, form) = form_of(
            r#"<form><input id="q" name="q" value="v"><button id="reset" type="reset" name="intent" value="reset">Reset</button></form>"#,
        );
        let reset = dom_api::get_element_by_id(&dom, "reset").unwrap();
        assert_eq!(
            form_data_with_submitter(&dom, &form, Some(&reset)),
            vec![("q".into(), "v".into())]
        );
    }

    #[test]
    fn request_submitter_distinguishes_type_and_ownership_errors() {
        let dom = parse_html(
            r#"<form id="a"><button id="ok">Go</button><button id="plain" type="button">Plain</button></form>
               <form id="b"><input id="foreign" type="submit"></form>"#,
        );
        let form = dom_api::get_element_by_id(&dom, "a").unwrap();
        let ok = dom_api::get_element_by_id(&dom, "ok").unwrap();
        let plain = dom_api::get_element_by_id(&dom, "plain").unwrap();
        let foreign = dom_api::get_element_by_id(&dom, "foreign").unwrap();

        assert_eq!(request_submitter(&dom, &form, None), Ok(None));
        assert_eq!(request_submitter(&dom, &form, Some(&ok)), Ok(Some(ok.as_slice())));
        assert_eq!(
            request_submitter(&dom, &form, Some(&plain)),
            Err(RequestSubmitterError::NotSubmitButton)
        );
        assert_eq!(
            request_submitter(&dom, &form, Some(&foreign)),
            Err(RequestSubmitterError::NotOwnedByForm)
        );
    }

    #[test]
    fn request_submitter_accepts_disabled_and_image_submit_buttons() {
        let (dom, form) = form_of(
            r#"<form><button id="disabled" disabled>Go</button><input id="image" type="image" name="spot"></form>"#,
        );
        let disabled = dom_api::get_element_by_id(&dom, "disabled").unwrap();
        let image = dom_api::get_element_by_id(&dom, "image").unwrap();

        assert!(request_submitter(&dom, &form, Some(&disabled)).is_ok());
        assert!(request_submitter(&dom, &form, Some(&image)).is_ok());
        assert!(!is_submit_control(element(&dom, &disabled)));
        assert!(!is_submit_control(element(&dom, &image)));
        assert!(is_submit_button(element(&dom, &disabled)));
        assert!(is_submit_button(element(&dom, &image)));
    }

    #[test]
    fn image_submitter_serializes_default_coordinates() {
        let (dom, form) = form_of(
            r#"<form><input name="q" value="v"><input id="image" type="image" name="spot" formaction="/image"></form>"#,
        );
        let image = dom_api::get_element_by_id(&dom, "image").unwrap();
        let base = Url::parse("http://example.com/form").unwrap();
        let submission =
            prepare_submission_with_submitter(&dom, &form, Some(&image), &base).unwrap();
        assert_eq!(submission.url.to_string(), "http://example.com/image?q=v&spot.x=0&spot.y=0");
    }

    #[test]
    fn disabled_request_submitter_keeps_overrides_but_is_not_successful_data() {
        let (dom, form) = form_of(
            r#"<form action="/default"><input name="q" value="v"><button id="draft" disabled name="intent" value="draft" formaction="/draft" formmethod="post" formnovalidate>Draft</button></form>"#,
        );
        let draft = dom_api::get_element_by_id(&dom, "draft").unwrap();
        let base = Url::parse("http://example.com/form").unwrap();
        assert!(submission_skips_validation(&dom, &form, Some(&draft)));
        let submission =
            prepare_submission_with_submitter(&dom, &form, Some(&draft), &base).unwrap();
        assert_eq!(submission.method, SubmissionMethod::Post);
        assert_eq!(submission.url.to_string(), "http://example.com/draft");
        assert_eq!(submission.entries, vec![("q".into(), "v".into())]);
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
        assert_eq!(submission.url.to_string(), "http://example.com/p");
    }

    #[test]
    fn submitter_overrides_action_method_and_payload() {
        let (dom, form) = form_of(
            r#"<form action="/default" method="get"><input name="q" value="v"><button id="publish" name="intent" value="publish" formaction="/publish" formmethod="post">Publish</button></form>"#,
        );
        let publish = dom_api::get_element_by_id(&dom, "publish").unwrap();
        let base = Url::parse("http://example.com/editor").unwrap();
        let submission =
            prepare_submission_with_submitter(&dom, &form, Some(&publish), &base).unwrap();
        assert_eq!(submission.method, SubmissionMethod::Post);
        assert_eq!(submission.url.to_string(), "http://example.com/publish");
        assert_eq!(
            submission.entries,
            vec![("q".into(), "v".into()), ("intent".into(), "publish".into())]
        );
    }

    #[test]
    fn submitter_get_override_uses_its_own_query_payload() {
        let (dom, form) = form_of(
            r#"<form action="/save" method="post"><input name="q" value="v"><input id="preview" type="submit" name="mode" value="preview" formaction="/preview?old=1" formmethod="get"></form>"#,
        );
        let preview = dom_api::get_element_by_id(&dom, "preview").unwrap();
        let base = Url::parse("http://example.com/editor").unwrap();
        let submission =
            prepare_submission_with_submitter(&dom, &form, Some(&preview), &base).unwrap();
        assert_eq!(submission.method, SubmissionMethod::Get);
        assert_eq!(
            submission.url.to_string(),
            "http://example.com/preview?q=v&mode=preview"
        );
    }

    #[test]
    fn implicit_submitter_is_the_first_enabled_submit_button() {
        let (dom, form) = form_of(
            r#"<form><input name="q"><button disabled>Disabled</button><button id="first" name="go" value="1">First</button><input id="second" type="submit" name="go" value="2"></form>"#,
        );
        let first = dom_api::get_element_by_id(&dom, "first").unwrap();
        assert_eq!(implicit_submitter(&dom, &form), Some(first));
    }

    #[test]
    fn novalidate_and_formnovalidate_skip_interactive_validation() {
        let (dom, form) = form_of(r#"<form novalidate><input required><button id="go">Go</button></form>"#);
        let go = dom_api::get_element_by_id(&dom, "go").unwrap();
        assert!(submission_skips_validation(&dom, &form, Some(&go)));

        let (dom, form) = form_of(
            r#"<form><input required><button id="normal">Normal</button><button id="draft" formnovalidate>Draft</button></form>"#,
        );
        let normal = dom_api::get_element_by_id(&dom, "normal").unwrap();
        let draft = dom_api::get_element_by_id(&dom, "draft").unwrap();
        assert!(!submission_skips_validation(&dom, &form, Some(&normal)));
        assert!(submission_skips_validation(&dom, &form, Some(&draft)));
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
