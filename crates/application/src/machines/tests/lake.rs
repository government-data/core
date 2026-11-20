//! The lake's gate (0066, point 10): which machines pass, the passes kept
//! for `LAKE_CHECK_TTL` without asking the registry, the refusals audited
//! and never kept, and who may ask for another: a gate alone.

use super::*;
use AuditEventType::CertificateRefused;

/// `name`, of `role`, in `state`, using `0a` and with `0b` as its next.
fn machine(name: &str, role: MachineRole, state: MachineState) -> Machine {
    let at = SystemTime::UNIX_EPOCH;
    Machine {
        id: MachineId::new(format!("m-{name}")),
        name: name.into(),
        role,
        pool: None,
        state,
        version: None,
        cert_serial: Some("0a".into()),
        cert_not_after: Some(at + DAY),
        cert_serial_next: Some("0b".into()),
        cert_not_after_next: Some(at + DAY * 2),
        created_at: at,
        approved_at: None,
        approved_by: None,
        retired_at: None,
        retired_by: None,
    }
}

/// A world with `machines`, and the registry to count its lookups.
fn with(machines: Vec<Machine>) -> (World, Arc<Memory>) {
    let memory = Arc::new(Memory::default());
    *memory.machines.lock().unwrap() = machines;
    let audit = Arc::new(Audit::default());
    let clock = Arc::new(FixedClock(Mutex::new(SystemTime::UNIX_EPOCH)));
    let service = MachinesService::new(
        memory.clone(),
        Arc::new(Mint::default()),
        audit.clone(),
        clock.clone(),
    );
    let world = World {
        service,
        audit,
        clock,
    };
    (world, memory)
}

fn finds(memory: &Memory) -> u64 {
    memory.finds.load(Ordering::SeqCst)
}

async fn check(w: &World, unit: &str, name: &str, serial: &str) -> Result<(), LakeCheckError> {
    let presented = presented(unit, name, serial);
    w.service.check_lake(&presented, &origin()).await
}

#[tokio::test]
async fn approved_and_draining_machines_of_the_lakes_roles_pass_unaudited() {
    for role in LAKE_ROLES {
        for state in [Approved, Draining] {
            let (w, _) = with(vec![machine("m-01", role, state)]);
            let passed = check(&w, role.as_str(), "m-01", "0a").await;
            assert_eq!(passed, Ok(()), "{role:?} {state:?}");
            assert!(w.kinds().is_empty(), "a pass is not audited");
        }
    }
}

#[tokio::test]
async fn pending_retired_and_other_roles_are_refused_and_audited() {
    let (w, _) = with(vec![
        machine("pending-01", Worker, MachineState::Pending),
        machine("retired-01", Worker, Retired),
        machine("worker-01", Worker, Approved),
    ]);
    for (unit, name) in [
        ("worker", "pending-01"),
        ("worker", "retired-01"),
        ("auditor", "worker-01"),
        ("lake", "worker-01"),
        ("worker", "unknown-01"),
    ] {
        let refused = check(&w, unit, name, "0a").await;
        assert_eq!(refused, Err(LakeCheckError::Refused), "{unit} {name}");
    }
    let events = w.audit.0.lock().unwrap();
    assert_eq!(events.len(), 5);
    assert!(events
        .iter()
        .all(|e| e.event_type == CertificateRefused && e.action == "lake:certificate"));
}

#[tokio::test]
async fn only_the_serial_in_use_or_the_next_passes_and_the_next_is_promoted() {
    let (w, memory) = with(vec![machine("worker-01", Worker, Approved)]);
    let refused = check(&w, "worker", "worker-01", "0c").await;
    assert_eq!(refused, Err(LakeCheckError::Refused));
    assert_eq!(check(&w, "worker", "worker-01", "0B").await, Ok(()));
    let now = memory.machines.lock().unwrap()[0].clone();
    assert_eq!(
        (now.cert_serial.as_deref(), now.cert_serial_next),
        (Some("0b"), None),
        "the next one, first presented, is the one in use"
    );
    let replaced = check(&w, "worker", "worker-01", "0a").await;
    assert_eq!(replaced, Err(LakeCheckError::Refused), "the one before");
}

#[tokio::test]
async fn a_pass_is_kept_for_the_ttl_without_asking_the_registry() {
    let (w, memory) = with(vec![machine("worker-01", Worker, Approved)]);
    assert_eq!(check(&w, "worker", "worker-01", "0a").await, Ok(()));
    assert_eq!(finds(&memory), 1);
    w.advance(LAKE_CHECK_TTL - Duration::from_millis(1));
    assert_eq!(check(&w, "worker", "worker-01", "0A").await, Ok(()));
    assert_eq!(finds(&memory), 1, "a hit does not ask");
    w.advance(Duration::from_millis(1));
    assert_eq!(check(&w, "worker", "worker-01", "0a").await, Ok(()));
    assert_eq!(finds(&memory), 2, "once the TTL is over, it asks again");
}

