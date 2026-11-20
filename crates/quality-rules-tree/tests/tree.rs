//! Expressions of the closed grammar become the neutral tree; anything else
//! is refused with its reason and place (decisions 0019, 0033 point 6).

use quality_rules_tree::{
    parse_expr, BinaryOp, Expr, Function, IntervalUnit, Literal, Reason, MAX_DEPTH, MAX_NODES,
};

fn field(name: &str) -> Box<Expr> {
    Box::new(Expr::Field(name.into()))
}

fn text(s: &str) -> Box<Expr> {
    Box::new(Expr::Literal(Literal::Text(s.into())))
}

fn reason(text: &str) -> Reason {
    parse_expr(text).expect_err(text).reason
}

#[test]
fn reads_the_decisions_expressions() {
    for e in [
        "ActividadLaboral = 'EMPLEADO'",
        "CodigoCiiu = 8430 AND ActividadLaboral <> 'JUBILADO/PENSIONADO'",
        "client_type = 'PN'",
        "years_between(FechaNacimientoCliente, :cut)",
        "days_between(FechaUltimaActualizacionCliente, :cut)",
        "FechaNacimiento + INTERVAL 18 YEAR",
        "1.5 * TotalIngresos",
        "is_empty(x) OR regexp_like(x, '3\\d{9}')",
        "in_list(ActividadLaboral, 'income_activities')",
        "in_reference(CodigoAsesor, 'advisors.CodigoAsesor')",
        "CASE WHEN a = 1 THEN 'x' ELSE 'y' END",
        "CAST(Monto AS DECIMAL(18, 2)) BETWEEN 0 AND 100",
        "Correo NOT LIKE '%@%'",
        "TipoOperación IN ('A', 'B') AND \"Campo con espacio\" IS NOT NULL",
        "FechaCorte > DATE '2024-01-31' -- a comment is dropped",
    ] {
        assert!(parse_expr(e).is_ok(), "{e}: {:?}", parse_expr(e));
    }
}

#[test]
fn builds_the_tree() {
    assert_eq!(
        parse_expr("ActividadLaboral = 'EMPLEADO'").unwrap(),
        Expr::Binary {
            op: BinaryOp::Eq,
            left: field("ActividadLaboral"),
            right: text("EMPLEADO")
        }
    );
    assert_eq!(
        parse_expr("FechaExpedicion < :cut").unwrap(),
        Expr::Binary {
            op: BinaryOp::Lt,
            left: field("FechaExpedicion"),
            right: Box::new(Expr::Cut)
        }
    );
    assert_eq!(
        parse_expr("FechaNacimiento + INTERVAL 18 YEAR").unwrap(),
        Expr::Binary {
            op: BinaryOp::Add,
            left: field("FechaNacimiento"),
            right: Box::new(Expr::Interval {
                amount: 18,
                unit: IntervalUnit::Year
            }),
        }
    );
    assert_eq!(
        parse_expr("IS_EMPTY(x)").unwrap(),
        Expr::Call {
            function: Function::IsEmpty,
            args: vec![Expr::Field("x".into())]
        }
    );
}

#[test]
fn keeps_literals_as_text() {
    assert_eq!(*text("it's"), parse_expr("'it''s'").unwrap());
    assert_eq!(*text("a\\b"), parse_expr("'a\\b'").unwrap());
    assert_eq!(*text("-- x /* y */"), parse_expr("'-- x /* y */'").unwrap());
    assert_eq!(
        Expr::Literal(Literal::Number("0123".into())),
        parse_expr("0123").unwrap()
    );
}

#[test]
fn refuses_a_second_statement() {
    assert_eq!(reason("1; DROP TABLE t"), Reason::TrailingInput);
    assert_eq!(reason("x = 1 x"), Reason::TrailingInput);
    let e = parse_expr("a = 1; SELECT 2").unwrap_err();
    assert_eq!(e.at, Some((1, 6)));
}

