//! `server audit verify` (decision 0070, point 11): every tenant's audit
//! chain, or one tenant's, recomputed from its rows; a line per tenant with
//! the rows and numbers checked, the head, where adoption ended and the
//! first break by its number and kind, never a value. Each run is kept in
//! `audit_chain_checks`; a break is logged at error level and the command
//! exits 1.
//!
//! ```text
//! server audit verify [--tenant ID] [--lang es|en]
//! ```

use domain::audit_chain_check::ChainReport;
use infrastructure::{PgAuditChain, PgConfig};
use texts::{text_in, Language};
use uuid::Uuid;

use crate::init_tracing;

type Failure = Box<dyn std::error::Error>;

pub(crate) async fn run() -> Result<(), Failure> {
    let (lang, tenant) = options(std::env::args().skip(2).collect())?;
    init_tracing();
    let pool = infrastructure::connect(&PgConfig::from_env()?).await?;
    infrastructure::run_migrations(&pool).await?;
    let chain = PgAuditChain::new(pool);
    let reports = application::audit_chain::verify_all(&chain, &chain, tenant.as_deref()).await?;
    for report in &reports {
        println!("{}", line(lang, report));
    }
    if reports.iter().any(|r| r.first_break.is_some()) {
        return Err(text_in(lang, "audit.chain.broken", &[]).into());
    }
    Ok(())
}

/// `verify`, then `--tenant ID` and `--lang es|en`, each at most once.
fn options(args: Vec<String>) -> Result<(Language, Option<String>), Failure> {
    let (mut lang, mut tenant) = (Language::Es, None);
    let usage = |lang| -> Failure { text_in(lang, "audit.chain.usage", &[]).into() };
    let mut args = args.into_iter();
    if args.next().as_deref() != Some("verify") {
        return Err(usage(lang));
    }
    while let Some(flag) = args.next() {
        let value = args.next().ok_or_else(|| usage(lang))?;
        match flag.as_str() {
            "--lang" => lang = Language::parse(&value).ok_or_else(|| usage(lang))?,
            "--tenant" => {
                let id = Uuid::parse_str(&value).map_err(|_| usage(lang))?;
                tenant = Some(id.to_string());
            }
            _ => return Err(usage(lang)),
        }
    }
    Ok((lang, tenant))
}

/// A tenant's report in one line.
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
            ("tenant", r.tenant.clone()),
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
        let tenant = "00000000-0000-0000-0000-000000000001";
        let (lang, only) = options(args(&["verify", "--lang", "en", "--tenant", tenant])).unwrap();
        assert_eq!((lang, only.as_deref()), (Language::En, Some(tenant)));
        assert_eq!(options(args(&["verify"])).unwrap(), (Language::Es, None));
        for bad in [
            &["check"][..],
            &["verify", "--tenant", "x"],
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
            "t: 4 rows checked, 1 to 4; head 6 ab01; adopted up to 2: first break at 5, gap"
        );
        assert_eq!(
            line(Language::Es, &report),
            "t: 4 filas revisadas, de 1 a 4; cabeza 6 ab01; adoptadas hasta 2: primera ruptura en 5, gap"
        );
    }
}
