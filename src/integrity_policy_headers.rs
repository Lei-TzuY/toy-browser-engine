//! Response-header plumbing for Subresource Integrity's Integrity-Policy.
//!
//! SRI initializes a document policy container with two empty policies and only
//! processes a policy when the corresponding response header is present. This
//! distinction matters: processing a present field defaults a missing `sources`
//! member to `inline`, while an absent field leaves the container policy empty.

use crate::integrity_policy::{
    evaluate_integrity_policy, IntegrityPolicy, IntegrityPolicyDecision, IntegrityPolicyDestination,
    IntegrityPolicyRequestMode,
};
use crate::net::{FetchResponse, HeaderMap};

pub const INTEGRITY_POLICY_HEADER: &str = "integrity-policy";
pub const INTEGRITY_POLICY_REPORT_ONLY_HEADER: &str = "integrity-policy-report-only";

/// Integrity policy state committed by a document response.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IntegrityPolicyContainer {
    pub enforced: IntegrityPolicy,
    pub report_only: IntegrityPolicy,
}

impl IntegrityPolicyContainer {
    /// Parse both Integrity-Policy response headers into document policy state.
    ///
    /// `HeaderMap::get` combines repeated field lines in arrival order. That is
    /// appropriate for Structured Fields dictionaries: repeated dictionary
    /// members become duplicate keys and the policy parser safely falls back to
    /// its harmless processed-field default.
    pub fn from_headers(headers: &HeaderMap) -> Self {
        Self {
            enforced: parse_present_policy(headers, INTEGRITY_POLICY_HEADER),
            report_only: parse_present_policy(headers, INTEGRITY_POLICY_REPORT_ONLY_HEADER),
        }
    }

    pub fn from_response(response: &FetchResponse) -> Self {
        Self::from_headers(&response.headers)
    }

    /// Apply this response-committed policy to one subresource request.
    pub fn evaluate(
        &self,
        destination: IntegrityPolicyDestination,
        has_valid_integrity_metadata: bool,
        mode: IntegrityPolicyRequestMode,
        is_local: bool,
    ) -> IntegrityPolicyDecision {
        evaluate_integrity_policy(
            &self.enforced,
            &self.report_only,
            destination,
            has_valid_integrity_metadata,
            mode,
            is_local,
        )
    }
}

fn parse_present_policy(headers: &HeaderMap, name: &str) -> IntegrityPolicy {
    match headers.get(name) {
        Some(value) => IntegrityPolicy::parse(&value),
        None => IntegrityPolicy::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity_policy::{IntegrityPolicyDestination, IntegrityPolicySource};
    use crate::net::Url;

    #[test]
    fn absent_headers_leave_both_container_policies_empty() {
        let container = IntegrityPolicyContainer::from_headers(&HeaderMap::new());
        assert_eq!(container, IntegrityPolicyContainer::default());
        assert!(container.enforced.sources.is_empty());
        assert!(container.report_only.sources.is_empty());
    }

    #[test]
    fn parses_enforced_and_report_only_independently() {
        let mut headers = HeaderMap::new();
        headers.insert_raw(
            INTEGRITY_POLICY_HEADER,
            "blocked-destinations=(script), endpoints=(enforced)",
        );
        headers.insert_raw(
            INTEGRITY_POLICY_REPORT_ONLY_HEADER,
            "blocked-destinations=(style), endpoints=(observe)",
        );

        let container = IntegrityPolicyContainer::from_headers(&headers);
        assert_eq!(container.enforced.sources, vec![IntegrityPolicySource::Inline]);
        assert_eq!(
            container.enforced.blocked_destinations,
            vec![IntegrityPolicyDestination::Script]
        );
        assert_eq!(container.enforced.endpoints, vec!["enforced"]);
        assert_eq!(
            container.report_only.blocked_destinations,
            vec![IntegrityPolicyDestination::Style]
        );
        assert_eq!(container.report_only.endpoints, vec!["observe"]);
    }

    #[test]
    fn repeated_dictionary_member_fails_safe_after_header_combination() {
        let mut headers = HeaderMap::new();
        headers.append_raw(INTEGRITY_POLICY_HEADER, "blocked-destinations=(script)");
        headers.append_raw(INTEGRITY_POLICY_HEADER, "blocked-destinations=(style)");

        let container = IntegrityPolicyContainer::from_headers(&headers);
        assert_eq!(container.enforced.sources, vec![IntegrityPolicySource::Inline]);
        assert!(container.enforced.blocked_destinations.is_empty());
        assert!(container.enforced.endpoints.is_empty());
    }

    #[test]
    fn can_build_policy_container_directly_from_fetch_response() {
        let mut response = FetchResponse::synthetic(
            Url::parse("https://example.test/").unwrap(),
            200,
            Some("text/html"),
            Vec::new(),
        );
        response.headers.insert_raw(
            INTEGRITY_POLICY_HEADER,
            "blocked-destinations=(script style)",
        );

        let container = IntegrityPolicyContainer::from_response(&response);
        assert!(container
            .enforced
            .blocks_destination(IntegrityPolicyDestination::Script));
        assert!(container
            .enforced
            .blocks_destination(IntegrityPolicyDestination::Style));
    }
}
