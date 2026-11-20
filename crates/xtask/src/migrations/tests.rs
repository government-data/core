//! The migration rule's cases, and the repository's migrations against it.

use super::*;

fn reasons_of(sql: &str) -> Vec<&'static str> {
    check("0042_x.sql", sql)
        .into_iter()
        .map(|h| h.reason)
        .collect()
}

#[test]
fn adding_is_accepted() {
    let sql = "
        CREATE TABLE system_updates (id UUID PRIMARY KEY, phase TEXT NOT NULL);
        ALTER TABLE flows ADD COLUMN code_sha256 TEXT;
        ALTER TABLE flows ADD COLUMN update_mode TEXT NOT NULL DEFAULT 'auto';
        ALTER TABLE flows ADD CONSTRAINT x CHECK (update_mode IN ('auto', 'paused'));
        CREATE INDEX system_updates_phase_idx ON system_updates (phase);
        ALTER TABLE t ADD COLUMN g INT NOT NULL GENERATED ALWAYS AS (1) STORED;
    ";
    assert_eq!(reasons_of(sql), Vec::<&str>::new());
}

#[test]
fn destructive_changes_are_rejected() {
    for (sql, reason) in [
        ("DROP TABLE x", "drops a table"),
        ("ALTER TABLE x DROP COLUMN y", "drops a column"),
        (
            "alter table x rename column a to b",
            "renames (the previous version keeps using the old name)",
        ),
        (
            "ALTER TABLE x RENAME TO y",
            "renames (the previous version keeps using the old name)",
        ),
        (
            "ALTER TABLE x ALTER COLUMN y TYPE BIGINT",
            "changes a column's type",
        ),
        (
            "ALTER TABLE x ALTER COLUMN y SET DATA TYPE BIGINT",
            "changes a column's type",
        ),
        (
            "ALTER TABLE x ALTER COLUMN y SET NOT NULL",
            "requires a value the previous version does not write",
        ),
        (
            "DROP INDEX x_idx",
            "drops an index (an ON CONFLICT of the previous version may depend on it)",
        ),
        ("TRUNCATE x", "empties a table"),
    ] {
        assert_eq!(reasons_of(sql), vec![reason], "{sql}");
    }
}

#[test]
fn a_trigger_on_truncate_empties_nothing() {
    let guard = "CREATE TRIGGER g BEFORE UPDATE OR DELETE OR TRUNCATE ON t
        FOR EACH STATEMENT EXECUTE FUNCTION refuse()";
    assert!(reasons_of(guard).is_empty());
    assert_eq!(
        reasons_of("CREATE TABLE a (b INT); truncate table t"),
        vec!["empties a table"]
    );
}

#[test]
fn a_check_replaced_in_the_same_statement_is_accepted() {
    let widened = "ALTER TABLE jobs DROP CONSTRAINT jobs_kind_check,
        ADD CONSTRAINT jobs_kind_check CHECK (kind IN ('map', 'tables'))";
    assert!(reasons_of(widened).is_empty());
    let dropped = "drops a constraint (an ON CONFLICT of the previous version may depend on it)";
    for sql in [
        "ALTER TABLE jobs DROP CONSTRAINT jobs_kind_check",
        "ALTER TABLE jobs DROP CONSTRAINT IF EXISTS k, ADD CONSTRAINT other CHECK (true)",
        "ALTER TABLE jobs DROP CONSTRAINT k; ALTER TABLE jobs ADD CONSTRAINT k CHECK (true)",
        "ALTER TABLE jobs DROP CONSTRAINT k, ADD CONSTRAINT k UNIQUE (kind)",
    ] {
        assert_eq!(reasons_of(sql), vec![dropped], "{sql}");
    }
}

#[test]
fn a_required_column_without_a_value_is_rejected() {
    assert_eq!(
        reasons_of("ALTER TABLE x ADD COLUMN y TEXT, ADD COLUMN z TEXT NOT NULL"),
        vec!["adds a NOT NULL column without DEFAULT (the previous version's INSERTs fail)"]
    );
    // A new table has no old INSERTs to fail.
    assert!(reasons_of("CREATE TABLE x (y TEXT NOT NULL)").is_empty());
}

#[test]
fn comments_literals_and_functions_do_not_count() {
    let sql = "
        -- this used to DROP TABLE x
        /* DROP COLUMN y */
        INSERT INTO notes (text) VALUES ('we must DROP TABLE x; later');
        CREATE FUNCTION f() RETURNS trigger AS $$
        BEGIN
          DROP TABLE IF EXISTS tmp;
          RETURN NEW;
        END $$ LANGUAGE plpgsql;
        CREATE FUNCTION g() RETURNS void AS $body$ TRUNCATE t $body$ LANGUAGE sql;
    ";
    assert_eq!(reasons_of(sql), Vec::<&str>::new());
}

#[test]
fn marked_as_contraction_is_accepted() {
    let sql = "-- contract: v1.2.0 no longer uses it\nALTER TABLE x DROP COLUMN y;";
    assert!(reasons_of(sql).is_empty());
}

#[test]
fn each_finding_says_which_statement() {
    let h = check("0050_x.sql", "CREATE TABLE a (b INT); DROP TABLE c;");
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].statement, 2);
    assert_eq!(h[0].to_string(), "0050_x.sql (statement 2): drops a table");
}

#[test]
fn the_repository_migrations_comply() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../infrastructure/migrations");
    let findings = check_dir(&dir).unwrap();
    assert!(findings.is_empty(), "{findings:#?}");
}
