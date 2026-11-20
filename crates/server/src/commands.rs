//! The one-shot commands: they do their job and exit, without serving.

use std::path::Path;
use std::sync::Arc;

use auth_gateway::{BootstrapOutcome, BootstrapService};
use infrastructure::auth_config::AuthConfig;
use infrastructure::ca::{self, Ca};
use infrastructure::{Argon2Hasher, PgAdminRepository, PgConfig};

use crate::init_tracing;

type CommandResult = Result<(), Box<dyn std::error::Error>>;

/// `server license-keygen`: a key pair to sign licenses. The private one is
/// kept outside every repository; the public one is compiled into the binary
/// (`ORQ_LICENSE_PUBKEY=<public> cargo build ...`).
pub(crate) fn license_keygen() -> CommandResult {
    let (private, public) = infrastructure::license::generate_keypair()?;
    println!("private (PKCS#8, base64; keep it secret): {private}");
    println!("public (ORQ_LICENSE_PUBKEY): {public}");
    Ok(())
}

/// `server license-sign PRIVATE_KEY PERMIT.json`: sign a permit (an
/// Entitlement in JSON) and print the signed permit.
pub(crate) fn license_sign() -> CommandResult {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let [key_file, permit_file] = args.as_slice() else {
        return Err("usage: server license-sign PRIVATE_KEY PERMIT.json".into());
    };
    let private = std::fs::read_to_string(key_file)?;
    let entitlement: license_model::Entitlement =
        serde_json::from_str(&std::fs::read_to_string(permit_file)?)?;
    println!(
        "{}",
        infrastructure::license::sign_permit(&entitlement, &private)?
    );
    Ok(())
}

/// `server ca-init --name INSTALLATION --cert PATH --key PATH`: the
/// installation's CA (0066), made once by the installer. Writes its
/// certificate (0644) and key (0600, the secret of `CA_KEY_FILE`), refuses
/// to overwrite either, and prints only the certificate's SHA-256, what
/// the join command checks it with (`--ca-sha256`, 0029).
pub(crate) fn ca_init() -> CommandResult {
    let args: Vec<String> = std::env::args().skip(2).collect();
    println!("{}", ca_init_with(&args)?);
    Ok(())
}

const CA_INIT_USAGE: &str = "usage: server ca-init --name INSTALLATION --cert PATH --key PATH";

/// [`ca_init`] on these arguments: the new certificate's fingerprint.
fn ca_init_with(args: &[String]) -> Result<String, Box<dyn std::error::Error>> {
    let (mut name, mut cert, mut key) = (None, None, None);
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let slot = match flag.as_str() {
            "--name" => &mut name,
            "--cert" => &mut cert,
            "--key" => &mut key,
            _ => return Err(CA_INIT_USAGE.into()),
        };
        *slot = Some(args.next().ok_or(CA_INIT_USAGE)?.clone());
    }
    let (Some(name), Some(cert), Some(key)) = (name, cert, key) else {
        return Err(CA_INIT_USAGE.into());
    };
    let ca = Ca::create(&name, ca::DEFAULT_CA_VALIDITY)?;
    ca::write_new(&ca, Path::new(&cert), Path::new(&key))?;
    Ok(ca::sha256_fingerprint(&ca.cert_pem)?)
}

/// `server oidc-keygen [--es256]`: a new key to sign the OIDC provider's
/// tokens, as a secret: Ed25519 for `OIDC_SIGNING_KEY` or, with `--es256`,
/// P-256 for `OIDC_SIGNING_KEY_ES256`.
pub(crate) fn oidc_keygen() -> CommandResult {
    let key = match std::env::args().nth(2).as_deref() {
        None => infrastructure::oidc::Ed25519Signer::generate()?,
        Some("--es256") => infrastructure::oidc_es256::Es256Signer::generate()?,
        Some(other) => return Err(format!("oidc-keygen: unknown option {other}").into()),
    };
    println!("{key}");
    Ok(())
}

/// `server oidc-client ID NAME REDIRECT_URI... [--public] [options]`: register
/// an application with the OIDC provider. Without `--public` it gets a
/// secret, shown this one time (its hash is stored), unless `--machine-role`
/// makes its machines' certificates authenticate it (decision 0066).
pub(crate) async fn oidc_client() -> CommandResult {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let pool = infrastructure::connect(&PgConfig::from_env()?).await?;
    infrastructure::run_migrations(&pool).await?;
    for line in infrastructure::oidc::register_client(&pool, &args).await? {
        println!("{line}");
    }
    Ok(())
}

/// `server oidc-client-update ID [options]`: change an application registered
/// with the OIDC provider (its name, redirect URIs, listing and settings).
pub(crate) async fn oidc_client_update() -> CommandResult {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let pool = infrastructure::connect(&PgConfig::from_env()?).await?;
    infrastructure::run_migrations(&pool).await?;
    for line in infrastructure::oidc::update_client(&pool, &args).await? {
        println!("{line}");
    }
    Ok(())
}

