use browser_engine::{
    enforce_subresource_integrity, evaluate_subresource_integrity_policy, IntegrityPolicy,
    IntegrityPolicyContainer, IntegrityPolicyDestination, IntegrityPolicyRequestMode,
    SubresourceIntegrityError,
};

fn container(enforced: &str, report_only: &str) -> IntegrityPolicyContainer {
    IntegrityPolicyContainer {
        enforced: IntegrityPolicy::parse(enforced),
        report_only: IntegrityPolicy::parse(report_only),
    }
}

#[test]
fn policy_can_reject_script_before_response_verification() {
    let policy = container("blocked-destinations=(script)", "sources=()");
    let decision = evaluate_subresource_integrity_policy(
        &policy,
        IntegrityPolicyDestination::Script,
        "",
        IntegrityPolicyRequestMode::Cors,
        false,
    );
    assert!(decision.blocked);
    assert!(decision.enforced_violation);
}

#[test]
fn supported_integrity_metadata_allows_policy_then_verifies_body() {
    let policy = container("blocked-destinations=(script)", "sources=()");
    let metadata = "sha256-Jok2eyBcFs4y7UIAlCuLix4mLfxw2byfvHfElpmk8d8=";

    assert!(enforce_subresource_integrity(
        &policy,
        IntegrityPolicyDestination::Script,
        metadata,
        IntegrityPolicyRequestMode::Cors,
        false,
        b"ok",
    )
    .is_ok());

    assert_eq!(
        enforce_subresource_integrity(
            &policy,
            IntegrityPolicyDestination::Script,
            metadata,
            IntegrityPolicyRequestMode::Cors,
            false,
            b"changed",
        ),
        Err(SubresourceIntegrityError::IntegrityMismatch)
    );
}

#[test]
fn unsupported_only_metadata_does_not_bypass_integrity_policy() {
    let policy = container("blocked-destinations=(style)", "sources=()");
    assert_eq!(
        enforce_subresource_integrity(
            &policy,
            IntegrityPolicyDestination::Style,
            "sha999-future-digest",
            IntegrityPolicyRequestMode::Cors,
            false,
            b"body",
        ),
        Err(SubresourceIntegrityError::PolicyBlocked)
    );
}

#[test]
fn no_cors_metadata_does_not_count_as_policy_opt_in() {
    let policy = container("blocked-destinations=(script)", "sources=()");
    let metadata = "sha256-Jok2eyBcFs4y7UIAlCuLix4mLfxw2byfvHfElpmk8d8=";
    assert_eq!(
        enforce_subresource_integrity(
            &policy,
            IntegrityPolicyDestination::Script,
            metadata,
            IntegrityPolicyRequestMode::NoCors,
            false,
            b"ok",
        ),
        Err(SubresourceIntegrityError::PolicyBlocked)
    );
}

#[test]
fn report_only_violation_is_observable_without_blocking() {
    let policy = container("sources=()", "blocked-destinations=(style)");
    let result = enforce_subresource_integrity(
        &policy,
        IntegrityPolicyDestination::Style,
        "",
        IntegrityPolicyRequestMode::SameOrigin,
        false,
        b"body",
    )
    .expect("report-only policy must not block");
    assert!(result.report_only_violation);
}

#[test]
fn destination_scope_does_not_leak_between_script_and_style() {
    let policy = container("blocked-destinations=(script)", "sources=()");
    assert!(enforce_subresource_integrity(
        &policy,
        IntegrityPolicyDestination::Style,
        "",
        IntegrityPolicyRequestMode::Cors,
        false,
        b"body",
    )
    .is_ok());
}

#[test]
fn local_resource_exemption_survives_combined_gate() {
    let policy = container("blocked-destinations=(script style)", "sources=()");
    assert!(enforce_subresource_integrity(
        &policy,
        IntegrityPolicyDestination::Script,
        "",
        IntegrityPolicyRequestMode::SameOrigin,
        true,
        b"body",
    )
    .is_ok());
}
