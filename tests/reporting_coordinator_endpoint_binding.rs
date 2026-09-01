use browser_engine::{
    IntegrityViolationReport, IntegrityViolationReportBody, ReportingCoordinator,
    ReportingDeliveryBatch, ReportingEndpoints, ReportingRetryPolicy,
    ResolvedIntegrityViolationReport, Url,
};

fn report(endpoint: &str) -> IntegrityViolationReport {
    IntegrityViolationReport {
        report_type: "integrity-violation",
        endpoint: endpoint.into(),
        body: IntegrityViolationReportBody {
            document_url: "https://example.test/page".into(),
            blocked_url: "https://cdn.test/app.js".into(),
            destination: "script".into(),
            report_only: false,
        },
    }
}

fn coordinator() -> ReportingCoordinator {
    ReportingCoordinator::new(
        ReportingEndpoints::parse(
            r#"primary="https://reports.test/collect", backup="https://reports.test/backup""#,
        ),
        ReportingRetryPolicy::default(),
    )
}

#[test]
fn rejects_forged_report_endpoint_inside_otherwise_valid_resolved_batch() {
    let mut coordinator = coordinator();
    let endpoint_url = Url::parse("https://reports.test/collect").unwrap();
    let batch = ReportingDeliveryBatch {
        endpoint_url: endpoint_url.clone(),
        reports: vec![ResolvedIntegrityViolationReport {
            endpoint_name: "primary".into(),
            endpoint_url,
            report: report("backup"),
        }],
    };

    assert!(coordinator.queue_initial_batch(batch, 0, "ua").is_err());
    assert_eq!(coordinator.in_flight_len(), 0);
}

#[test]
fn accepts_report_whose_endpoint_matches_resolution_binding() {
    let mut coordinator = coordinator();
    let endpoint_url = Url::parse("https://reports.test/collect").unwrap();
    let batch = ReportingDeliveryBatch {
        endpoint_url: endpoint_url.clone(),
        reports: vec![ResolvedIntegrityViolationReport {
            endpoint_name: "primary".into(),
            endpoint_url,
            report: report("primary"),
        }],
    };

    assert!(coordinator.queue_initial_batch(batch, 0, "ua").is_ok());
    assert_eq!(coordinator.in_flight_len(), 1);
}

#[test]
fn normal_resolve_and_batch_output_remains_queueable() {
    let mut coordinator = coordinator();
    let reports = vec![report("primary"), report("backup")];
    let batches = coordinator.resolve_and_batch(&reports);
    assert_eq!(batches.len(), 2);

    for batch in batches {
        assert!(coordinator.queue_initial_batch(batch, 25, "ua").is_ok());
    }
    assert_eq!(coordinator.in_flight_len(), 2);
}
