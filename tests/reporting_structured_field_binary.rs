use browser_engine::ReportingEndpoints;

#[test]
fn binary_parameter_accepts_padded_and_unpadded_base64() {
    let padded = ReportingEndpoints::parse(
        r#"default="https://reports.test/collect";digest=:YWI=:"#,
    );
    assert_eq!(padded.len(), 1);

    let unpadded = ReportingEndpoints::parse(
        r#"default="https://reports.test/collect";digest=:YWI:"#,
    );
    assert_eq!(unpadded.len(), 1);
}

#[test]
fn binary_parameter_rejects_impossible_or_misplaced_padding() {
    for value in [
        r#"default="https://reports.test/collect";digest=:A:"#,
        r#"default="https://reports.test/collect";digest=:Y=WI:"#,
        r#"default="https://reports.test/collect";digest=:YWI==:"#,
        r#"default="https://reports.test/collect";digest=:====:"#,
    ] {
        assert!(ReportingEndpoints::parse(value).is_empty(), "{value}");
    }
}

#[test]
fn malformed_binary_parameter_invalidates_the_whole_dictionary() {
    let endpoints = ReportingEndpoints::parse(
        r#"primary="https://reports.test/a";digest=:A:, backup="https://reports.test/b""#,
    );
    assert!(endpoints.is_empty());
}
