use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use super::*;
use crate::cert::tests::TestCa;
use crate::ports::{ManualClock, ScriptedCommands};
use crate::testing::TempDir;

const YEAR: Duration = Duration::from_secs(365 * 24 * 3600);
const POLL: Duration = Duration::from_secs(2);

/// The API's two routes: answers given in turn, and the certificate
/// presented by each call.
#[derive(Default)]
struct Gate {
    holds: Mutex<VecDeque<Result<HoldAnswer, CallError>>>,
    releases: Mutex<VecDeque<Result<bool, CallError>>>,
    presented: Mutex<Vec<(&'static str, String)>>,
}

#[async_trait]
impl TrinoGate for Gate {
    async fn hold(&self, identity: &Credentials, _: &str) -> Result<HoldAnswer, CallError> {
        let presented = ("hold", identity.cert_pem.clone());
        self.presented.lock().unwrap().push(presented);
        let next = self.holds.lock().unwrap().pop_front();
        next.unwrap_or(Err(CallError::Unavailable(503)))
    }

    async fn release(&self, identity: &Credentials, _: &str) -> Result<bool, CallError> {
        let presented = ("release", identity.cert_pem.clone());
        self.presented.lock().unwrap().push(presented);
        let next = self.releases.lock().unwrap().pop_front();
        next.unwrap_or(Ok(true))
    }
}

/// Trino: down for `down` asks, then started `uptime` ago, or never.
struct Trino {
    down: Mutex<u32>,
    then: Option<Info>,
}

#[async_trait]
impl TrinoInfo for Trino {
    async fn info(&self) -> Result<Info, String> {
        let mut down = self.down.lock().unwrap();
        if *down > 0 {
            *down -= 1;
            return Err("connection refused".into());
        }
        self.then.ok_or_else(|| "connection refused".into())
    }
}

struct World {
    _dir: TempDir,
    store: FileStore,
    ca: TestCa,
    gate: Arc<Gate>,
    clock: ManualClock,
    commands: ScriptedCommands,
}

impl World {
    fn new() -> Self {
        let dir = TempDir::new();
        let store = FileStore::open(dir.path()).unwrap();
        let ca = TestCa::new();
        let clock = ManualClock::at(SystemTime::now());
        store.write_ca(&ca.cert_pem).unwrap();
        store
            .store_next(&ca.credentials(clock.now(), YEAR))
            .unwrap();
        store.promote().unwrap();
        Self {
            _dir: dir,
            store,
            ca,
            gate: Arc::new(Gate::default()),
            clock,
            commands: ScriptedCommands::default(),
        }
    }

    fn current(&self) -> String {
        self.store.current().unwrap().unwrap().cert_pem
    }

