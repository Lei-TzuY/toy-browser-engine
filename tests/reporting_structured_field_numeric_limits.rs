use browser_engine::ReportingEndpoints;

#[test]
fn integer_parameter_accepts_fifteen_digits_and_rejects_sixteen() {
    let valid = ReportingEndpoints::parse(
        r#"default="https://reports.test/collect";quota=999999999999999"#,
    );
    assert_eq!(valid.len(), 1);

    let invalid = ReportingEndpoints::parse(
        r#"default="https://reports.test/collect";quota=9999999999999999, backup="https://reports.test/backup""#,
    );
    assert!(invalid.is_empty());
}

#[test]
fn negative_integer_limit_excludes_the_sign() {
    let valid = ReportingEndpoints::parse(
        r#"default="https://reports.test/collect";quota=-999999999999999"#,
    );
    assert_eq!(valid.len(), 1);

    let invalid = ReportingEndpoints::parse(
        r#"default="https://reports.test/collect";quota=-9999999999999999"#,
    );
    assert!(invalid.is_empty());
}

#[test]
fn decimal_parameter_enforces_twelve_integer_and_three_fraction_digits() {
    let valid = ReportingEndpoints::parse(
        r#"default="https://reports.test/collect";weight=999999999999.999"#,
    );
    assert_eq!(valid.len(), 1);

    let too_many_integer_digits = ReportingEndpoints::parse(
        r#"default="https://reports.test/collect";weight=9999999999999.1"#,
    );
    assert!(too_many_integer_digits.is_empty());

    let too_many_fraction_digits = ReportingEndpoints::parse(
        r#"default="https://reports.test/collect";weight=1.2345"#,
    );
    assert!(too_many_fraction_digits.is_empty());
}

#[test]
fn numeric_limit_failure_invalidates_the_whole_structured_dictionary() {
    let endpoints = ReportingEndpoints::parse(
        r#"primary="https://reports.test/a";sample=1.0000, backup="https://reports.test/b""#,
    );
    assert!(endpoints.is_empty());
}
