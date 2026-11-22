//! `qd-node` (decisions 0029, 0030, 0066): the agent every machine runs. It
//! joins once with its token, then keeps the machine's certificate renewed
//! with the certificate it has, writes the files its services read, and
//! runs a command once a new one is in place. No person is involved; the
//! key never leaves the machine. Files and their order: `store.rs`.
//!
//! Configuration, from the environment:
//!
//! - `QD_AGENT_DIR`: its directory (0700): `ca.crt`, `machine.pem`
//!   (certificate then key), `machine.key`, `machine.crt`, and `next.pem`
//!   while a renewal waits;
//! - `QD_INTERNAL_URL`: the API on Traefik's internal entry point,
//!   `https://<INTERNAL_DOMAIN>:9443/api`, for renewing;
//! - `QD_RENEW_EVERY`: how often to renew (`8h`; `s`, `m`, `h`, `d`), ±10%;
//!   at start at once if under two thirds of the certificate is left;
//! - `QD_AFTER_RENEW`: a command run after a new certificate is in place
//!   (`pg_ctl reload`, `docker kill --signal HUP postgres`), without a
//!   shell: words split at whitespace, or a JSON array of strings;
//! - `QD_RENEW_MODE`: `rolling` (the default: a renewed certificate is
//!   taken at once) or `held` (taken only when `QD_HELD_READY` exits 0,
//!   asked every `QD_HELD_ASK_EVERY`, `5m`, once the one in use is older
//!   than `QD_HELD_AFTER`, `24h`: 0066, point 11).
//!
//! And to join, only while there is no `machine.pem`:
//!
//! - `QD_PUBLIC_URL`: the API on the public entry point, `https://<domain>/api`;
//! - `QD_MACHINE_NAME`: its certificate's `CN`;
//! - `QD_JOIN_TOKEN_FILE` (deleted once joined) or `QD_JOIN_TOKEN`;
//! - `QD_CA_SHA256`: the CA's fingerprint, as `server ca-init` prints it:
//!   an answer with another CA writes nothing and stops the agent;
//! - `QD_CA_FILE`: a CA to trust for the join's TLS besides the public
//!   roots (what `join.sh` checked against `--ca-sha256`). The renewal
//!   trusts `ca.crt` alone.
//!
//! On the application VM (role `app`, rolling for PostgreSQL and Traefik),
//! a follower restarts Dolt when it is free to give it the certificate in
//! use (0066, point 11), only with `QD_DOLT_RESTART` (`docker restart
//! <container>`): every `QD_HELD_ASK_EVERY` (`5m`), once the certificate Dolt
//! was last started with is older than `QD_HELD_AFTER` (`24h`) and not the
//! one in use, it asks the API to hold the catalog, restarts Dolt, waits up
//! to `QD_DOLT_WAIT` (`2m`) for `QD_DOLT_ADDR` (`host:port`) to take a
//! connection and releases the hold. Its failures are logged and never
//! stop the agent.
//!
//! Its lines are every service's (decision 0026, `logging`): flat JSON with
//! `service` `node-agent` and `vm` from `OBS_VM`, filtered by `RUST_LOG`
//! (`info`), `LOG_FORMAT=pretty` for text. Its metrics file (decision 0071)
//! goes to `QD_METRICS_DIR` (`/run/qd-metrics`, `off` for none):
//! `node-agent.prom`, renewals by result and the certificate's end.
//!
//! It exits non-zero when a person must act: the token refused (401), the
//! name taken (409), the machine refused at renewing (retired or replaced:
//! remembered in `refused`, so a restart does not ask again), another CA,
//! or a certificate that ended. Anything else is tried again, backing off
//! up to 10 minutes. Nothing it logs holds a key, a token or a certificate.

use node_agent::agent::Agent;
use node_agent::config::{DoltSettings, Settings};
use node_agent::dolt::{Followed, Follower, TcpProbe};
use node_agent::http::{DoltApi, HttpApi};
use node_agent::metrics::NodeMetrics;
use node_agent::ports::{Processes, SystemClock};
use node_agent::store::FileStore;

#[tokio::main]
async fn main() {
    logging::init(node_agent::SERVICE);
    let done = match std::env::args().nth(1).as_deref() {
        None => run().await,
        Some(other) => Err(format!("{other}: no such command").into()),
    };
    if let Err(e) = done {
        eprintln!("qd-node: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let settings = Settings::from(|name| std::env::var(name).ok())?;
    let store = FileStore::open(&settings.dir)
        .map_err(|e| format!("QD_AGENT_DIR {}: {e}", settings.dir.display()))?;
    let join_ca = match &settings.ca_file {
        Some(file) => {
            Some(std::fs::read(file).map_err(|e| format!("QD_CA_FILE {}: {e}", file.display()))?)
        }
        None => None,
    };
    if let Some(dolt) = DoltSettings::from(|name| std::env::var(name).ok())? {
        let store = FileStore::open(&settings.dir)
            .map_err(|e| format!("QD_AGENT_DIR {}: {e}", settings.dir.display()))?;
        let api = HttpApi::new("", &settings.internal_url, None);
        tokio::spawn(follow_dolt(dolt, store, api));
    }
    let public_url = settings.public_url.clone().unwrap_or_default();
    let api = HttpApi::new(&public_url, &settings.internal_url, join_ca);
    tracing::info!(mode = ?settings.mode, every = ?settings.renew_every, "qd-node started");
    let metrics = NodeMetrics::start(store.clone());
    let agent =
        Agent::new(settings, store, api, SystemClock, Processes::default()).with_renewals(metrics);
    Err(agent.run().await.into())
}

/// Dolt's follower, every `QD_HELD_ASK_EVERY`, for as long as the agent runs.
async fn follow_dolt(settings: DoltSettings, store: FileStore, api: HttpApi) {
    let follower = Follower {
        after: settings.after,
        how: node_agent::restart::Restart {
            command: settings.restart,
            wait: settings.wait,
            poll: std::time::Duration::from_secs(2),
        },
    };
    let probe = TcpProbe {
        addr: settings.addr,
    };
    let (gate, processes) = (DoltApi(&api), Processes::default());
    tracing::info!(every = ?settings.every, "following Dolt's certificate");
    loop {
        match follower
            .look(&store, &gate, &probe, &processes, &SystemClock)
            .await
        {
            Followed::Restarted(released) => {
                tracing::info!(released, "Dolt restarted with the certificate in use");
            }
            Followed::Failed(e) => tracing::error!("Dolt's restart: {e}"),
            Followed::Busy(why) => tracing::info!("Dolt not restarted yet: {why}"),
            Followed::Recorded | Followed::Current | Followed::NotYet => {}
        }
        tokio::time::sleep(settings.every).await;
    }
}
