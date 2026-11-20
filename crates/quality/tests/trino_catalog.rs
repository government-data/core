//! A source on Trino as its catalog (decision 0048, point 1): its name, its
//! statements, values quoted, and nothing that could add a property to the
//! JDBC URL. Synthetic values only.

use std::collections::BTreeMap;

use quality::source::Source;
use quality::trino_catalog::{catalog_name, create_catalog, drop_catalog, CatalogError};

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn sqlserver(fields: &[(&str, &str)]) -> Source {
    Source {
        name: "core".into(),
        driver: "sqlserver".into(),
        fields: map(fields),
        secrets: map(&[("user", "reader"), ("password", "it's-synthetic")]),
    }
}

#[test]
fn a_sql_server_source_is_a_catalog_with_its_url_and_credentials() {
    let source = sqlserver(&[
        ("host", "db.example"),
        ("database", "core"),
        ("options", "encrypt=false"),
    ]);
    assert_eq!(catalog_name("core"), "src_core");
    assert_eq!(
        create_catalog(&source).unwrap(),
        "CREATE CATALOG src_core USING sqlserver WITH (\"connection-url\" = 'jdbc:sqlserver://db.example:1433;databaseName=core;encrypt=false', \"connection-user\" = 'reader', \"connection-password\" = 'it''s-synthetic')"
    );
    assert_eq!(drop_catalog("core"), "DROP CATALOG IF EXISTS src_core");
}

#[test]
fn an_oracle_source_names_its_service() {
    let source = Source {
        name: "ledger".into(),
        driver: "oracle".into(),
        fields: map(&[
            ("host", "ora.example"),
            ("port", "1600"),
            ("service", "LEDGER"),
        ]),
        secrets: map(&[("user", "reader"), ("password", "synthetic")]),
    };
    let sql = create_catalog(&source).unwrap();
    assert!(
        sql.contains("'jdbc:oracle:thin:@//ora.example:1600/LEDGER'"),
        "{sql}"
    );
}

#[test]
fn what_cannot_be_a_catalog_is_said() {
    let smuggled = sqlserver(&[("host", "db.example"), ("database", "core;user=sa")]);
    assert_eq!(
        create_catalog(&smuggled),
        Err(CatalogError::Field("database".into()))
    );
    let no_host = sqlserver(&[("database", "core")]);
    assert_eq!(
        create_catalog(&no_host),
        Err(CatalogError::Field("host".into()))
    );
    let duckdb = Source {
        driver: "postgres".into(),
        ..sqlserver(&[("host", "db.example"), ("database", "core")])
    };
    assert_eq!(create_catalog(&duckdb), Err(CatalogError::NotOnTrino));
}
