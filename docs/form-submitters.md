# Form submitters

Form submission is prepared from both the owning `<form>` and, when present, the submit control that triggered it.

Supported submitter behavior:

- `<button>` defaults to `type="submit"`; `<input type="submit">` is also a submitter.
- Only the activated submitter contributes its own `name=value` pair to the form data set.
- `formaction` overrides the form's `action` for that submission.
- `formmethod` overrides the form's `method` for that submission (`get` and `post`).
- `<form novalidate>` skips interactive constraint validation for every submitter.
- `formnovalidate` skips interactive validation only for that submitter.
- Implicit Enter submission identifies the first enabled submit control as its submitter.

Programmatic `form.submit()` remains a separate path: it has no submitter, fires no `submit` event, and bypasses interactive validation.

`<input type="image">` coordinate fields and the `dialog` form method are not implemented yet.
