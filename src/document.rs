// Thin facade: keep the large, stable document implementation in one blob
// and layer session-state construction beside it. Both includes expand in this
// module, so the extensions can reuse the core's private loading helpers without
// duplicating or widening them in the public API.
include!("document_core.rs");
include!("document_cookie_session_ext.rs");
include!("document_subresource_policy_ext.rs");