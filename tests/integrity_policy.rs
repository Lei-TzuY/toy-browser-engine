use browser_engine::{
    evaluate_integrity_policy, IntegrityPolicy, IntegrityPolicyDecision, IntegrityPolicyDestination,
    IntegrityPolicyRequestMode, IntegrityPolicySource,
};

#[test]
fn absent_sources_defaults_to_inline_and_current_destinations_are_recognized() {
    let policy = IntegrityPolicy::parse(
        "blocked-destinations=(script style), endpoints=(integrity-endpoint backup)",
    );

    assert_eq!(policy.sources, vec![IntegrityPolicySource::Inline]);
    assert!(policy.blocks_destination(IntegrityPolicyDestination::Script));
    assert!(policy.blocks_destination(IntegrityPolicyDestination::Style));
    assert!(!policy.blocks_destination(IntegrityPolicyDestination::Other));
    assert_eq!(policy.endpoints, vec!["integrity-endpoint", "backup"]);
}

#[test]
fn unknown_tokens_do_not_widen_enforcement() {
    let policy = IntegrityPolicy::parse(
        "sources=(future-source), blocked-destinations=(image future-dest), endpoints=(report)",
    );

    assert!(policy.sources.is_empty());
    assert!(policy.blocked_destinations.is_empty());
    assert_eq!(policy.endpoints, vec!["report"]);
}

#[test]
fn report_only_policy_never_blocks() {
    let enforced = IntegrityPolicy::default();
    let report_only = IntegrityPolicy::parse("blocked-destinations=(script)");

    assert_eq!(
        evaluate_integrity_policy(
            &enforced,
            &report_only,
            IntegrityPolicyDestination::Script,
            false,
            IntegrityPolicyRequestMode::NoCors,
            false,
        ),
        IntegrityPolicyDecision {
            blocked: false,
            enforced_violation: false,
            report_only_violation: true,
        }
    );
}

#[test]
fn enforced_policy_blocks_missing_integrity_for_script_and_style() {
    let enforced = IntegrityPolicy::parse("blocked-destinations=(script style)");
    let empty = IntegrityPolicy::default();

    for destination in [
        IntegrityPolicyDestination::Script,
        IntegrityPolicyDestination::Style,
    ] {
        let decision = evaluate_integrity_policy(
            &enforced,
            &empty,
            destination,
            false,
            IntegrityPolicyRequestMode::NoCors,
            false,
        );
        assert!(decision.blocked);
        assert!(decision.enforced_violation);
        assert!(!decision.report_only_violation);
    }
}

#[test]
fn valid_integrity_only_short_circuits_for_cors_or_same_origin_modes() {
    let enforced = IntegrityPolicy::parse("blocked-destinations=(script)");
    let empty = IntegrityPolicy::default();

    for mode in [
        IntegrityPolicyRequestMode::Cors,
        IntegrityPolicyRequestMode::SameOrigin,
    ] {
        assert_eq!(
            evaluate_integrity_policy(
                &enforced,
                &empty,
                IntegrityPolicyDestination::Script,
                true,
                mode,
                false,
            ),
            IntegrityPolicyDecision::default()
        );
    }

    assert!(
        evaluate_integrity_policy(
            &enforced,
            &empty,
            IntegrityPolicyDestination::Script,
            true,
            IntegrityPolicyRequestMode::NoCors,
            false,
        )
        .blocked
    );
}

#[test]
fn local_requests_are_exempt_before_policy_enforcement() {
    let enforced = IntegrityPolicy::parse("blocked-destinations=(style)");
    let report_only = IntegrityPolicy::parse("blocked-destinations=(style)");

    assert_eq!(
        evaluate_integrity_policy(
            &enforced,
            &report_only,
            IntegrityPolicyDestination::Style,
            false,
            IntegrityPolicyRequestMode::NoCors,
            true,
        ),
        IntegrityPolicyDecision::default()
    );
}

#[test]
fn malformed_structured_fields_fail_to_a_non_blocking_policy() {
    for malformed in [
        "blocked-destinations=script",
        "blocked-destinations=(script",
        "blocked-destinations=(script), blocked-destinations=(style)",
        "endpoints=(\"quoted-string\")",
    ] {
        let policy = IntegrityPolicy::parse(malformed);
        assert_eq!(policy.sources, vec![IntegrityPolicySource::Inline]);
        assert!(policy.blocked_destinations.is_empty());
        assert!(policy.endpoints.is_empty());
    }
}
