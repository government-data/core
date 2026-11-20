//! Dolt's hold (0066, point 11): asked by the application VM's machine
//! (role `app`) alone, held only when the catalog says free, released,
//! audited.

use super::*;
use AuditEventType::{CertificateRefused, DoltHeld, DoltReleased};

/// The catalog's Dolt, in memory: free or not, and whether it is held.
#[derive(Default)]
struct Store {
    busy: AtomicU64,
    held: Mutex<bool>,
}

#[async_trait]
impl DoltStore for Store {
    async fn hold(&self, ttl: Duration) -> DoltHold {
        assert_eq!(ttl, DOLT_HOLD_TTL);
        let busy = self.busy.load(Ordering::SeqCst);
        *self.held.lock().unwrap() = busy == 0;
        DoltHold {
            free: busy == 0,
            in_flight: busy,
            running: (busy == 0).then_some(0),
        }
    }

    fn release(&self) -> bool {
        std::mem::take(&mut *self.held.lock().unwrap())
    }
}

/// The application VM's machine, `qd-app`, with the certificate `0a`, in
/// `state`: made as the installer makes it (point 16), never by joining.
fn app_machine(state: MachineState) -> Machine {
    let at = SystemTime::UNIX_EPOCH;
    Machine {
        id: MachineId::new("m-app"),
        name: "qd-app".into(),
        role: MachineRole::App,
        pool: None,
        state,
        version: None,
        cert_serial: Some("0a".into()),
        cert_not_after: Some(at + DAY),
        cert_serial_next: None,
        cert_not_after_next: None,
        created_at: at,
        approved_at: None,
        approved_by: None,
        retired_at: None,
        retired_by: None,
    }
}

/// A world with the application VM's machine in `state` and the catalog.
fn with_dolt(state: MachineState) -> (World, Arc<Store>) {
    let memory = Memory::default();
    memory.machines.lock().unwrap().push(app_machine(state));
    let audit = Arc::new(Audit::default());
    let clock = Arc::new(FixedClock(Mutex::new(SystemTime::UNIX_EPOCH)));
    let store = Arc::new(Store::default());
    let service = MachinesService::new(
        Arc::new(memory),
        Arc::new(Mint::default()),
        audit.clone(),
        clock.clone(),
    )
    .with_dolt(store.clone());
    (
        World {
            service,
            audit,
            clock,
        },
        store,
    )
}

fn qd_app() -> auth_gateway::MachineCertificate {
    presented("app", "qd-app", "0a")
}

fn last(w: &World) -> (AuditEventType, String) {
    let events = w.audit.0.lock().unwrap();
    let e = events.last().unwrap();
    (e.event_type, e.action.clone())
}

#[tokio::test]
async fn dolt_is_held_only_when_free_and_released_by_the_app_machine() {
    let (w, store) = with_dolt(Approved);
    let (s, o) = (&w.service, origin());
    store.busy.store(2, Ordering::SeqCst);
    let busy = s.hold_dolt(&qd_app(), &o).await.unwrap();
    assert_eq!((busy.free, busy.in_flight), (false, 2));
    assert!(!*store.held.lock().unwrap(), "nothing held");
    assert_eq!(last(&w), (DoltHeld, "dolt:busy".into()));

    store.busy.store(0, Ordering::SeqCst);
    let free = s.hold_dolt(&qd_app(), &o).await.unwrap();
    assert_eq!((free.free, free.running), (true, Some(0)));
    assert_eq!(last(&w), (DoltHeld, "dolt:held".into()));
    assert_eq!(s.release_dolt(&qd_app(), &o).await, Ok(true));
    assert_eq!(last(&w), (DoltReleased, "dolt:released".into()));
    assert_eq!(s.release_dolt(&qd_app(), &o).await, Ok(false), "expired");
    assert_eq!(last(&w), (DoltReleased, "dolt:expired".into()));
}

#[tokio::test]
async fn dolt_is_held_by_no_other_certificate() {
    for state in [MachineState::Pending, Retired] {
        let (w, store) = with_dolt(state);
        let refused = w.service.hold_dolt(&qd_app(), &origin()).await;
        assert_eq!(refused, Err(DoltHoldError::Refused), "{state:?}");
        assert!(!*store.held.lock().unwrap());
    }
    let (w, _) = with_dolt(Draining);
    assert!(w.service.hold_dolt(&qd_app(), &origin()).await.is_ok());
    let (w, _) = with_dolt(Approved);
    let (s, o) = (&w.service, origin());
    for other in [
        presented("worker", "qd-app", "0a"),
        presented("trino-coordinator", "qd-app", "0a"),
        presented("app", "qd-app", "0b"),
        presented("app", "qd-other", "0a"),
    ] {
        assert_eq!(s.hold_dolt(&other, &o).await, Err(DoltHoldError::Refused));
        assert_eq!(
            s.release_dolt(&other, &o).await,
            Err(DoltHoldError::Refused)
        );
    }
    assert_eq!(last(&w), (CertificateRefused, "dolt:certificate".into()));
}

#[tokio::test]
async fn without_a_catalog_dolt_is_unavailable() {
    let w = world();
    let refused = w.service.hold_dolt(&qd_app(), &origin()).await;
    assert_eq!(refused, Err(DoltHoldError::Unavailable));
}
