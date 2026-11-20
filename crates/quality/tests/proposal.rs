//! What a map job proposes (decision 0038), from synthetic profiles: the
//! mapping reads the table it profiled and parses, its types follow the
//! aggregates, and the rules it suggests lint against it with no error.

use quality::mapping::{Kind, Mapping, Priority, Semantic};
use quality::proposal::{propose, Alerts, ColumnProfile, Looks, Patterns, TableProfile};
use quality_rules::lint::{lint, Severity};

fn column(name: &str, declared: &str, empty: u64, distinct: u64, looks: Looks) -> ColumnProfile {
    ColumnProfile {
        name: name.into(),
        declared_type: declared.into(),
        nullable: empty > 0,
        primary_key: false,
        empty,
        distinct,
        min_length: Some(1),
        max_length: Some(20),
        looks,
    }
}

fn profile() -> TableProfile {
    let sampled = 1000;
    let mut id = column("client_id", "INTEGER", 0, sampled, Looks::default());
    id.primary_key = true;
    TableProfile {
        source: "core".into(),
        table: "public.clients".into(),
        row_estimate: Some(25_000),
        sampled,
        columns: vec![
            id,
            column(
                "document",
                "VARCHAR",
                0,
                990,
                Looks {
                    document: 1000,
                    number: 1000,
                    ..Looks::default()
                },
            ),
            column(
                "email",
                "VARCHAR",
                100,
                880,
                Looks {
                    email: 890,
                    ..Looks::default()
                },
            ),
            column(
                "mobile",
                "VARCHAR",
                50,
                940,
                Looks {
                    mobile: 950,
                    number: 950,
                    ..Looks::default()
                },
            ),
            column("opened", "DATE", 0, 300, Looks::default()),
            column(
                "balance",
                "DECIMAL(14,2)",
                0,
                700,
                Looks {
                    number: 1000,
                    ..Looks::default()
                },
            ),
            column(
                "active",
                "VARCHAR",
                0,
                2,
                Looks {
                    flag: 1000,
                    ..Looks::default()
                },
            ),
            column("branch", "VARCHAR", 0, 12, Looks::default()),
        ],
    }
}

fn alerts() -> Alerts {
    Alerts {
        required: "Empty".into(),
        unique: "Repeated".into(),
        email: "Not an email".into(),
        mobile: "Not a mobile".into(),
        document: "Not a document".into(),
    }
}

#[test]
fn a_profile_proposes_a_mapping_of_its_table_with_types_from_its_aggregates() {
    let p = propose("clients", &profile(), &Patterns::default(), &alerts());
    let m = Mapping::parse(&p.mapping).unwrap();
    let from = m.from.unwrap().value;
    assert_eq!(
        (from.source.as_str(), from.table.as_str()),
        ("core", "public.clients")
    );
    assert_eq!(
        (m.dataset.value.as_str(), m.source.value.as_str()),
        ("clients", "clients")
    );
    assert_eq!(m.key, ["client_id"]);
    let kind = |f: &str| (m.fields[f].value.kind, m.fields[f].value.semantic);
    assert_eq!(kind("document"), (Kind::Text, Some(Semantic::Document)));
    assert_eq!(kind("email"), (Kind::Text, Some(Semantic::Email)));
    assert_eq!(kind("mobile"), (Kind::Text, Some(Semantic::Mobile)));
    assert_eq!(kind("opened"), (Kind::Date, None));
    assert_eq!(kind("balance"), (Kind::Number, Some(Semantic::Amount)));
    assert_eq!(kind("active"), (Kind::Text, Some(Semantic::Flag)));
    assert_eq!(kind("branch"), (Kind::Text, Some(Semantic::Code)));
    assert_eq!(m.fields["client_id"].value.column, "client_id");
    // Roles from names and counts, for a steward to confirm.
    let roles = |f: &str| {
        m.fields[f]
            .value
            .roles
            .iter()
            .map(|r| r.name())
            .collect::<Vec<_>>()
    };
    assert_eq!(roles("document"), ["entity"]);
    assert_eq!(roles("balance"), ["outlier_amount"]);
    assert_eq!(roles("branch"), ["dimension"]);
    assert!(roles("opened").is_empty() && roles("client_id").is_empty());
    // Contact expires, a document does not; priorities are a steward's.
    let field = |f: &str| &m.fields[f].value;
    assert!(field("email").expires && field("mobile").expires);
    assert!(!field("document").expires && !field("opened").expires);
    assert_eq!(field("email").priority, Priority::Medium);
}

