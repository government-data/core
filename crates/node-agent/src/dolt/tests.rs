use std::collections::VecDeque;
use std::sync::Mutex;

use super::*;
use crate::cert::tests::TestCa;
use crate::ports::{ManualClock, ScriptedCommands};
use crate::testing::TempDir;

const YEAR: Duration = Duration::from_secs(365 * 24 * 3600);
const DAY: Duration = Duration::from_secs(24 * 3600);

/// The API's two routes: answers given in turn, and what was presented.
#[derive(Default)]
struct Gate {
    holds: Mutex<VecDeque<Result<HoldAnswer, CallError>>>,
    calls: Mutex<Vec<(&'static str, String)>>,
}

#[async_trait]
impl Release for Gate {
    async fn release(&self, identity: &Credentials, _: &str) -> Result<bool, CallError> {
        let call = ("release", identity.cert_pem.clone());
        self.calls.lock().unwrap().push(call);
        Ok(true)
    }
}

#[async_trait]
impl DoltGate for Gate {
    async fn hold(&self, identity: &Credentials, _: &str) -> Result<HoldAnswer, CallError> {
        let call = ("hold", identity.cert_pem.clone());
        self.calls.lock().unwrap().push(call);
        let next = self.holds.lock().unwrap().pop_front();
        next.unwrap_or(Err(CallError::Unavailable(503)))
    }
}

/// Dolt's port: answers or not.
struct Port(bool);

#[async_trait]
impl Probe for Port {
    async fn back(&self, _: Duration) -> bool {
        self.0
    }
}

struct World {
    _dir: TempDir,
    store: FileStore,
    ca: TestCa,
    gate: Gate,
    clock: ManualClock,
    commands: ScriptedCommands,
    follower: Follower,
}

impl World {
    fn new() -> Self {
        let dir = TempDir::new();
        let store = FileStore::open(dir.path()).unwrap();
        let ca = TestCa::new();
        store.write_ca(&ca.cert_pem).unwrap();
        let world = Self {
            _dir: dir,
            store,
            ca,
            gate: Gate::default(),
            clock: ManualClock::at(SystemTime::now()),
            commands: ScriptedCommands::default(),
            follower: Follower {
                after: DAY,
                how: Restart {
                    command: vec!["docker".into(), "restart".into(), "dolt".into()],
                    wait: Duration::from_secs(120),
                    poll: Duration::from_secs(1),
                },
            },
        };
        world.renew();
        world
    }

    /// The agent renewed in rolling mode: a new certificate in use.
    fn renew(&self) {
        let next = self.ca.credentials(self.clock.now(), YEAR);
        self.store.store_next(&next).unwrap();
        self.store.promote().unwrap();
    }

    async fn look(&self, port: bool) -> Followed {
        let (store, gate, clock) = (&self.store, &self.gate, &self.clock);
        let probe = Port(port);
        self.follower
            .look(store, gate, &probe, &self.commands, clock)
            .await
    }

    fn calls(&self) -> Vec<&'static str> {
        self.gate
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|c| c.0)
            .collect()
    }
}

fn answer(free: bool) -> Result<HoldAnswer, CallError> {
    Ok(HoldAnswer {
        free,
        in_flight: u64::from(!free),
        running: free.then_some(0),
    })
}

#[tokio::test]
async fn dolt_is_followed_only_once_its_certificate_is_old_and_replaced() {
    let w = World::new();
    assert_eq!(w.look(true).await, Followed::Recorded, "the first run");
    assert_eq!(w.look(true).await, Followed::Current);

    // Renewed every 8 hours: Dolt's is another, but not a day old.
    w.clock.advance(Duration::from_secs(8 * 3600));
    w.renew();
    assert_eq!(w.look(true).await, Followed::NotYet);
    assert!(w.calls().is_empty(), "nothing asked");
    assert!(w.commands.ran().is_empty(), "nothing restarted");
}

#[tokio::test]
async fn busy_dolt_is_asked_again_and_restarted_once_free() {
    let w = World::new();
    w.look(true).await;
    w.clock.advance(DAY);
    w.renew();
    w.gate.holds.lock().unwrap().push_back(answer(false));
    assert_eq!(w.look(true).await, Followed::Busy("not free".into()));
    assert!(w.commands.ran().is_empty(), "not restarted while busy");
    assert!(matches!(w.look(true).await, Followed::Busy(_)), "API down");

    w.gate.holds.lock().unwrap().push_back(answer(true));
    w.commands.answering(&[true]);
    assert_eq!(w.look(true).await, Followed::Restarted(true));
    let restart = vec!["docker".to_string(), "restart".into(), "dolt".into()];
    assert_eq!(w.commands.ran(), vec![restart]);
    assert_eq!(w.calls(), ["hold", "hold", "hold", "release"]);
    let presented = w.gate.calls.lock().unwrap().last().unwrap().1.clone();
    let current = w.store.current().unwrap().unwrap().cert_pem;
    assert_eq!(presented, current, "released with the certificate in use");
    assert_eq!(w.look(true).await, Followed::Current, "recorded");
}

#[tokio::test]
async fn dolt_not_back_leaves_the_hold_and_is_tried_again() {
    let w = World::new();
    w.look(true).await;
    w.clock.advance(DAY);
    w.renew();
    w.gate.holds.lock().unwrap().push_back(answer(true));
    w.commands.answering(&[true]);
    let failed = w.look(false).await;
    assert_eq!(
        failed,
        Followed::Failed(RestartError::NotBack(Duration::from_secs(120)))
    );
    assert_eq!(w.calls(), ["hold"], "no release: the hold expires");

    // A failed command too; the record is unchanged, so it is asked again.
    w.gate.holds.lock().unwrap().push_back(answer(true));
    w.commands.answering(&[false]);
    assert!(matches!(
        w.look(true).await,
        Followed::Failed(RestartError::Command(_))
    ));
    w.gate.holds.lock().unwrap().push_back(answer(true));
    w.commands.answering(&[true]);
    assert_eq!(w.look(true).await, Followed::Restarted(true));
}

#[test]
fn a_record_is_read_back() {
    let dir = TempDir::new();
    let path = dir.path().join("dolt.followed");
    let at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    assert_eq!(
        write_or(&path, "ab", at, Followed::Current),
        Followed::Current
    );
    assert_eq!(read_record(&path), Some(("ab".into(), at)));
    std::fs::write(&path, "garbage").unwrap();
    assert_eq!(read_record(&path), None);
}
