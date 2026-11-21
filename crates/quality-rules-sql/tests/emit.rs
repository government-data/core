//! What the writer emits: names quoted, values escaped and checked, the cut
//! bound, and DuckDB's own spelling (decision 0033, point 6).

use quality_rules_sql::{expr, quote_ident, quote_text, Engine, Error, Table, Tables};
use quality_rules_tree::{parse_expr, Expr, Function, Literal};

fn sql(text: &str) -> String {
    expr(&parse_expr(text).unwrap(), Engine::DuckDb, &Tables::new())
        .unwrap()
        .text
}

#[test]
fn quotes_names_and_texts() {
    assert_eq!(
        quote_ident("Tipo \"raro\"").unwrap(),
        "\"Tipo \"\"raro\"\"\""
    );
    assert_eq!(quote_text("O'Brien").unwrap(), "'O''Brien'");
    assert_eq!(quote_text("a\\b").unwrap(), "'a\\b'");
    assert_eq!(quote_text("a\u{0}b"), Err(Error::ControlCharacter));
    assert_eq!(quote_ident("a\nb"), Err(Error::ControlCharacter));
    assert_eq!(
        sql("\"Campo con espacio\" = 'it''s'"),
        "(\"Campo con espacio\" = 'it''s')"
    );
}

#[test]
fn checks_numbers_and_dates_it_did_not_parse() {
    let number = |n: &str| {
        expr(
            &Expr::Literal(Literal::Number(n.into())),
            Engine::DuckDb,
            &Tables::new(),
        )
    };
    assert_eq!(number("12.5e-3").unwrap().text, "12.5e-3");
    assert_eq!(
        number("1; DROP TABLE t"),
        Err(Error::BadNumber("1; DROP TABLE t".into()))
    );
    let date = expr(
        &Expr::Literal(Literal::Date("2024-01-31' OR '1".into())),
        Engine::DuckDb,
        &Tables::new(),
    );
    assert!(matches!(date, Err(Error::BadDate(_))));
}

#[test]
fn binds_the_cut() {
    let s = expr(
        &parse_expr("a < :cut AND b > :cut - INTERVAL 13 MONTH").unwrap(),
        Engine::DuckDb,
        &Tables::new(),
    )
    .unwrap();
    assert_eq!(s.cut_params, 2);
    assert_eq!(
        s.text,
        "((\"a\" < CAST(? AS DATE)) AND (\"b\" > (CAST(? AS DATE) - INTERVAL '13' MONTH)))"
    );
}

#[test]
fn writes_duckdbs_own_spelling() {
    assert_eq!(
        sql("regexp_like(x, '3\\d{9}')"),
        "regexp_matches(CAST(\"x\" AS VARCHAR), '^(?:3\\d{9})$')"
    );
    let normalized = expr(
        &Expr::Normalized(Box::new(Expr::Field("d".into()))),
        Engine::DuckDb,
        &Tables::new(),
    )
    .unwrap()
    .text;
    assert!(normalized.ends_with("'', 'g'))"));
}

#[test]
fn writes_the_platforms_functions() {
    assert_eq!(
        sql("is_empty(x)"),
        "(\"x\" IS NULL OR trim(CAST(\"x\" AS VARCHAR), concat(' ', chr(160))) = '')"
    );
    assert_eq!(sql("x / 2"), "(CAST(\"x\" AS DOUBLE) / 2)");
    assert_eq!(sql("CAST(x AS INTEGER)"), "TRY_CAST(\"x\" AS BIGINT)");
    assert_eq!(sql("x LIKE 'a%'"), "(\"x\" LIKE 'a%' ESCAPE '\\')");
    assert!(sql("years_between(a, :cut)")
        .starts_with("(year(CAST(? AS DATE)) - year(\"a\") - CASE WHEN"));
    let dup = Expr::Duplicated(vec![Expr::Field("k".into())]);
    assert_eq!(
        expr(&dup, Engine::DuckDb, &Tables::new()).unwrap().text,
        "(count(*) OVER (PARTITION BY \"k\") > 1)"
    );
}

#[test]
fn a_reference_reads_the_table_the_platform_resolved() {
    let e = Expr::Call {
        function: Function::InReference,
        args: vec![
            Expr::Field("CodigoAsesor".into()),
            Expr::Literal(Literal::Text("advisors.Codigo".into())),
        ],
    };
    let mut tables = Tables::new();
    assert_eq!(
        expr(&e, Engine::DuckDb, &tables),
        Err(Error::UnknownReference("advisors.Codigo".into()))
    );
    tables.insert(
        "advisors".into(),
        Table {
            path: vec!["lake".into(), "advisors_silver".into(), "advisors".into()],
            filter: None,
        },
    );
    assert_eq!(
        expr(&e, Engine::DuckDb, &tables).unwrap().text,
        "(\"CodigoAsesor\" IN (SELECT \"Codigo\" FROM \"lake\".\"advisors_silver\".\"advisors\" WHERE \"Codigo\" IS NOT NULL))"
    );
}

#[test]
fn an_unexpanded_list_is_refused() {
    let e = Expr::Call {
        function: Function::InList,
        args: vec![
            Expr::Field("x".into()),
            Expr::Literal(Literal::Text("l".into())),
        ],
    };
    assert_eq!(
        expr(&e, Engine::DuckDb, &Tables::new()),
        Err(Error::NotExpanded("in_list"))
    );
    let empty = Expr::InList {
        expr: Box::new(Expr::Field("x".into())),
        list: vec![],
        negated: true,
    };
    assert_eq!(
        expr(&empty, Engine::DuckDb, &Tables::new()).unwrap().text,
        "TRUE"
    );
}

#[test]
fn a_reference_may_keep_only_some_records() {
    let e = parse_expr("in_reference(Codigo, 'activos.Codigo')").unwrap();
    let mut tables = Tables::new();
    let filter = Some(parse_expr("Estado = 'ACTIVO' AND Alta < :cut").unwrap());
    tables.insert(
        "activos".into(),
        Table {
            path: vec!["asesores".into()],
            filter,
        },
    );
    let s = expr(&e, Engine::DuckDb, &tables).unwrap();
    assert_eq!(
        s.text,
        "(\"Codigo\" IN (SELECT \"Codigo\" FROM \"asesores\" WHERE \"Codigo\" IS NOT NULL AND ((\"Estado\" = 'ACTIVO') AND (\"Alta\" < CAST(? AS DATE)))))"
    );
    assert_eq!(s.cut_params, 1);
}