#[tokio::test]
async fn a_retired_machine_keeps_the_lake_no_longer_than_the_ttl() {
    let (w, _) = with(vec![machine("worker-01", Worker, Approved)]);
    let id = MachineId::new("m-worker-01");
    assert_eq!(check(&w, "worker", "worker-01", "0a").await, Ok(()));
    w.service.retire(&id, &admin(), &origin()).await.unwrap();
    assert_eq!(check(&w, "worker", "worker-01", "0a").await, Ok(()));
    w.advance(LAKE_CHECK_TTL);
    let refused = check(&w, "worker", "worker-01", "0a").await;
    assert_eq!(refused, Err(LakeCheckError::Refused));
}

#[tokio::test]
async fn a_refusal_is_not_kept() {
    let (w, memory) = with(vec![machine("worker-01", Worker, MachineState::Pending)]);
    let refused = check(&w, "worker", "worker-01", "0a").await;
    assert_eq!(refused, Err(LakeCheckError::Refused));
    let id = MachineId::new("m-worker-01");
    w.service.approve(&id, &admin(), &origin()).await.unwrap();
    assert_eq!(check(&w, "worker", "worker-01", "0a").await, Ok(()));
    assert_eq!(finds(&memory), 2);
}

/// As the gate `gate` (its `OU` and name, serial `0a`), for `named`.
async fn ask(
    w: &World,
    gate: (&str, &str),
    named: Option<(&str, &str)>,
) -> Result<(), LakeCheckError> {
    let (asker, origin) = (presented(gate.0, gate.1, "0a"), origin());
    let named = named.map(|(unit, name)| presented(unit, name, "0a"));
    let asked = w.service.check_lake_for(&asker, named.as_ref(), &origin);
    asked.await
}

#[tokio::test]
async fn a_gate_of_the_lake_asks_for_the_machine_it_names() {
    for role in LAKE_GATE_ROLES {
        let (w, memory) = with(vec![
            machine("gate-01", role, Approved),
            machine("worker-01", Worker, Approved),
        ]);
        let gate = (role.as_str(), "gate-01");
        assert_eq!(ask(&w, gate, Some(("worker", "worker-01"))).await, Ok(()));
        assert_eq!(finds(&memory), 2, "the gate, then the machine");
        assert_eq!(ask(&w, gate, Some(("worker", "worker-01"))).await, Ok(()));
        assert_eq!(finds(&memory), 2, "both passes kept");
        assert!(w.kinds().is_empty(), "a pass is not audited");
        // The gate's pass says nothing of the machines it names.
        let unknown = ask(&w, gate, Some(("worker", "worker-02"))).await;
        assert_eq!(unknown, Err(LakeCheckError::Refused));
        assert_eq!(w.audit.0.lock().unwrap()[0].action, "lake:certificate");
    }
}

#[tokio::test]
async fn who_is_not_a_gate_is_refused_whatever_it_names() {
    let (w, _) = with(vec![
        machine("worker-01", Worker, Approved),
        machine("worker-02", Worker, Approved),
        machine("gate-01", MachineRole::Lake, MachineState::Pending),
        machine("gate-02", MachineRole::Lake, Retired),
    ]);
    for gate in [
        ("worker", "worker-02"),
        ("lake", "worker-02"),
        ("lake", "gate-01"),
        ("lake", "gate-02"),
        ("lake", "unknown-01"),
    ] {
        // Naming an approved worker, or itself.
        for named in [("worker", "worker-01"), gate] {
            let refused = ask(&w, gate, Some(named)).await;
            assert_eq!(refused, Err(LakeCheckError::Refused), "{gate:?}");
        }
    }
    let events = w.audit.0.lock().unwrap();
    assert_eq!(events.len(), 10);
    assert!(events
        .iter()
        .all(|e| e.event_type == CertificateRefused && e.action == "lake:gate"));
}

#[tokio::test]
async fn a_gate_that_names_no_one_is_refused_and_a_retired_gate_stops_asking() {
    let (w, _) = with(vec![
        machine("gate-01", MachineRole::Lake, Approved),
        machine("worker-01", Worker, Approved),
    ]);
    let gate = ("lake", "gate-01");
    assert_eq!(ask(&w, gate, None).await, Err(LakeCheckError::Refused));
    assert_eq!(w.audit.0.lock().unwrap()[0].action, "lake:unnamed");
    assert_eq!(ask(&w, gate, Some(("worker", "worker-01"))).await, Ok(()));
    let id = MachineId::new("m-gate-01");
    w.service.retire(&id, &admin(), &origin()).await.unwrap();
    w.advance(LAKE_CHECK_TTL);
    let refused = ask(&w, gate, Some(("worker", "worker-01"))).await;
    assert_eq!(refused, Err(LakeCheckError::Refused));
}