#[test]
fn its_rules_lint_against_its_mapping_with_no_error_each_in_its_fields_file() {
    let p = propose("clients", &profile(), &Patterns::default(), &alerts());
    let schema = Mapping::parse(&p.mapping).unwrap().schema();
    let files: Vec<&str> = p.rules.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(
        files,
        [
            "client_id.yaml",
            "document.yaml",
            "email.yaml",
            "mobile.yaml",
            "opened.yaml",
            "balance.yaml",
            "active.yaml",
            "branch.yaml"
        ],
        "a file a field with rules, in the columns' order"
    );
    for (file, yaml) in &p.rules {
        let report = lint(yaml, &schema);
        let errors: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.severity() == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{errors:?}\n{yaml}");
        let elsewhere = quality_rules::lint::placement(file, yaml, &report);
        assert!(elsewhere.is_empty(), "{file}: {elsewhere:?}");
    }
    let yaml = |file: &str| &p.rules.iter().find(|(f, _)| f == file).unwrap().1;
    // Required where never empty, unique for the key, formats by meaning.
    for (file, expected) in [
        (
            "client_id.yaml",
            "field: \"client_id\"\n    check: \"required\"",
        ),
        (
            "client_id.yaml",
            "field: \"client_id\"\n    check: \"unique\"",
        ),
        (
            "document.yaml",
            "field: \"document\"\n    check: \"required\"",
        ),
        ("email.yaml", "field: \"email\"\n    check: \"format\""),
        ("mobile.yaml", "field: \"mobile\"\n    check: \"format\""),
    ] {
        assert!(
            yaml(file).contains(expected),
            "missing {expected:?} in {file}"
        );
    }
    assert!(
        !yaml("email.yaml").contains("check: \"required\""),
        "it has empties"
    );
}

#[test]
fn many_columns_give_a_file_a_field() {
    let mut big = profile();
    big.columns = (0..40)
        .map(|i| column(&format!("c{i}"), "VARCHAR", 0, 5, Looks::default()))
        .collect();
    for c in &mut big.columns {
        c.nullable = false;
    }
    let p = propose("wide", &big, &Patterns::default(), &alerts());
    assert_eq!(
        p.rules.len(),
        40,
        "one required rule a column, each its own file"
    );
    assert_eq!(p.rules[0].0, "c0.yaml");
    assert!(p.mapping.contains("  c0: { column: \"c0\""));
}

#[test]
fn an_installations_patterns_are_the_ones_its_maps_profile_and_propose() {
    // Another country's mobile, and the rest as they are.
    let patterns: Patterns = serde_json::from_str(r#"{ "mobile": "9\\d{8}" }"#).unwrap();
    assert_eq!(patterns.mobile, r"9\d{8}");
    assert_eq!(patterns.email, Patterns::default().email);
    assert!(
        serde_json::from_str::<Patterns>(r#"{ "phone": "x" }"#).is_err(),
        "unknown kind"
    );

    // A job carries its patterns; without them, the defaults.
    let job = serde_json::json!({ "patterns": patterns });
    assert_eq!(Patterns::of_job(&job), patterns);
    assert_eq!(
        Patterns::of_job(&serde_json::json!({})),
        Patterns::default()
    );

    let p = propose("clients", &profile(), &patterns, &alerts());
    let (_, yaml) = p.rules.iter().find(|(f, _)| f == "mobile.yaml").unwrap();
    assert!(yaml.contains(r#"pattern: "9\\d{8}""#), "{yaml}");
}
