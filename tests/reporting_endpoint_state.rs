use browser_engine::{
    IntegrityViolationReport, IntegrityViolationReportBody, ReportingDeliveryBatch,
    ReportingDeliveryDisposition, ReportingDeliveryOutcome, ReportingEndpointState,
    ReportingEndpoints, Url,
};

fn report(endpoint: &str, blocked: &str) -> IntegrityViolationReport {
    IntegrityViolationReport {
        report_type: "integrity-violation",
        endpoint: endpoint.into(),
        body: IntegrityViolationReportBody {
            document_url: "https://example.test/page".into(),
            blocked_url: blocked.into(),
            destination: "script".into(),
            report_only: false,
        },
    }
}

#[test]
fn gone_endpoint_is_excluded_from_future_report_resolution() {
    let endpoints = ReportingEndpoints::parse(
        r#"default="https://reports.test/collect", fallback="https://reports.test/fallback""#,
    );
    let mut state = ReportingEndpointState::new(endpoints);
    let reports = vec![
        report("default", "https://cdn.test/a.js"),
        report("fallback", "https://cdn.test/b.js"),
    ];

    let before = state.resolve(&reports);
    assert_eq!(before.len(), 2);

    let outcome = ReportingDeliveryOutcome::Delivered {
        id: 9,
        batch: ReportingDeliveryBatch {
            endpoint_url: Url::parse("https://reports.test/collect").unwrap(),
            reports: vec![before[0].clone()],
        },
        disposition: ReportingDeliveryDisposition::RemoveEndpoint,
    };
    assert_eq!(state.apply_delivery_outcomes(&[outcome]), 1);

    let after = state.resolve(&reports);
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].endpoint_name, "fallback");
}

#[test]
fn aliases_to_removed_endpoint_are_suppressed_together() {
    let endpoints = ReportingEndpoints::parse(
        r#"primary="https://reports.test/collect", secondary="https://reports.test/collect""#,
    );
    let mut state = ReportingEndpointState::new(endpoints);
    let reports = vec![
        report("primary", "https://cdn.test/a.js"),
        report("secondary", "https://cdn.test/b.js"),
    ];
    let resolved = state.resolve(&reports);

    let outcome = ReportingDeliveryOutcome::Delivered {
        id: 10,
        batch: ReportingDeliveryBatch {
            endpoint_url: Url::parse("https://reports.test/collect").unwrap(),
            reports: vec![resolved[0].clone()],
        },
        disposition: ReportingDeliveryDisposition::RemoveEndpoint,
    };
    assert_eq!(state.apply_delivery_outcomes(&[outcome]), 1);
    assert!(state.resolve(&reports).is_empty());
}

#[test]
fn ordinary_success_does_not_poison_endpoint_state() {
    let endpoints = ReportingEndpoints::parse(
        r#"default="https://reports.test/collect""#,
    );
    let mut state = ReportingEndpointState::new(endpoints);
    let reports = vec![report("default", "https://cdn.test/a.js")];
    let resolved = state.resolve(&reports);

    let outcome = ReportingDeliveryOutcome::Delivered {
        id: 11,
        batch: ReportingDeliveryBatch {
            endpoint_url: Url::parse("https://reports.test/collect").unwrap(),
            reports: resolved,
        },
        disposition: ReportingDeliveryDisposition::Delivered,
    };
    assert_eq!(state.apply_delivery_outcomes(&[outcome]), 0);
    assert_eq!(state.resolve(&reports).len(), 1);
}
