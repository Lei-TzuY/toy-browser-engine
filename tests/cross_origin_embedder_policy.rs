use browser_engine::net::fetch::HeaderMap;
use browser_engine::{
    parse_cross_origin_embedder_policy, parse_cross_origin_embedder_policy_report_only,
    CrossOriginEmbedderPolicy,
};

fn headers(name: &str, values: &[&str]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for value in values {
        headers.append_raw(name, value);
    }
    headers
}

#[test]
fn enforced_coep_defaults_and_tokens_are_script_independent() {
    assert_eq!(
        parse_cross_origin_embedder_policy(&HeaderMap::new()).policy,
        CrossOriginEmbedderPolicy::UnsafeNone
    );

    let require = parse_cross_origin_embedder_policy(&headers(
        "Cross-Origin-Embedder-Policy",
        &["require-corp"],
    ));
    assert_eq!(require.policy, CrossOriginEmbedderPolicy::RequireCorp);

    let credentialless = parse_cross_origin_embedder_policy(&headers(
        "cross-origin-embedder-policy",
        &["credentialless"],
    ));
    assert_eq!(
        credentialless.policy,
        CrossOriginEmbedderPolicy::Credentialless
    );
}

#[test]
fn report_endpoint_and_report_only_policy_are_preserved() {
    let enforced = parse_cross_origin_embedder_policy(&headers(
        "Cross-Origin-Embedder-Policy",
        &[r#"require-corp; report-to="enforce""#],
    ));
    assert_eq!(enforced.policy, CrossOriginEmbedderPolicy::RequireCorp);
    assert_eq!(enforced.report_to.as_deref(), Some("enforce"));

    let report_only = parse_cross_origin_embedder_policy_report_only(&headers(
        "Cross-Origin-Embedder-Policy-Report-Only",
        &[r#"credentialless; report-to="observe""#],
    ));
    assert_eq!(
        report_only.policy,
        CrossOriginEmbedderPolicy::Credentialless
    );
    assert_eq!(report_only.report_to.as_deref(), Some("observe"));
}

#[test]
fn malformed_or_duplicated_policy_fails_to_unsafe_none() {
    for value in ["Require-Corp", "require-corp, credentialless", "unknown"] {
        assert_eq!(
            parse_cross_origin_embedder_policy(&headers(
                "Cross-Origin-Embedder-Policy",
                &[value],
            ))
            .policy,
            CrossOriginEmbedderPolicy::UnsafeNone
        );
    }

    let duplicated = headers(
        "Cross-Origin-Embedder-Policy",
        &["require-corp", "credentialless"],
    );
    assert_eq!(
        parse_cross_origin_embedder_policy(&duplicated).policy,
        CrossOriginEmbedderPolicy::UnsafeNone
    );
}

#[test]
fn unknown_well_formed_parameters_do_not_change_policy() {
    let parsed = parse_cross_origin_embedder_policy(&headers(
        "Cross-Origin-Embedder-Policy",
        &[r#"require-corp; future=?1; report-to="coep""#],
    ));
    assert_eq!(parsed.policy, CrossOriginEmbedderPolicy::RequireCorp);
    assert_eq!(parsed.report_to.as_deref(), Some("coep"));
}
