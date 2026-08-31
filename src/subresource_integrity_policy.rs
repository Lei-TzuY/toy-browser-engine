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
/// The request policy gate runs first. A syntactically valid supported integrity
/// hash only satisfies Integrity-Policy for CORS or same-origin request modes;
/// malformed or unsupported-only metadata therefore cannot bypass policy. Once
/// the request is allowed, classic SRI verification applies to supported
/// metadata. Empty or unsupported-only metadata remains non-verifying for hash
/// agility in the lower-level SRI verifier.
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

/// Return whether metadata contains at least one syntactically valid integrity
/// expression using an algorithm implemented by this engine.
///
/// Integrity-Policy's `inline` source is satisfied by actual integrity metadata,
/// not merely by an algorithm-looking prefix. In particular, strings such as
/// `sha256-deadbeef` must not turn a request-time policy violation into a
/// post-response hash mismatch and thereby cause a request that should have
/// been blocked before dispatch.
///
/// We accept both the normal and URL-safe Base64 alphabets and both padded and
/// unpadded encodings. The decoded size must equal the digest size mandated by
/// the selected SHA-2 algorithm.
pub fn integrity_metadata_has_supported_expression(metadata: &str) -> bool {
    metadata.split_ascii_whitespace().any(|item| {
        let expression = item.split('?').next().unwrap_or(item);
        let Some((algorithm, digest)) = expression.split_once('-') else {
            return false;
        };
        let Some(expected_len) = supported_digest_length_bytes(algorithm) else {
            return false;
        };
        base64_value_matches_digest_length(digest, expected_len)
    })
}

fn supported_digest_length_bytes(algorithm: &str) -> Option<usize> {
    if algorithm.eq_ignore_ascii_case("sha256") {
        Some(32)
    } else if algorithm.eq_ignore_ascii_case("sha384") {
        Some(48)
    } else if algorithm.eq_ignore_ascii_case("sha512") {
        Some(64)
    } else {
        None
    }
}

fn base64_value_matches_digest_length(value: &str, expected_len: usize) -> bool {
    if value.is_empty() {
        return false;
    }

    let unpadded_len = value.trim_end_matches('=').len();
    let padding = value.len().saturating_sub(unpadded_len);
    if padding > 2 || value[..unpadded_len].contains('=') {
        return false;
    }
    if padding > 0 && value.len() % 4 != 0 {
        return false;
    }
    if unpadded_len % 4 == 1 {
        return false;
    }
    if !value[..unpadded_len].bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'-' | b'_')
    }) {
        return false;
    }

    // Each Base64 character contributes six payload bits. Integer division is
    // exactly the decoded byte length for legal unpadded lengths (mod 4 != 1).
    unpadded_len.saturating_mul(6) / 8 == expected_len
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
    fn supported_metadata_detection_requires_valid_digest_shape() {
        assert!(!integrity_metadata_has_supported_expression(""));
        assert!(!integrity_metadata_has_supported_expression("sha999-deadbeef"));
        assert!(!integrity_metadata_has_supported_expression("sha256-deadbeef"));
        assert!(!integrity_metadata_has_supported_expression("sha256-%%%%"));
        assert!(!integrity_metadata_has_supported_expression(
            "sha256-Jok2eyBcFs4y7UIAlCuLix4mLfxw2byfvHfElpmk8d8=="
        ));
        assert!(integrity_metadata_has_supported_expression(
            "sha256-Jok2eyBcFs4y7UIAlCuLix4mLfxw2byfvHfElpmk8d8="
        ));
        assert!(integrity_metadata_has_supported_expression(
            "sha256-Jok2eyBcFs4y7UIAlCuLix4mLfxw2byfvHfElpmk8d8?foo"
        ));
        assert!(integrity_metadata_has_supported_expression(
            "sha999-x sha512-n7u7Wg8yn5eC4jVvpB2Jz5s2lDJ8GpNNavKp3y1/k2zoNxf7UTGWpM5VSEcXCM1xNMKumbPDV7yrsur8e5t1cA=="
        ));
    }

    #[test]
    fn url_safe_unpadded_digest_shape_is_accepted() {
        assert!(base64_value_matches_digest_length(
            "Jok2eyBcFs4y7UIAlCuLix4mLfxw2byfvHfElpmk8d8",
            32
        ));
        assert!(base64_value_matches_digest_length(
            "___________________________________________",
            32
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
    fn malformed_supported_algorithm_metadata_is_still_policy_blocked() {
        assert_eq!(
            enforce_subresource_integrity(
                &blocking_container(),
                IntegrityPolicyDestination::Script,
                "sha256-deadbeef",
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
