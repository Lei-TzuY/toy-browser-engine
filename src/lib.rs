//! browser_engine — A minimal browser engine built from scratch in Rust.
//!
//! Pipeline:
//!  HTML string
//!   → `html::parse_html`      → DOM tree  (`dom::Node`)
//!   → `style::style_tree`     → Styled tree (`style::StyledNode`)
//!   → `layout::layout_tree`   → Layout tree (`layout::LayoutBox`)
//!   → `paint::paint`          → Pixel canvas (`paint::Canvas`) → PPM file

pub mod animation;
pub mod audio;
#[path = "browser_cookie_session_final.rs"]
pub mod browser;
#[allow(dead_code)]
#[path = "browser.rs"]
mod browser_prev;
pub mod canvas;
pub mod cookie;
pub mod cookie_network;
pub mod cookie_same_site;
pub mod cors_settings;
pub mod css;
pub mod document;
pub mod document_referrer;
pub mod dom;
pub mod editing;
pub mod eventloop;
pub(crate) mod fetch_cors;
pub(crate) mod fetch_cors_preflight;
pub(crate) mod fetch_cors_redirect;
pub(crate) mod fetch_integrity;
pub mod fetch_redirect_policy;
pub mod form_state;
pub mod forms;
pub mod hsts;
pub mod hsts_network;
pub mod html;
pub mod html_subresource_integrity;
pub mod hyperlink_referrer;
pub mod image;
pub mod input;
pub mod integrity_policy;
pub mod integrity_policy_headers;
pub mod integrity_policy_reporting;
pub mod integrity_report_queue;
pub mod layout;
#[path = "navigation_network_with_credentials.rs"]
pub mod navigation_network;
pub mod net;
pub mod paint;
pub mod redirect_policy;
pub mod referrer_meta;
pub mod referrer_policy;
pub mod script;
pub mod select_state;
pub mod session_network;
mod session_redirect;
pub mod style;
pub mod subresource_cors;
pub mod subresource_cors_credentials;
pub mod subresource_integrity_policy;
pub mod subresource_referrer;
pub mod svg;
pub mod text;
pub mod transition;
pub mod validation;

pub use animation::AnimationManager;
pub use browser::Browser;
pub use cookie_network::{CookieJarRef, CookieNetwork};
pub use cookie_same_site::{cookie_allows_request, same_site_allows, SameSiteRequestContext};
pub use cors_settings::{
    cors_enabled, parse_cors_settings_attribute, CorsCredentialsMode, CorsSettingsAttribute,
};
pub use document::{Document, PointerState};
pub use document_referrer::DocumentReferrerContext;
pub use fetch_redirect_policy::FetchRedirectMode;
pub use hsts::{HstsCache, HstsPolicy};
pub use hsts_network::{HstsCacheRef, HstsNetwork};
pub use html::extract_inline_styles;
pub use html_subresource_integrity::{
    fetch_html_subresource_with_integrity, fetch_html_subresource_with_integrity_reporting,
    HtmlSubresourceIntegrityError, HtmlSubresourceIntegrityResult,
};
pub use hyperlink_referrer::{
    hyperlink_referrer_policy, parse_referrer_policy_attribute, rel_has_noreferrer,
};
pub use integrity_policy::{
    evaluate_integrity_policy, IntegrityPolicy, IntegrityPolicyDecision, IntegrityPolicyDestination,
    IntegrityPolicyRequestMode, IntegrityPolicySource,
};
pub use integrity_policy_headers::{
    IntegrityPolicyContainer, INTEGRITY_POLICY_HEADER, INTEGRITY_POLICY_REPORT_ONLY_HEADER,
};
pub use integrity_policy_reporting::{
    build_integrity_violation_reports, IntegrityViolationReport, IntegrityViolationReportBody,
    INTEGRITY_VIOLATION_REPORT_TYPE,
};
pub use integrity_report_queue::IntegrityReportQueue;
pub use navigation_network::NavigationNetwork;
pub use net::{MemoryLoader, ResourceLoader, Url};
pub use redirect_policy::{RedirectError, RedirectPlanner, FETCH_MAX_REDIRECTS};
pub use referrer_meta::apply_meta_referrer_policies;
pub use referrer_policy::{RedirectReferrerState, ReferrerPolicy};
pub use session_network::SessionNetwork;
pub use subresource_cors::validate_subresource_cors_response;
pub use subresource_integrity_policy::{
    enforce_subresource_integrity, evaluate_subresource_integrity_policy,
    integrity_metadata_has_supported_expression, SubresourceIntegrityError,
    SubresourceIntegrityResult,
};
pub use subresource_referrer::{
    prepare_subresource_request, subresource_redirect_state, subresource_referrer_policy,
};
pub use transition::TransitionManager;