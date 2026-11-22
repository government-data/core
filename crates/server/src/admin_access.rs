//! `server admin-access --email ADDRESS [--force]` (decision 0086): the
//! administrator's entry from the server, with no password anywhere. With no
//! administrator it creates `ADDRESS`'s account pending activation; it
//! prints a one-time link to choose the password (mailed too when outgoing
//! mail is set). For an account that already chose its password, a link
//! only with `--force` (it was lost). On-premise only.
//!
//! The link goes to standard output and nowhere else; nothing logs the
//! address. Audited as the internal route is.

use std::sync::Arc;
use std::time::SystemTime;

use api::internal::AdminAccessRoute;
use application::Edition;
use auth_gateway::{AccessError, AdminAccess, LoginService};
use domain::{AuditEventType, NewAuditEvent};
use infrastructure::auth_config::AuthConfig;
use infrastructure::{
    Argon2Hasher, PgAdminRepository, PgAuditRepository, PgConfig, PgIpLockoutStore, SecretBox,
    SystemClock,
};

use crate::serve_services::{password_reset, settings, settings_key_file};

const USAGE: &str = "usage: server admin-access --email ADDRESS [--force]";

/// What the audit records as the user agent.
const USER_AGENT: &str = "server admin-access";

type Failure = Box<dyn std::error::Error>;

/// The command's arguments: the address and whether to force a link.
#[derive(Debug, PartialEq, Eq)]
struct Args {
    email: String,
    force: bool,
}

fn parse(args: &[String]) -> Result<Args, &'static str> {
    let (mut email, mut force) = (None, false);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--email" => email = Some(args.next().ok_or(USAGE)?.clone()),
            "--force" => force = true,
            _ => return Err(USAGE),
        }
    }
    Ok(Args {
        email: email.filter(|e| !e.trim().is_empty()).ok_or(USAGE)?,
        force,
    })
}

pub(crate) async fn run() -> Result<(), Failure> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let args = parse(&args)?;
    crate::init_tracing();
    let edition = Edition::parse(&std::env::var("ORQ_EDITION").unwrap_or_default())
        .map_err(|e| format!("ORQ_EDITION {e}"))?;
    let route = route(edition).await?;
    let granted = route
        .service
        .admin_access(&args.email, &route.link_base, args.force)
        .await
        .map_err(refusal)?;
    audit(&route, &granted, &args.email).await;
    println!("{}", outcome(&granted));
    Ok(())
}

/// The service and the audit, built as `serve` builds them.
async fn route(edition: Edition) -> Result<AdminAccessRoute, Failure> {
    let auth = AuthConfig::from_env()?;
    let pool = infrastructure::connect(&PgConfig::from_env()?).await?;
    infrastructure::run_migrations(&pool).await?;
    crate::modules::migrate(&crate::modules::registry(), &pool).await?;
    let admins: Arc<dyn auth_gateway::AdminRepository> =
        Arc::new(PgAdminRepository::new(pool.clone()));
    let login = LoginService::new(
        admins.clone(),
        Arc::new(PgIpLockoutStore::new(pool.clone())),
        Arc::new(Argon2Hasher::new(auth.argon2)?),
        Arc::new(SystemClock),
        auth.lockout_policy,
    );
    let secret_box = SecretBox::from_env_or_file(settings_key_file())?;
    let (_, _, settings) = settings(&pool, &auth, &secret_box, &login).await?;
    let audit = auth_gateway::AuditService::new(Arc::new(PgAuditRepository::new(pool.clone())));
    let (_, _, route) = password_reset(&pool, &admins, &auth, &settings, &audit, edition)?;
    route.ok_or_else(|| "admin-access is for an on-premise installation".into())
}

fn refusal(e: AccessError) -> Failure {
    match e {
        AccessError::InvalidEmail(why) => format!("--email is not an address: {why}").into(),
        AccessError::UnknownAccount => {
            "there are administrators already and none has that address".into()
        }
        AccessError::DirectoryAccount => {
            "that account's password is in the directory (LDAP / AD)".into()
        }
        AccessError::Internal(e) => e.into(),
    }
}

async fn audit(route: &AdminAccessRoute, granted: &AdminAccess, email: &str) {
    if !granted.created && granted.link.is_none() {
        return;
    }
    let event = NewAuditEvent {
        event_type: if granted.created {
            AuditEventType::UserCreated
        } else {
            AuditEventType::ActivationSent
        },
        admin_id: Some(granted.admin.clone()),
        email_attempted: Some(email.trim().to_ascii_lowercase()),
        ip: "127.0.0.1".into(),
        user_agent: Some(USER_AGENT.into()),
        occurred_at: SystemTime::now(),
    };
    if let Err(e) = route.audit.record(event).await {
        tracing::warn!("admin access: failed to record audit event: {e}");
    }
    tracing::warn!(
        created = granted.created,
        emailed = granted.emailed,
        "admin access link issued from the server"
    );
}

/// What the command prints: the link, or why there is none.
fn outcome(granted: &AdminAccess) -> String {
    let Some(link) = &granted.link else {
        return "the account already chose its password: --force issues a link to choose another"
            .into();
    };
    let what = if granted.created {
        "administrator created, pending: choose the password at"
    } else {
        "choose the password at"
    };
    let mailed = if granted.emailed {
        " (also sent by e-mail)"
    } else {
        ""
    };
    format!("{what}{mailed}\n{link}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_the_address_and_force() {
        let parsed = parse(&args(&["--email", "admin@example.com"])).unwrap();
        assert_eq!(parsed.email, "admin@example.com");
        assert!(!parsed.force);
        let forced = parse(&args(&["--force", "--email", "admin@example.com"])).unwrap();
        assert!(forced.force);
    }

    #[test]
    fn refuses_no_address_and_unknown_arguments() {
        assert_eq!(parse(&args(&[])), Err(USAGE));
        assert_eq!(parse(&args(&["--email"])), Err(USAGE));
        assert_eq!(parse(&args(&["--email", " "])), Err(USAGE));
        assert_eq!(
            parse(&args(&["--email", "a@example.com", "--x"])),
            Err(USAGE)
        );
    }

    #[test]
    fn prints_the_link_or_why_there_is_none() {
        let granted = |created, link: Option<&str>, emailed| AdminAccess {
            admin: domain::AdminId::new("00000000-0000-0000-0000-000000000001"),
            created,
            link: link.map(str::to_string),
            emailed,
        };
        let created = outcome(&granted(true, Some("https://q.example/a#t"), false));
        assert!(created.starts_with("administrator created"), "{created}");
        assert!(created.ends_with("\nhttps://q.example/a#t"), "{created}");
        let mailed = outcome(&granted(false, Some("https://q.example/r#t"), true));
        assert!(mailed.contains("also sent by e-mail"), "{mailed}");
        let none = outcome(&granted(false, None, false));
        assert!(none.contains("--force"), "{none}");
    }
}
