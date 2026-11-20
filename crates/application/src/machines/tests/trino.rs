//! A machine known by its certificate on a route for one role (0066,
//! point 9): Trino's coordinator reading its rules and holding its jobs.

use super::*;
use AuditEventType::{CertificateRefused, TrinoHeld, TrinoReleased};

/// A Trino coordinator joined, with the certificate `0a`, taken to `state`.
pub(super) async fn coordinator(w: &World, state: MachineState) -> Machine {
    let (s, by, o) = (&w.service, admin(), origin());
    let issued = s
        .issue_join_token(TrinoCoordinator, None, &by, &o)
        .await
        .unwrap();
    let m = s.redeem(&issued.token, "trino-1", &o).await.unwrap();
    let end = SystemTime::UNIX_EPOCH + DAY;
    s.record_certificate(&m.id, "0a", end, &o).await.unwrap();
    if matches!(state, Approved | Draining | Retired) {
        s.approve(&m.id, &by, &o).await.unwrap();
    }
    if state == Draining {
        s.drain(&m.id, &by, &o).await.unwrap();
    }
    if state == Retired {
        s.retire(&m.id, &by, &o).await.unwrap();
    }
    m
}

#[tokio::test]
async fn a_caller_is_an_approved_or_draining_machine_of_the_role() {
    for (state, works) in [
        (MachineState::Pending, false),
        (Approved, true),
        (Draining, true),
        (Retired, false),
    ] {
        let w = world();
        coordinator(&w, state).await;
        let caller = |unit, name, serial| {
            let presented = presented(unit, name, serial);
            let s = w.service.clone();
            async move { s.caller(&presented, TrinoCoordinator).await.unwrap() }
        };
        let found = caller("trino-coordinator", "trino-1", "0A").await;
        let expected = works.then(|| "trino-1".to_string());
        assert_eq!(found.map(|m| m.name), expected, "{state:?}");
        // Another role's unit, another name, another serial: never.
        assert_eq!(caller("worker", "trino-1", "0a").await, None);
        assert_eq!(caller("trino-coordinator", "trino-2", "0a").await, None);
        assert_eq!(caller("trino-coordinator", "trino-1", "0b").await, None);
    }
}

#[tokio::test]
async fn a_caller_of_another_role_or_organization_is_none() {
    let w = world();
    let m = w.joined("worker-01").await;
    let (s, o) = (&w.service, origin());
    let end = SystemTime::UNIX_EPOCH + DAY;
    s.record_certificate(&m.id, "0a", end, &o).await.unwrap();
    s.approve(&m.id, &admin(), &o).await.unwrap();
    let worker = presented("worker", "worker-01", "0a");
    assert!(s.caller(&worker, Worker).await.unwrap().is_some());
    // A worker's certificate claiming the coordinator's unit.
    let claimed = presented("trino-coordinator", "worker-01", "0a");
    assert_eq!(s.caller(&claimed, TrinoCoordinator).await.unwrap(), None);
    assert_eq!(s.caller(&worker, TrinoCoordinator).await.unwrap(), None);
    let mut other = worker.clone();
    other.organization = Some("other".into());
    assert_eq!(s.caller(&other, Worker).await.unwrap(), None);
}

/// The queue's jobs that use Trino, in memory: `running` of them, and
/// whether they are held.
#[derive(Default)]
struct Jobs {
    running: AtomicU64,
    held: Mutex<Option<String>>,
}

#[async_trait]
impl TrinoJobs for Jobs {
    async fn hold(&self, machine: &str, ttl: Duration) -> Result<u64, String> {
        assert_eq!(ttl, TRINO_HOLD_TTL);
        *self.held.lock().unwrap() = Some(machine.into());
        Ok(self.running.load(Ordering::SeqCst))
    }

    async fn release(&self) -> Result<bool, String> {
        Ok(self.held.lock().unwrap().take().is_some())
    }
}

/// Trino, saying how many queries it runs, or not answering.
struct Queries(Result<u64, String>);

#[async_trait]
impl TrinoQueries for Queries {
    async fn active(&self) -> Result<u64, String> {
        self.0.clone()
    }
}