/// After restoring a backup on a new server: the administrator of
/// `ADMIN_BOOTSTRAP_EMAIL` signs in with `ADMIN_BOOTSTRAP_PASSWORD` or,
/// without it, stays pending and chooses theirs with the link the installer
/// asks for (`/internal/admin-access`); the other local accounts choose their
/// password again and no session or linked external account remains (see
/// `infrastructure::admin_recovery`).
pub(crate) async fn recover_admin() -> CommandResult {
    init_tracing();
    let auth = AuthConfig::from_env()?;
    let pg_config = PgConfig::from_env()?;
    let email = std::env::var("ADMIN_BOOTSTRAP_EMAIL")
        .map_err(|_| "ADMIN_BOOTSTRAP_EMAIL is required for recover-admin")?;
    let email = domain::Email::parse(&email).map_err(|e| format!("ADMIN_BOOTSTRAP_EMAIL: {e}"))?;
    let password = std::env::var("ADMIN_BOOTSTRAP_PASSWORD")
        .ok()
        .filter(|p| !p.is_empty());
    if let Some(password) = &password {
        auth.password_policy
            .validate(password)
            .map_err(|e| format!("ADMIN_BOOTSTRAP_PASSWORD: {e}"))?;
    }
    let pool = infrastructure::connect(&pg_config).await?;
    infrastructure::run_migrations(&pool).await?;
    crate::modules::migrate(&crate::modules::registry(), &pool).await?;
    let hash = match &password {
        Some(password) => {
            auth_gateway::PasswordHasher::hash(&Argon2Hasher::new(auth.argon2)?, password).await?
        }
        None => auth_gateway::PasswordHash::pending_activation(),
    };
    let done = infrastructure::admin_recovery::recover_admin(&pool, &email, &hash).await?;
    tracing::warn!(
        email = %email,
        created = done.created,
        accounts_to_reset = done.accounts_to_reset,
        sessions_closed = done.sessions_closed,
        links_removed = done.links_removed,
        "recovered after restoring a backup"
    );
    Ok(())
}

/// Create the first administrator from environment secrets, then exit.
///
/// The email and password come from `ADMIN_BOOTSTRAP_EMAIL` and
/// `ADMIN_BOOTSTRAP_PASSWORD` — secrets injected at run time, never checked into
/// the repository. The operation is idempotent-friendly: it refuses to run once
/// any admin exists, so re-running it cannot reset an account.
pub(crate) async fn bootstrap_admin() -> CommandResult {
    init_tracing();
    let auth = AuthConfig::from_env()?;
    let pg_config = PgConfig::from_env()?;

    let email = std::env::var("ADMIN_BOOTSTRAP_EMAIL")
        .map_err(|_| "ADMIN_BOOTSTRAP_EMAIL is required for bootstrap-admin")?;
    let password = std::env::var("ADMIN_BOOTSTRAP_PASSWORD")
        .map_err(|_| "ADMIN_BOOTSTRAP_PASSWORD is required for bootstrap-admin")?;

    // Ensure the schema exists before we touch it — bootstrap may run against a
    // brand-new database, before the server has ever served.
    let pool = infrastructure::connect(&pg_config).await?;
    infrastructure::run_migrations(&pool).await?;
    crate::modules::migrate(&crate::modules::registry(), &pool).await?;

    let hasher = Arc::new(Argon2Hasher::new(auth.argon2)?);
    let repo = Arc::new(PgAdminRepository::new(pool));
    let bootstrap = BootstrapService::new(repo, hasher, auth.password_policy);

    match bootstrap.create_first_admin(&email, &password).await? {
        BootstrapOutcome::Created(id) => {
            tracing::info!(%email, %id, "created first admin");
        }
        BootstrapOutcome::AlreadyInitialized => {
            tracing::info!("an administrator already exists; nothing to do");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ca_init_writes_a_ca_prints_its_fingerprint_and_refuses_to_overwrite() {
        let dir = std::env::temp_dir().join(format!("qd-ca-init-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let (cert, key) = (dir.join("ca.crt"), dir.join("ca.key"));
        let args: Vec<String> = vec![
            "--name".into(),
            "uno".into(),
            "--cert".into(),
            cert.display().to_string(),
            "--key".into(),
            key.display().to_string(),
        ];
        let fingerprint = ca_init_with(&args).unwrap();
        let cert_pem = std::fs::read_to_string(&cert).unwrap();
        assert_eq!(fingerprint, ca::sha256_fingerprint(&cert_pem).unwrap());
        assert!(Ca::load(&cert_pem, &std::fs::read_to_string(&key).unwrap()).is_ok());

        let again = ca_init_with(&args).unwrap_err().to_string();
        assert!(again.contains("not overwritten"), "{again}");
        assert_eq!(std::fs::read_to_string(&cert).unwrap(), cert_pem);
        assert!(ca_init_with(&args[..4]).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