#[test]
fn refuses_subqueries_and_other_tables() {
    assert_eq!(reason("(SELECT 1)"), Reason::NotAllowed("subquery".into()));
    assert_eq!(
        reason("x IN (SELECT a FROM t)"),
        Reason::NotAllowed("subquery".into())
    );
    assert_eq!(
        reason("EXISTS (SELECT 1)"),
        Reason::NotAllowed("subquery".into())
    );
    assert_eq!(
        reason("t.a = 1"),
        Reason::NotAllowed("qualified name".into())
    );
}

#[test]
fn refuses_functions_outside_the_list() {
    for (e, name) in [
        ("read_csv('/etc/passwd')", "read_csv"),
        ("now()", "now"),
        ("count(x)", "count"),
        ("getenv('HOME')", "getenv"),
    ] {
        assert_eq!(reason(e), Reason::UnknownFunction(name.into()), "{e}");
    }
    assert!(matches!(
        reason("count(*)"),
        Reason::NotAllowed(_) | Reason::UnknownFunction(_)
    ));
    assert!(matches!(reason("upper(x) OVER ()"), Reason::NotAllowed(_)));
    assert!(matches!(reason("main.upper(x)"), Reason::NotAllowed(_)));
}

#[test]
fn checks_arguments() {
    assert_eq!(
        reason("upper(x, y)"),
        Reason::Arity {
            function: Function::Upper,
            got: 2
        }
    );
    assert_eq!(
        reason("coalesce(x)"),
        Reason::Arity {
            function: Function::Coalesce,
            got: 1
        }
    );
    assert_eq!(
        reason("regexp_like(x, y)"),
        Reason::TextRequired {
            function: Function::RegexpLike
        }
    );
    assert_eq!(
        reason("in_list(x, 'a' || 'b')"),
        Reason::TextRequired {
            function: Function::InList
        }
    );
    assert!(matches!(reason("x LIKE y"), Reason::NotAllowed(_)));
    assert!(matches!(
        reason("x LIKE 'a!%' ESCAPE '!'"),
        Reason::NotAllowed(_)
    ));
}

#[test]
fn refuses_what_could_change_a_query() {
    assert_eq!(reason(":other"), Reason::UnknownParameter(":other".into()));
    assert_eq!(reason("'a\u{0}b'"), Reason::ControlCharacter);
    assert_eq!(
        reason("CAST(x AS BLOB)"),
        Reason::UnsupportedType("BLOB".into())
    );
    assert_eq!(reason("x + INTERVAL '1' HOUR"), Reason::BadInterval);
    assert!(matches!(reason("x ILIKE 'a'"), Reason::NotAllowed(_)));
    // A Unicode quote is no quote: never a text that closes early.
    assert!(
        !matches!(parse_expr("x = \u{2019}a\u{2019}"), Ok(Expr::Binary { right, .. }) if matches!(*right, Expr::Literal(_)))
    );
}

#[test]
fn bounds_depth_and_size() {
    let deep = format!(
        "{}x{}",
        "(".repeat(MAX_DEPTH + 5),
        ")".repeat(MAX_DEPTH + 5)
    );
    assert!(matches!(reason(&deep), Reason::TooDeep | Reason::Syntax(_)));
    let wide = (0..MAX_NODES)
        .map(|i| format!("'{i}'"))
        .collect::<Vec<_>>()
        .join(", ");
    assert_eq!(reason(&format!("x IN ({wide})")), Reason::TooLarge);
}

#[test]
fn places_what_it_refuses() {
    let e = parse_expr("a = 1 AND b IN (SELECT c FROM t)").unwrap_err();
    assert_eq!(e.reason, Reason::NotAllowed("subquery".into()));
    assert_eq!(e.at.map(|(line, _)| line), Some(1));
    assert!(e.at.unwrap().1 >= 11, "{e:?}");
}

#[test]
fn reads_trim_in_its_plain_form_only() {
    assert_eq!(
        parse_expr("trim(x)").unwrap(),
        Expr::Call {
            function: Function::Trim,
            args: vec![Expr::Field("x".into())]
        }
    );
    assert!(matches!(
        parse_expr("trim(LEADING 'a' FROM x)").unwrap_err().reason,
        Reason::NotAllowed(_)
    ));
}
