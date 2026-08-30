//! HTML element subresource loading with SRI and `Integrity-Policy` enforcement.
//!
//! The lower-level integrity module deliberately knows nothing about network
//! dispatch. This layer turns it into the two-phase algorithm element loaders
//! need: reject policy violations before I/O, then verify the returned bytes
//! before script or stylesheet content is consumed.

use crate::cookie_same_site::SameSiteRequestContext;
use crate::document_referrer::DocumentReferrerContext;
use crate::integrity_policy::{IntegrityPolicyDestination, IntegrityPolicyRequestMode};
use crate::integrity_policy_headers::IntegrityPolicyContainer;
use crate::integrity_report_queue::IntegrityReportQueue;
use crate::navigation_network::NavigationNetwork;
use crate::net::{FetchRequest, FetchResponse, Method, Origin, Url};
use crate::subresource_integrity_policy::{
    enforce_subresource_integrity, evaluate_subresource_integrity_policy,
    integrity_metadata_has_supported_expression, SubresourceIntegrityError,
};

/// Successful HTML subresource fetch metadata.
#[derive(Debug, Clone)]
pub struct HtmlSubresourceIntegrityResult {
    pub response: FetchResponse,
    /// The report-only policy would have rejected this request. Callers that do
    /// not use the reporting-aware helper can still observe the violation bit.
    pub report_only_violation: bool,
}

/// Failure produced by the HTML element integrity-aware loader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HtmlSubresourceIntegrityError {
    /// The enforced `Integrity-Policy` rejected the request before transport.
    PolicyBlocked,
    /// Cross-origin SRI metadata requires a CORS-enabled element request.
    CorsRequired,
    /// The network/CORS/referrer/redirect stack rejected the request.
    Network(String),
    /// The final response did not have a successful HTTP status.
    HttpStatus(u16),
    /// The returned bytes did not match the strongest supported SRI digest.
    IntegrityMismatch,
}

impl std::fmt::Display for HtmlSubresourceIntegrityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PolicyBlocked => f.write_str("Integrity-Policy blocked subresource request"),
            Self::CorsRequired => f.write_str("cross-origin SRI requires a CORS-enabled request"),
            Self::Network(message) => write!(f, "subresource fetch failed: {message}"),
            Self::HttpStatus(status) => write!(f, "subresource returned HTTP {status}"),
            Self::IntegrityMismatch => f.write_str("subresource integrity mismatch"),
        }
    }
}

impl std::error::Error for HtmlSubresourceIntegrityError {}

/// Fetch one external `script` or stylesheet `link` using the committed
/// document's policy container and element attributes.
///
/// The policy-only check happens before network dispatch. This is important for
/// `Integrity-Policy`: a blocked request must not become an information leak by
/// issuing the request and discarding the response afterwards. When supported
/// SRI metadata is present on a cross-origin element, the element must opt into
/// CORS before transport; same-origin resources can use the ordinary path.
/// Finally, response bytes are checked before the caller can execute or parse
/// them.
pub fn fetch_html_subresource_with_integrity(
    navigation: &NavigationNetwork,
    referrer: &DocumentReferrerContext,
    container: &IntegrityPolicyContainer,
    destination: IntegrityPolicyDestination,
    url: &Url,
    crossorigin: Option<&str>,
    referrerpolicy: Option<&str>,
    integrity_metadata: &str,
) -> Result<HtmlSubresourceIntegrityResult, HtmlSubresourceIntegrityError> {
    let effective_target = navigation.effective_url(url);
    let same_origin = referrer.source().is_some_and(|source| {
        Origin::of(source) == Origin::of(&effective_target)
    });
    let mode = if same_origin {
        IntegrityPolicyRequestMode::SameOrigin
    } else if crossorigin.is_some() {
        IntegrityPolicyRequestMode::Cors
    } else {
        IntegrityPolicyRequestMode::NoCors
    };
    let is_local = !matches!(effective_target.scheme(), "http" | "https");

    let policy = evaluate_subresource_integrity_policy(
        container,
        destination,
        integrity_metadata,
        mode,
        is_local,
    );
    if policy.blocked {
        return Err(HtmlSubresourceIntegrityError::PolicyBlocked);
    }

    // SRI on a foreign classic script/stylesheet is only trustworthy after the
    // CORS protocol authorizes the response. Reject before I/O rather than
    // fetching an opaque response and hashing bytes that script could not read.
    if !same_origin
        && mode == IntegrityPolicyRequestMode::NoCors
        && integrity_metadata_has_supported_expression(integrity_metadata)
    {
        return Err(HtmlSubresourceIntegrityError::CorsRequired);
    }

    let same_site = referrer.source().is_some_and(|source| {
        source.scheme() == effective_target.scheme()
            && source.host().eq_ignore_ascii_case(effective_target.host())
    });
    let context = SameSiteRequestContext::new(same_site, false, Method::Get);
    let request = FetchRequest::get(url.clone());
    let response = referrer
        .fetch_subresource_with_cors_credentials(
            navigation,
            &request,
            context,
            referrerpolicy,
            crossorigin,
        )
        .map_err(|error| HtmlSubresourceIntegrityError::Network(error.to_string()))?;

    if !response.ok() {
        return Err(HtmlSubresourceIntegrityError::HttpStatus(response.status));
    }

    let integrity = enforce_subresource_integrity(
        container,
        destination,
        integrity_metadata,
        mode,
        is_local,
        &response.body,
    )
    .map_err(|error| match error {
        SubresourceIntegrityError::PolicyBlocked => HtmlSubresourceIntegrityError::PolicyBlocked,
        SubresourceIntegrityError::IntegrityMismatch => {
            HtmlSubresourceIntegrityError::IntegrityMismatch
        }
    })?;

    Ok(HtmlSubresourceIntegrityResult {
        response,
        report_only_violation: integrity.report_only_violation || policy.report_only_violation,
    })
}

