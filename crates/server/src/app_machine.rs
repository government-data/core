//! `server app-machine --name NAME --dir PATH` (decision 0066, point 16):
//! the installer, on the application VM where the CA's key is, makes the
//! machine of role `app` approved and signs its first certificate on the
//! spot, with no join token and no call to the API. The key is made here,
//! on that VM, and never leaves it; the files are the node agent's own
//! (`node_agent::store`, written aside and renamed, the key 0600), so its
//! agent renews from then on as any machine's.
//!
//! It reads the CA as `serve` does (`CA_CERT_FILE`, `CA_KEY_FILE`) and the
//! database of `DATABASE_URL`. Run twice, it makes no second machine: if
//! the directory already holds the machine's certificate nothing is done,
//! otherwise one is signed again for the same machine. It prints the
//! machine, the serial and the date; never the key or the certificate.

use std::path::Path;
use std::sync::Arc;

use application::{InstallError, Installed, MachinesService, Origin};
use infrastructure::ca::{certificate_info, Ca};
use infrastructure::{PgAuditRepository, PgConfig, PgMachines, RingJoinTokenMint, SystemClock};
use node_agent::cert::new_request;
use node_agent::store::FileStore;

type CommandResult = Result<(), Box<dyn std::error::Error>>;

const USAGE: &str = "usage: server app-machine --name NAME --dir PATH";

pub(crate) async fn run() -> CommandResult {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let (name, dir) = parse(&args)?;
    let machines = signing_machines("app-machine").await?;
    println!("{}", install(&machines, &name, Path::new(&dir)).await?);
    Ok(())
}

/// The machines' use cases with the CA (`CA_CERT_FILE`, `CA_KEY_FILE`),
/// over the database of `DATABASE_URL`, its migrations run: for a
/// `command` that signs a certificate on the spot.
pub(crate) async fn signing_machines(
    command: &str,
) -> Result<MachinesService, Box<dyn std::error::Error>> {
    let ca = Ca::from_env()?
        .ok_or_else(|| format!("{command} needs the CA: CA_CERT_FILE and CA_KEY_FILE"))?;
    let pool = infrastructure::connect(&PgConfig::from_env()?).await?;
    infrastructure::run_migrations(&pool).await?;
    Ok(MachinesService::new(
        Arc::new(PgMachines::new(pool.clone())),
        Arc::new(RingJoinTokenMint::new()),
        Arc::new(PgAuditRepository::new(pool)),
        Arc::new(SystemClock),
    )
    .with_signer(Arc::new(ca)))
}

/// `--name` and `--dir`, both required, nothing else; `Err` for the usage.
pub(crate) fn parse(args: &[String]) -> Result<(String, String), &'static str> {
    let (mut name, mut dir) = (None, None);
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let slot = match flag.as_str() {
            "--name" => &mut name,
            "--dir" => &mut dir,
            _ => return Err(USAGE),
        };
        *slot = Some(args.next().ok_or(USAGE)?.clone());
    }
    name.zip(dir).ok_or(USAGE)
}

/// The machine `name` installed with the agent's files in `dir`: what the
/// command prints.
async fn install(
    machines: &MachinesService,
    name: &str,
    dir: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let store = FileStore::open(dir)?;
    let request = new_request(name)?;
    let origin = Origin {
        ip: "127.0.0.1".into(),
        user_agent: Some("server app-machine".into()),
    };
    let held = held_serial(&store);
    let installed = machines
        .install_app(name, &request.csr_pem, held.as_deref(), &origin)
        .await
        .map_err(refusal)?;
    let (machine, serial, not_after) = match installed {
        Installed::Unchanged(machine) => {
            return Ok(format!(
                "{}: unchanged, its certificate is there",
                machine.name
            ));
        }
        Installed::Signed {
            machine,
            certificate,
            ca,
        } => {
            // As the agent's join ends: the CA, the bundle, then promoted.
            store.write_ca(&ca)?;
            store.store_next(&request.credentials(&certificate.pem))?;
            store.promote()?;
            (machine, certificate.serial, certificate.not_after)
        }
    };
    let until = chrono::DateTime::<chrono::Utc>::from(not_after).to_rfc3339();
    Ok(format!(
        "{} ({}, {}): certificate {serial} until {until}",
        machine.name,
        machine.role.as_str(),
        machine.state.as_str()
    ))
}

/// The serial of the certificate in the agent's directory, when there is
/// one whose key is there with it.
fn held_serial(store: &FileStore) -> Option<String> {
    let current = store.current().ok().flatten()?;
    current.validity().ok()?;
    certificate_info(&current.cert_pem).ok().map(|i| i.serial)
}

pub(crate) fn refusal(e: InstallError) -> String {
    match e {
        InstallError::InvalidName => {
            "the name must be lowercase letters, digits and hyphens".into()
        }
        InstallError::NameTaken => "a machine of another role has the name".into(),
        InstallError::InvalidRequest => "the CA refused the signing request".into(),
        InstallError::Unavailable => "no CA".into(),
        InstallError::Backend(e) => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_machine_takes_a_name_and_a_directory() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let parsed = parse(&args(&["--dir", "/run/qd-node", "--name", "qd-app"]));
        assert_eq!(parsed, Ok(("qd-app".into(), "/run/qd-node".into())));
        assert_eq!(parse(&args(&["--name", "qd-app"])), Err(USAGE));
        assert_eq!(parse(&args(&["--name"])), Err(USAGE));
        assert_eq!(parse(&args(&["--role", "app"])), Err(USAGE));
    }
}