    async fn restart(&self, trino: &Trino) -> Result<bool, RestartError> {
        let how = Restart {
            command: vec!["docker".into(), "restart".into(), "trino".into()],
            wait: Duration::from_secs(240),
            poll: POLL,
        };
        let (gate, clock) = (self.gate.as_ref(), &self.clock);
        restart(&self.store, gate, trino, &self.commands, clock, &how).await
    }
}

fn answer(free: bool, jobs: u64, queries: Option<u64>) -> Result<HoldAnswer, CallError> {
    Ok(HoldAnswer {
        free,
        running_jobs: jobs,
        running_queries: queries,
    })
}

#[tokio::test]
async fn ready_only_when_the_api_says_free() {
    let w = World::new();
    w.gate.holds.lock().unwrap().extend([
        answer(false, 1, Some(0)),
        answer(false, 0, None),
        answer(true, 0, Some(0)),
    ]);
    assert_eq!(ready(&w.store, w.gate.as_ref()).await, Ok(false));
    assert_eq!(ready(&w.store, w.gate.as_ref()).await, Ok(false));
    assert_eq!(ready(&w.store, w.gate.as_ref()).await, Ok(true));
    let refused = ready(&w.store, w.gate.as_ref()).await;
    assert!(refused.unwrap_err().contains("503"));
    let presented = w.gate.presented.lock().unwrap().clone();
    assert!(presented.iter().all(|(_, cert)| *cert == w.current()));
}

#[tokio::test]
async fn restart_waits_for_trino_then_releases_with_the_new_certificate() {
    let w = World::new();
    let old = w.current();
    // Held mode's order: ready (old), the next promoted, then the restart.
    w.gate
        .holds
        .lock()
        .unwrap()
        .push_back(answer(true, 0, Some(0)));
    assert_eq!(ready(&w.store, w.gate.as_ref()).await, Ok(true));
    let next = w.ca.credentials(w.clock.now(), YEAR);
    w.store.store_next(&next).unwrap();
    w.store.promote().unwrap();
    let trino = Trino {
        down: Mutex::new(10),
        then: Some(Info {
            starting: false,
            uptime: Duration::from_secs(3),
        }),
    };
    let start = w.clock.now();
    assert_eq!(w.restart(&trino).await, Ok(true));
    assert_eq!(w.clock.now().duration_since(start).unwrap(), POLL * 10);
    assert_eq!(w.commands.ran(), [["docker", "restart", "trino"]]);
    let presented = w.gate.presented.lock().unwrap().clone();
    assert_eq!(presented, [("hold", old), ("release", next.cert_pem)]);
}

#[tokio::test]
async fn trino_still_starting_or_never_restarted_is_not_back() {
    for then in [
        Some(Info {
            starting: true,
            uptime: Duration::from_secs(1),
        }),
        // The old process, up for hours: the command restarted nothing.
        Some(Info {
            starting: false,
            uptime: Duration::from_secs(5 * 3600),
        }),
        None,
    ] {
        let w = World::new();
        let trino = Trino {
            down: Mutex::new(0),
            then,
        };
        let err = w.restart(&trino).await.unwrap_err();
        assert_eq!(err, RestartError::NotBack(Duration::from_secs(240)));
        assert!(err.to_string().contains("stay held"));
        assert!(w.gate.presented.lock().unwrap().is_empty(), "not released");
    }
}

#[tokio::test]
async fn a_failed_restart_command_releases_nothing() {
    let w = World::new();
    w.commands.answering(&[false]);
    let trino = Trino {
        down: Mutex::new(0),
        then: None,
    };
    let err = w.restart(&trino).await.unwrap_err();
    assert!(matches!(err, RestartError::Command(_)));
    assert!(w.gate.presented.lock().unwrap().is_empty());
}

#[tokio::test]
async fn releasing_is_tried_again_and_a_refusal_is_said() {
    let back = || Trino {
        down: Mutex::new(0),
        then: Some(Info {
            starting: false,
            uptime: Duration::from_millis(500),
        }),
    };
    let w = World::new();
    let flaky = [Err(CallError::Network("reset".into())), Ok(false)];
    w.gate.releases.lock().unwrap().extend(flaky);
    assert_eq!(w.restart(&back()).await, Ok(false));
    let w = World::new();
    w.gate
        .releases
        .lock()
        .unwrap()
        .push_back(Err(CallError::Refused));
    let err = w.restart(&back()).await.unwrap_err();
    assert_eq!(err, RestartError::Release(CallError::Refused));
}

#[test]
fn trinos_durations_and_info_are_read() {
    let secs = |s: f64| Some(Duration::from_secs_f64(s));
    assert_eq!(parse_uptime("12.34s"), secs(12.34));
    assert_eq!(parse_uptime("1.50m"), secs(90.0));
    assert_eq!(parse_uptime("850.00ms"), secs(0.85));
    assert_eq!(parse_uptime("2.00h"), secs(7200.0));
    assert_eq!(parse_uptime("1.00d"), secs(86_400.0));
    for bad in ["", "s", "12", "1.0x", "-1s"] {
        assert_eq!(parse_uptime(bad), None, "{bad}");
    }
    let body = br#"{"nodeVersion":{"version":"484"},"starting":false,"uptime":"27.10s"}"#;
    let info = read_info(body).unwrap();
    assert_eq!(
        (info.starting, info.uptime),
        (false, Duration::from_secs_f64(27.1))
    );
    assert_eq!(read_info(b"{}"), None);
}
