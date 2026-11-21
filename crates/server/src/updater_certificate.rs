//! `server updater-certificate --name NAME --dir PATH` (decision 0003): the
//! installer, where the CA's key is, makes the update agent's machine (role
//! `updater`) approved and signs its certificate on the spot, as
//! `app-machine` does for the application VM. The key is made here and
//! written with the certificate and the CA into `PATH` (`ca.pem`,
//! `certificate.pem`, `private-key.pem` 0600: the agent's
//! `/etc/quality-data/internal/`), which it presents on the internal entry
//! point to call `/updater/v1/`.
//!
//! The agent does not renew it: the certificate lasts a year, and running
//! the command again signs a new one once less than 30 days are left (or
//! when the files are missing or are not the machine's); otherwise nothing
//! is done. It prints the machine, the serial and the date; never the key
//! or the certificate.

use std::path::Path;
use std::time::SystemTime;

use application::{Installed, MachinesService, Origin};
use infrastructure::ca::UpdaterFiles;
use node_agent::cert::new_request;

use crate::app_machine::{parse, refusal, signing_machines};

const USAGE: &str = "usage: server updater-certificate --name NAME --dir PATH";

pub(crate) async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let (name, dir) = parse(&args).map_err(|_| USAGE)?;
    let machines = signing_machines("updater-certificate").await?;
    println!("{}", install(&machines, &name, Path::new(&dir)).await?);
    Ok(())
}

/// The update agent `name` with its files in `dir`: what the command prints.
async fn install(
    machines: &MachinesService,
    name: &str,
    dir: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let files = UpdaterFiles::open(dir)?;
    let request = new_request(name)?;
    let origin = Origin {
        ip: "127.0.0.1".into(),
        user_agent: Some("server updater-certificate".into()),
    };
    let held = files.held(name, SystemTime::now());
    let installed = machines
        .install_updater(name, &request.csr_pem, held.as_deref(), &origin)
        .await
        .map_err(refusal)?;
    let Installed::Signed {
        machine,
        certificate,
        ca,
    } = installed
    else {
        return Ok(format!("{name}: unchanged, its certificate is there"));
    };
    let credentials = request.credentials(&certificate.pem);
    files.write(&ca, &credentials.cert_pem, &credentials.key_pem)?;
    let until = chrono::DateTime::<chrono::Utc>::from(certificate.not_after).to_rfc3339();
    Ok(format!(
        "{} ({}, {}): certificate {} until {until}",
        machine.name,
        machine.role.as_str(),
        machine.state.as_str(),
        certificate.serial
    ))
}
