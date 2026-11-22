//! `server timezone [set NAME]` (decision 0079): the platform's time zone,
//! the one of the update window and of new schedules. Without arguments it
//! prints it (UTC when it was never set); `set NAME` validates an IANA name
//! (`America/Bogota`, `UTC`) and saves it, recorded as done by this command,
//! with no person signed in. On an installation:
//!
//!   docker compose exec api server timezone set America/Bogota
//!
//! It reads the database of `DATABASE_URL` and runs its migrations, as the
//! other commands. The running API reads the new zone where it is used (the
//! update window, new schedules); its Settings page shows it after a
//! restart.

use std::sync::Arc;

use application::{PlatformTimezone, SettingsError};
use infrastructure::{PgAuditRepository, PgConfig, PgSettingsStore};

use crate::settings_wiring::Store;

type CommandResult = Result<(), Box<dyn std::error::Error>>;

const USAGE: &str = "usage: server timezone [set NAME]";

/// What the audit records as who changed it.
const SET_COMMAND: &str = "server timezone set";

/// What the arguments ask for.
#[derive(Debug, PartialEq, Eq)]
enum Ask {
    Show,
    Set(String),
}

pub(crate) async fn run() -> CommandResult {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let ask = parse(&args)?;
    let pool = infrastructure::connect(&PgConfig::from_env()?).await?;
    infrastructure::run_migrations(&pool).await?;
    let timezone = PlatformTimezone::new(
        Arc::new(Store(PgSettingsStore::new(pool.clone()))),
        Arc::new(PgAuditRepository::new(pool)),
    );
    let shown = match ask {
        Ask::Show => timezone.get().await?,
        Ask::Set(name) => texts::within(texts::Language::En, timezone.set(&name, SET_COMMAND))
            .await
            .map_err(refusal)?,
    };
    println!("{shown}");
    Ok(())
}

fn parse(args: &[String]) -> Result<Ask, &'static str> {
    match args {
        [] => Ok(Ask::Show),
        [set, name] if set == "set" && !name.trim().is_empty() => Ok(Ask::Set(name.clone())),
        _ => Err(USAGE),
    }
}

/// The refusal as one line for the person at the terminal.
fn refusal(e: SettingsError) -> Box<dyn std::error::Error> {
    match e {
        SettingsError::Invalid(fields) => fields
            .into_iter()
            .map(|(field, why)| format!("{field}: {why}"))
            .collect::<Vec<_>>()
            .join("; ")
            .into(),
        other => other.to_string().into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn it_shows_or_sets_and_nothing_else() {
        assert_eq!(parse(&args(&[])), Ok(Ask::Show));
        assert_eq!(
            parse(&args(&["set", "America/Bogota"])),
            Ok(Ask::Set("America/Bogota".into()))
        );
        assert_eq!(parse(&args(&["set"])), Err(USAGE));
        assert_eq!(parse(&args(&["set", " "])), Err(USAGE));
        assert_eq!(parse(&args(&["get"])), Err(USAGE));
        assert_eq!(parse(&args(&["set", "UTC", "extra"])), Err(USAGE));
    }
}