/// Fetch an HTML subresource while materializing any Integrity-Policy
/// violations into the document/session report queue.
///
/// Reporting happens from the same pre-dispatch policy decision used by the
/// loader. Consequently an enforced violation is queued even though transport
/// is never entered, while a report-only violation is queued and loading
/// continues normally. The actual Reporting API delivery mechanism remains
/// decoupled from element loading; this helper only creates pending work.
pub fn fetch_html_subresource_with_integrity_reporting(
    navigation: &NavigationNetwork,
    referrer: &DocumentReferrerContext,
    container: &IntegrityPolicyContainer,
    report_queue: &mut IntegrityReportQueue,
    destination: IntegrityPolicyDestination,
    url: &Url,
    crossorigin: Option<&str>,
    referrerpolicy: Option<&str>,
    integrity_metadata: &str,
) -> Result<HtmlSubresourceIntegrityResult, HtmlSubresourceIntegrityError> {
    let effective_target = navigation.effective_url(url);
    let same_origin = referrer.source().is_some_and(|source| {
        Origin::of(source) == Origin::of(&effective_target)
    });
    let mode = if same_origin {
        IntegrityPolicyRequestMode::SameOrigin
    } else if crossorigin.is_some() {
        IntegrityPolicyRequestMode::Cors
    } else {
        IntegrityPolicyRequestMode::NoCors
    };
    let is_local = !matches!(effective_target.scheme(), "http" | "https");
    let decision = evaluate_subresource_integrity_policy(
        container,
        destination,
        integrity_metadata,
        mode,
        is_local,
    );

    if let Some(document_url) = referrer.source() {
        report_queue.enqueue_decision(
            document_url,
            url,
            destination,
            decision,
            &container.enforced,
            &container.report_only,
        );
    }

    fetch_html_subresource_with_integrity(
        navigation,
        referrer,
        container,
        destination,
        url,
        crossorigin,
        referrerpolicy,
        integrity_metadata,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity_policy::IntegrityPolicy;

    #[test]
    fn request_mode_prefers_same_origin_over_crossorigin_attribute() {
        let container = IntegrityPolicyContainer {
            enforced: IntegrityPolicy::parse("blocked-destinations=(script)"),
            report_only: IntegrityPolicy::default(),
        };
        // This unit merely locks the policy assumption used by the transport
        // helper: valid SRI metadata on a same-origin request satisfies policy.
        let decision = evaluate_subresource_integrity_policy(
            &container,
            IntegrityPolicyDestination::Script,
            "sha256-deadbeef",
            IntegrityPolicyRequestMode::SameOrigin,
            false,
        );
        assert!(!decision.blocked);
    }
}