/// A world with an approved coordinator and Trino answering `queries`.
async fn gated(queries: Result<u64, String>) -> (World, Arc<Jobs>) {
    let mut w = world();
    let jobs = Arc::new(Jobs::default());
    w.service = w.service.with_trino(TrinoGate {
        jobs: jobs.clone(),
        queries: Arc::new(Queries(queries)),
    });
    coordinator(&w, Approved).await;
    (w, jobs)
}

fn trino_1() -> auth_gateway::MachineCertificate {
    presented("trino-coordinator", "trino-1", "0a")
}

impl World {
    fn last_action(&self) -> (AuditEventType, String) {
        let events = self.audit.0.lock().unwrap();
        let e = events.last().unwrap();
        (e.event_type, e.action.clone())
    }
}

#[tokio::test]
async fn trino_is_held_only_when_no_job_and_no_query_runs() {
    let (w, jobs) = gated(Ok(0)).await;
    let s = &w.service;
    jobs.running.store(1, Ordering::SeqCst);
    let busy = s.hold_trino(&trino_1(), &origin()).await.unwrap();
    let expected = TrinoHold {
        free: false,
        running_jobs: 1,
        running_queries: Some(0),
    };
    assert_eq!(busy, expected);
    assert_eq!(*jobs.held.lock().unwrap(), None, "released at once");
    assert_eq!(w.last_action(), (TrinoHeld, "trino:busy".into()));

    jobs.running.store(0, Ordering::SeqCst);
    let free = s.hold_trino(&trino_1(), &origin()).await.unwrap();
    assert!(free.free);
    assert_eq!(jobs.held.lock().unwrap().as_deref(), Some("trino-1"));
    assert_eq!(w.last_action(), (TrinoHeld, "trino:held".into()));

    assert!(s.release_trino(&trino_1(), &origin()).await.unwrap());
    assert_eq!(w.last_action(), (TrinoReleased, "trino:released".into()));
    assert!(!s.release_trino(&trino_1(), &origin()).await.unwrap());
    assert_eq!(w.last_action(), (TrinoReleased, "trino:expired".into()));
}

#[tokio::test]
async fn trino_running_a_query_or_not_answering_is_not_free() {
    for (queries, running) in [(Ok(2), Some(2)), (Err("refused".to_string()), None)] {
        let (w, jobs) = gated(queries).await;
        let hold = w.service.hold_trino(&trino_1(), &origin()).await.unwrap();
        assert_eq!((hold.free, hold.running_queries), (false, running));
        assert_eq!(*jobs.held.lock().unwrap(), None);
    }
}

#[tokio::test]
async fn trino_is_held_by_its_coordinator_alone() {
    let (w, jobs) = gated(Ok(0)).await;
    let m = w.joined("worker-01").await;
    let (s, o) = (&w.service, origin());
    let end = SystemTime::UNIX_EPOCH + DAY;
    s.record_certificate(&m.id, "0b", end, &o).await.unwrap();
    s.approve(&m.id, &admin(), &o).await.unwrap();
    let worker = presented("worker", "worker-01", "0b");
    assert_eq!(
        s.hold_trino(&worker, &o).await,
        Err(TrinoHoldError::Refused)
    );
    assert_eq!(
        s.release_trino(&worker, &o).await,
        Err(TrinoHoldError::Refused)
    );
    assert_eq!(*jobs.held.lock().unwrap(), None);
    assert_eq!(
        w.last_action(),
        (CertificateRefused, "trino:certificate".into())
    );
    let without = world();
    coordinator(&without, Approved).await;
    let unavailable = without.service.hold_trino(&trino_1(), &o).await;
    assert_eq!(unavailable, Err(TrinoHoldError::Unavailable));
}

#[tokio::test]
async fn a_callers_next_certificate_becomes_the_one_in_use() {
    let w = world();
    coordinator(&w, Approved).await;
    let s = &w.service;
    let old = presented("trino-coordinator", "trino-1", "0a");
    s.renew(&old, "csr", &origin()).await.unwrap();
    assert!(s.caller(&old, TrinoCoordinator).await.unwrap().is_some());
    let new = presented("trino-coordinator", "trino-1", "ab12");
    let m = s.caller(&new, TrinoCoordinator).await.unwrap().unwrap();
    assert_eq!(m.cert_serial.as_deref(), Some("ab12"));
    assert_eq!(s.caller(&old, TrinoCoordinator).await.unwrap(), None);
}
