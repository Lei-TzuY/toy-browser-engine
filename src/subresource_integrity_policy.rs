//! Shared SRI + Integrity-Policy gate for document-owned subresources.
//!
//! `Integrity-Policy` is a request-time gate while classic Subresource
//! Integrity verifies the response bytes. Keeping both decisions in one helper
//! prevents element loaders from accidentally enforcing only half of the SRI
//! contract.

use crate::integrity_policy::{
    IntegrityPolicyDecision, IntegrityPolicyDestination, IntegrityPolicyRequestMode,
};
use crate::integrity_policy_headers::IntegrityPolicyContainer;

/// Why a document-owned script/style subresource was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubresourceIntegrityError {
    /// The enforced `Integrity-Policy` rejected the request before its body
    /// could be accepted.
    PolicyBlocked,
    /// Supported integrity metadata was present but the response bytes did not
    /// satisfy the strongest supported hash algorithm in the metadata.
    IntegrityMismatch,
}

/// Successful SRI-policy evaluation metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SubresourceIntegrityResult {
    /// A report-only policy would have rejected this request. Callers can use
    /// this to feed a Reporting API implementation without blocking loading.
    pub report_only_violation: bool,
}

/// Evaluate an element subresource against the committed document
/// `Integrity-Policy` and, when applicable, verify its response bytes using SRI.
///
/// The request policy gate runs first. A syntactically supported integrity hash
/// only satisfies Integrity-Policy for CORS or same-origin request modes; an
/// unsupported-only metadata string therefore cannot bypass policy. Once the
/// request is allowed, classic SRI verification applies to supported metadata.
/// Empty or unsupported-only metadata remains non-verifying for hash agility.
pub fn enforce_subresource_integrity(
    container: &IntegrityPolicyContainer,
    destination: IntegrityPolicyDestination,
    integrity_metadata: &str,
    mode: IntegrityPolicyRequestMode,
    is_local: bool,
    response_body: &[u8],
) -> Result<SubresourceIntegrityResult, SubresourceIntegrityError> {
    let has_supported_metadata = integrity_metadata_has_supported_expression(integrity_metadata);
    let decision = container.evaluate(
        destination,
        has_supported_metadata,
        mode,
        is_local,
    );

    if decision.blocked {
        return Err(SubresourceIntegrityError::PolicyBlocked);
    }

    if has_supported_metadata
        && !crate::fetch_integrity::bytes_match_integrity(integrity_metadata, response_body)
    {
        return Err(SubresourceIntegrityError::IntegrityMismatch);
    }

    Ok(SubresourceIntegrityResult {
        report_only_violation: decision.report_only_violation,
    })
}

/// Return whether metadata contains at least one integrity expression using an
/// algorithm implemented by this engine.
///
/// SRI ignores unsupported algorithms for hash agility. Integrity-Policy must
/// not, however, treat unsupported-only metadata as a valid opt-in, otherwise a
/// future-looking token such as `sha999-...` would disable policy enforcement.
pub fn integrity_metadata_has_supported_expression(metadata: &str) -> bool {
    metadata.split_ascii_whitespace().any(|item| {
        let expression = item.split('?').next().unwrap_or(item);
        let Some((algorithm, digest)) = expression.split_once('-') else {
            return false;
        };
        !digest.is_empty()
            && (algorithm.eq_ignore_ascii_case("sha256")
                || algorithm.eq_ignore_ascii_case("sha384")
                || algorithm.eq_ignore_ascii_case("sha512"))
    })
}

/// Expose the policy-only decision for loaders that need to reject before they
/// dispatch network I/O while still using the same supported-metadata test as
/// the combined gate.
pub fn evaluate_subresource_integrity_policy(
    container: &IntegrityPolicyContainer,
    destination: IntegrityPolicyDestination,
    integrity_metadata: &str,
    mode: IntegrityPolicyRequestMode,
    is_local: bool,
) -> IntegrityPolicyDecision {
    container.evaluate(
        destination,
        integrity_metadata_has_supported_expression(integrity_metadata),
        mode,
        is_local,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity_policy::IntegrityPolicy;

    fn blocking_container() -> IntegrityPolicyContainer {
        IntegrityPolicyContainer {
            enforced: IntegrityPolicy::parse("blocked-destinations=(script style)"),
            report_only: IntegrityPolicy::default(),
        }
    }

    #[test]
    fn supported_metadata_detection_ignores_unknown_algorithms() {
        assert!(!integrity_metadata_has_supported_expression(""));
        assert!(!integrity_metadata_has_supported_expression("sha999-deadbeef"));
        assert!(integrity_metadata_has_supported_expression("sha256-deadbeef"));
        assert!(integrity_metadata_has_supported_expression(
            "sha999-x sha512-deadbeef?foo"
        ));
    }

    #[test]
    fn enforced_policy_blocks_missing_metadata() {
        assert_eq!(
            enforce_subresource_integrity(
                &blocking_container(),
                IntegrityPolicyDestination::Script,
                "",
                IntegrityPolicyRequestMode::Cors,
                false,
                b"ok",
            ),
            Err(SubresourceIntegrityError::PolicyBlocked)
        );
    }

    #[test]
    fn matching_supported_metadata_satisfies_policy_and_sri() {
        let metadata = "sha256-Jok2eyBcFs4y7UIAlCuLix4mLfxw2byfvHfElpmk8d8=";
        assert_eq!(
            enforce_subresource_integrity(
                &blocking_container(),
                IntegrityPolicyDestination::Script,
                metadata,
                IntegrityPolicyRequestMode::Cors,
                false,
                b"ok",
            ),
            Ok(SubresourceIntegrityResult::default())
        );
    }

    #[test]
    fn matching_policy_metadata_still_rejects_wrong_response_bytes() {
        let metadata = "sha256-Jok2eyBcFs4y7UIAlCuLix4mLfxw2byfvHfElpmk8d8=";
        assert_eq!(
            enforce_subresource_integrity(
                &blocking_container(),
                IntegrityPolicyDestination::Style,
                metadata,
                IntegrityPolicyRequestMode::SameOrigin,
                false,
                b"tampered",
            ),
            Err(SubresourceIntegrityError::IntegrityMismatch)
        );
    }

    #[test]
    fn unsupported_only_metadata_cannot_bypass_integrity_policy() {
        assert_eq!(
            enforce_subresource_integrity(
                &blocking_container(),
                IntegrityPolicyDestination::Script,
                "sha999-future",
                IntegrityPolicyRequestMode::Cors,
                false,
                b"anything",
            ),
            Err(SubresourceIntegrityError::PolicyBlocked)
        );
    }

    #[test]
    fn local_resources_are_exempt_from_policy_and_do_not_require_metadata() {
        assert!(enforce_subresource_integrity(
            &blocking_container(),
            IntegrityPolicyDestination::Style,
            "",
            IntegrityPolicyRequestMode::SameOrigin,
            true,
            b"body",
        )
        .is_ok());
    }
}
