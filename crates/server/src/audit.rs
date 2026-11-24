//! `server audit verify` (decision 0070, point 11): the installation's audit
//! chain (decision 0076: one), recomputed from its rows; a line with the
//! rows and numbers checked, the head, where adoption ended and the
//! first break by its number and kind, never a value. Each run is kept in
//! `audit_chain_checks`; a break is logged at error level and the command
//! exits 1.
//!
//! ```text
//! server audit verify [--lang es|en]
//! ```

use domain::audit_chain_check::ChainReport;
use infrastructure::{PgAuditChain, PgConfig};
use texts::{text_in, Language};

use crate::init_tracing;

type Failure = Box<dyn std::error::Error>;

pub(crate) async fn run() -> Result<(), Failure> {
    let lang = options(std::env::args().skip(2).collect())?;
    init_tracing();
    let pool = infrastructure::connect(&PgConfig::from_env()?).await?;
    infrastructure::run_migrations(&pool).await?;
    let chain = PgAuditChain::new(pool);
    let reports = application::audit_chain::verify_all(&chain, &chain, None).await?;
    for report in &reports {
        println!("{}", line(lang, report));
    }
    if reports.iter().any(|r| r.first_break.is_some()) {
        return Err(text_in(lang, "audit.chain.broken", &[]).into());
    }
    Ok(())
}

/// `verify`, then `--lang es|en` at most once.
fn options(args: Vec<String>) -> Result<Language, Failure> {
    let mut lang = Language::Es;
    let usage = |lang| -> Failure { text_in(lang, "audit.chain.usage", &[]).into() };
    let mut args = args.into_iter();
    if args.next().as_deref() != Some("verify") {
        return Err(usage(lang));
    }
    while let Some(flag) = args.next() {
        let value = args.next().ok_or_else(|| usage(lang))?;
        match flag.as_str() {
            "--lang" => lang = Language::parse(&value).ok_or_else(|| usage(lang))?,
            _ => return Err(usage(lang)),
        }
    }
    Ok(lang)
}

/// The chain's report in one line.
fn line(lang: Language, r: &ChainReport) -> String {
    let number = |n: Option<i64>| n.map_or_else(|| "—".to_string(), |n| n.to_string());
    let hash = r.head.as_ref().map_or_else(
        || "—".to_string(),
        |(_, h)| h.iter().map(|b| format!("{b:02x}")).collect(),
    );
    let checked = text_in(
        lang,
        "audit.chain.report",
        &[
            ("count", r.rows_checked.to_string()),
            ("first", number(r.first_seq)),
            ("last", number(r.last_seq)),
            ("head", number(r.head.as_ref().map(|(seq, _)| *seq))),
            ("hash", hash),
            ("adopted", number(r.adopted_seq)),
        ],
    );
    let result = match r.first_break {
        None => text_in(lang, "audit.chain.unbroken", &[]),
        Some(b) => text_in(
            lang,
            "audit.chain.break",
            &[("seq", b.seq.to_string()), ("kind", b.kind.as_str().into())],
        ),
    };
    format!("{checked}: {result}")
}

#[cfg(test)]
mod tests {
    use domain::audit_chain_check::{BreakKind, ChainBreak};

    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn reads_the_options() {
        let lang = options(args(&["verify", "--lang", "en"])).unwrap();
        assert_eq!(lang, Language::En);
        assert_eq!(options(args(&["verify"])).unwrap(), Language::Es);
        for bad in [
            &["check"][..],
            &["verify", "--tenant", "00000000-0000-0000-0000-000000000001"],
            &["verify", "--lang"],
        ] {
            assert!(options(args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_break_is_its_number_and_kind_in_both_languages() {
        let report = ChainReport {
            tenant: "t".into(),
            rows_checked: 4,
            first_seq: Some(1),
            last_seq: Some(4),
            head: Some((6, vec![0xab, 0x01])),
            adopted_seq: Some(2),
            first_break: Some(ChainBreak {
                seq: 5,
                kind: BreakKind::Gap,
            }),
        };
        assert_eq!(
            line(Language::En, &report),
            "4 rows checked, 1 to 4; head 6 ab01; adopted up to 2: first break at 5, gap"
        );
        assert_eq!(
            line(Language::Es, &report),
            "4 filas revisadas, de 1 a 4; cabeza 6 ab01; adoptadas hasta 2: primera ruptura en 5, gap"
        );
    }
}
