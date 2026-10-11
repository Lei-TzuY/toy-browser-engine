//! Document policy container for cross-origin embedder/resource checks.
//!
//! A loaded document needs to retain both the enforced and report-only COEP
//! response policies.  Subresource loaders can then ask one object whether a
//! no-CORS response is allowed and separately whether the report-only policy
//! would have blocked it.

use crate::cross_origin_embedder_policy::{
    parse_cross_origin_embedder_policy, parse_cross_origin_embedder_policy_report_only,
    ParsedCrossOriginEmbedderPolicy,
};
use crate::cross_origin_resource_policy::{
    response_allows_cross_origin_resource_with_embedder_policy, CorpOriginRelation,
};
use crate::net::HeaderMap;

/// Cross-origin policy state inherited from a document response.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DocumentPolicyContainer {
    pub embedder_policy: ParsedCrossOriginEmbedderPolicy,
    pub embedder_policy_report_only: ParsedCrossOriginEmbedderPolicy,
}

/// Result of checking one no-CORS response against the document policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoCorsResponsePolicyResult {
    /// Whether the enforced policy allows the response to be consumed.
    pub allowed: bool,
    /// Whether the report-only policy would have blocked the response.
    ///
    /// This never changes `allowed`; callers can use it to emit diagnostics or
    /// reporting events without accidentally enforcing report-only policy.
    pub report_only_violation: bool,
}

impl DocumentPolicyContainer {
    /// Build the policy container from one document response's headers.
    pub fn from_response_headers(headers: &HeaderMap) -> Self {
        Self {
            embedder_policy: parse_cross_origin_embedder_policy(headers),
            embedder_policy_report_only: parse_cross_origin_embedder_policy_report_only(headers),
        }
    }

    /// Apply the document's COEP state to one no-CORS response.
    ///
    /// Explicit CORP response policy is parsed inside the underlying Fetch
    /// internal check. The report-only policy is evaluated independently and
    /// can therefore produce telemetry without affecting delivery.
    pub fn check_no_cors_response(
        &self,
        response_headers: &HeaderMap,
        relation: CorpOriginRelation,
        initiator_is_https: bool,
        response_is_https: bool,
        request_includes_credentials: bool,
        for_navigation: bool,
    ) -> NoCorsResponsePolicyResult {
        let allowed = response_allows_cross_origin_resource_with_embedder_policy(
            response_headers,
            self.embedder_policy.policy,
            relation,
            initiator_is_https,
            response_is_https,
            request_includes_credentials,
            for_navigation,
        );
        let report_only_allowed = response_allows_cross_origin_resource_with_embedder_policy(
            response_headers,
            self.embedder_policy_report_only.policy,
            relation,
            initiator_is_https,
            response_is_https,
            request_includes_credentials,
            for_navigation,
        );

        NoCorsResponsePolicyResult {
            allowed,
            report_only_violation: !report_only_allowed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document_headers(enforced: Option<&str>, report_only: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(value) = enforced {
            headers.append_raw("Cross-Origin-Embedder-Policy", value);
        }
        if let Some(value) = report_only {
            headers.append_raw("Cross-Origin-Embedder-Policy-Report-Only", value);
        }
        headers
    }

    fn resource_headers(corp: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(value) = corp {
            headers.append_raw("Cross-Origin-Resource-Policy", value);
        }
        headers
    }

    #[test]
    fn retains_enforced_and_report_only_policy_independently() {
        let container = DocumentPolicyContainer::from_response_headers(&document_headers(
            Some("require-corp; report-to=\"enforced\""),
            Some("credentialless; report-to=\"observe\""),
        ));

        assert_eq!(container.embedder_policy.report_to.as_deref(), Some("enforced"));
        assert_eq!(
            container.embedder_policy_report_only.report_to.as_deref(),
            Some("observe")
        );
    }

    #[test]
    fn require_corp_blocks_cross_site_response_without_opt_in() {
        let container = DocumentPolicyContainer::from_response_headers(&document_headers(
            Some("require-corp"),
            None,
        ));
        let result = container.check_no_cors_response(
            &resource_headers(None),
            CorpOriginRelation::CrossSite,
            true,
            true,
            false,
            false,
        );
        assert!(!result.allowed);
        assert!(!result.report_only_violation);
    }

    #[test]
    fn explicit_cross_origin_corp_satisfies_require_corp() {
        let container = DocumentPolicyContainer::from_response_headers(&document_headers(
            Some("require-corp"),
            None,
        ));
        let result = container.check_no_cors_response(
            &resource_headers(Some("cross-origin")),
            CorpOriginRelation::CrossSite,
            true,
            true,
            true,
            false,
        );
        assert!(result.allowed);
    }

    #[test]
    fn report_only_violation_never_blocks_delivery() {
        let container = DocumentPolicyContainer::from_response_headers(&document_headers(
            None,
            Some("require-corp"),
        ));
        let result = container.check_no_cors_response(
            &resource_headers(None),
            CorpOriginRelation::CrossSite,
            true,
            true,
            false,
            false,
        );
        assert!(result.allowed);
        assert!(result.report_only_violation);
    }

    #[test]
    fn credentialless_allows_uncredentialed_cross_site_subresource() {
        let container = DocumentPolicyContainer::from_response_headers(&document_headers(
            Some("credentialless"),
            None,
        ));
        let result = container.check_no_cors_response(
            &resource_headers(None),
            CorpOriginRelation::CrossSite,
            true,
            true,
            false,
            false,
        );
        assert!(result.allowed);
    }

    #[test]
    fn credentialless_still_blocks_credentialed_cross_site_subresource() {
        let container = DocumentPolicyContainer::from_response_headers(&document_headers(
            Some("credentialless"),
            None,
        ));
        let result = container.check_no_cors_response(
            &resource_headers(None),
            CorpOriginRelation::CrossSite,
            true,
            true,
            true,
            false,
        );
        assert!(!result.allowed);
    }
}
